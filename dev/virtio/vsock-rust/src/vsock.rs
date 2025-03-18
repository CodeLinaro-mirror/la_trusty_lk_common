/*
 * Copyright (c) 2024 Google Inc. All rights reserved
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files
 * (the "Software"), to deal in the Software without restriction,
 * including without limitation the rights to use, copy, modify, merge,
 * publish, distribute, sublicense, and/or sell copies of the Software,
 * and to permit persons to whom the Software is furnished to do so,
 * subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be
 * included in all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
 * CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
 * TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
 * SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

#![deny(unsafe_op_in_unsafe_fn)]
use core::ffi::c_void;
use core::ffi::CStr;
use core::ops::Deref;
use core::ops::DerefMut;
use core::ptr::eq;
use core::ptr::null_mut;
use core::time::Duration;

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::ffi::CString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use log::debug;
use log::error;
use log::info;
use log::warn;

use rust_support::handle::IPC_HANDLE_POLL_HUP;
use rust_support::handle::IPC_HANDLE_POLL_MSG;
use rust_support::handle::IPC_HANDLE_POLL_READY;
use rust_support::handle::IPC_HANDLE_POLL_SEND_UNBLOCKED;
use rust_support::ipc::iovec_kern;
use rust_support::ipc::ipc_get_msg;
use rust_support::ipc::ipc_msg_info;
use rust_support::ipc::ipc_msg_kern;
use rust_support::ipc::ipc_port_connect_async;
use rust_support::ipc::ipc_put_msg;
use rust_support::ipc::ipc_read_msg;
use rust_support::ipc::ipc_send_msg;
use rust_support::ipc::zero_uuid;
use rust_support::ipc::IPC_CONNECT_WAIT_FOR_PORT;
use rust_support::ipc::IPC_PORT_PATH_MAX;
use rust_support::sync::Mutex;
use rust_support::thread;
use rust_support::thread::sleep;
use rust_support::thread::Builder;
use rust_support::thread::Priority;
use virtio_drivers_and_devices::device::socket::SocketError;
use virtio_drivers_and_devices::device::socket::VirtIOSocket;
use virtio_drivers_and_devices::device::socket::VsockAddr;
use virtio_drivers_and_devices::device::socket::VsockConnectionManager;
use virtio_drivers_and_devices::device::socket::VsockEvent;
use virtio_drivers_and_devices::device::socket::VsockEventType;
use virtio_drivers_and_devices::transport::Transport;
use virtio_drivers_and_devices::Error as VirtioError;
use virtio_drivers_and_devices::Hal;
use virtio_drivers_and_devices::PAGE_SIZE;

use rust_support::handle::HandleRef;
use rust_support::handle_set::HandleSet;

use rust_support::Error as LkError;

use crate::err::Error;

const ACTIVE_TIMEOUT: Duration = Duration::from_secs(5);

struct TipcPortAcl {
    name: &'static CStr,
    enabled: bool,
}

// macro will generate the variable containing the ACL for all tipc ports. If the feature name is
// defined, the corresponding port will be enabled; the port will be disabled otherwise. The macro
// will generate 2 extra ports for connections that send the port name in the first package for port
// 0 and 1.
macro_rules! comm_port_feature_enable {
    ($var_name:ident[$number_ports: literal]={$({port_name: $port_name:literal, feature_name: $feature_name:literal}),+ $(,)*}) => {
    const $var_name: [TipcPortAcl; $number_ports + 2] = [
        TipcPortAcl { name: c"", enabled: true }, // connections on port zero must send port name in first packet
        TipcPortAcl { name: c"", enabled: true }, // temporary workaround to not change the port 1 to port 0
        $(
            #[cfg(feature = $feature_name)]
            TipcPortAcl { name: $port_name, enabled: true },
            #[cfg(not(feature = $feature_name))]
            TipcPortAcl { name: $port_name, enabled: false },
        )+
    ];
    }
}

// Mapping of vsock port numbers to tipc port names.
//
// Each tipc port name must be shorter than IPC_PORT_PATH_MAX.
comm_port_feature_enable! {
    PORT_MAP[8] = {
        {port_name: c"com.android.trusty.authmgr", feature_name: "authmgr"},
        {port_name: c"com.android.trusty.hwcryptooperations", feature_name: "hwcrypto_hal"},
        {port_name: c"com.android.trusty.rust.hwcryptohal.V1", feature_name: "hwcrypto_hal"},
        {port_name: c"com.android.trusty.securestorage", feature_name: "securestorage_hal"},
        {port_name: c"com.android.trusty.widevine.transact", feature_name: "widevine_aidl_comm"},
        {port_name: c"com.android.trusty.storage.proxy", feature_name: "securestorage_hal"},
        {port_name: c"com.android.trusty.gatekeeper", feature_name: "gatekeeper"},
        {port_name: c"com.android.trusty.keymint", feature_name: "keymint"},
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum VsockConnectionState {
    #[default]
    Invalid = 0,
    VsockOnly,
    TipcOnly,
    TipcConnecting,
    TipcSendBlocked,
    Active,
    TipcClosed,
    Closed,
}

#[derive(Default)]
struct VsockConnection {
    peer: VsockAddr,
    local_port: u32,
    state: VsockConnectionState,
    tipc_port_name: Option<CString>,
    href: HandleRef,
    tx_count: u64,
    tx_since_rx: u64,
    rx_count: u64,
    rx_since_tx: u64,
    rx_buffer: Box<[u8]>, // buffers data if the tipc connection blocks
    rx_pending: usize,    // how many bytes to send when tipc unblocks
}

impl VsockConnection {
    fn new(peer: VsockAddr, local_port: u32) -> Self {
        // Make rx_buffer twice as large as the vsock connection rx buffer such
        // that we can buffer pending messages if TIPC blocks.
        //
        // TODO: the ideal rx_buffer size depends on the connection so it might
        // be worthwhile to dynamically re-size the buffer in response to tipc
        // blocking or unblocking.
        let rx_buffer_len = 2 * PAGE_SIZE;
        Self {
            peer,
            local_port,
            state: VsockConnectionState::VsockOnly,
            tipc_port_name: None,
            rx_buffer: vec![0u8; rx_buffer_len].into_boxed_slice(),
            ..Default::default()
        }
    }

    fn tipc_port_name(&self) -> &str {
        self.tipc_port_name
            .as_ref()
            .map(|s| s.to_str().expect("invalid port name"))
            .unwrap_or("(no port name)")
    }

    fn print_stats(&self) {
        info!(
            "vsock: tx {:?} ({:>5?}) rx {:?} ({:>5?}) port: {}, remote {}, state {:?}",
            self.tx_since_rx,
            self.tx_count,
            self.rx_since_tx,
            self.rx_count,
            self.tipc_port_name(),
            self.peer.port,
            self.state
        );
    }

    fn tipc_try_send(&mut self) -> Result<(), Error> {
        debug_assert!(self.rx_pending > 0 && self.rx_pending < PAGE_SIZE);
        debug_assert!(
            self.state == VsockConnectionState::Active
                || self.state == VsockConnectionState::TipcSendBlocked
        );

        let length = self.rx_pending;
        let mut iov = iovec_kern { iov_base: self.rx_buffer.as_mut_ptr() as _, iov_len: length };
        let mut msg = ipc_msg_kern::new(&mut iov);

        // Safety:
        // `c.href.handle` is a handle attached to a tipc channel.
        // `msg` contains an `iov` which points to a buffer from which
        // the kernel can read `iov_len` bytes.
        let ret = unsafe { ipc_send_msg(self.href.handle(), &mut msg) };
        if ret == LkError::ERR_NOT_ENOUGH_BUFFER.into() {
            self.state = VsockConnectionState::TipcSendBlocked;
            return Ok(());
        } else if ret < 0 {
            error!("failed to send {length} bytes to {}: {ret} ", self.tipc_port_name());
            LkError::from_lk(ret)?;
        } else if ret as usize != length {
            // TODO: in streaming mode, this should not be an error. Instead, consume
            // the data that was sent and try sending the rest in the next message.
            error!("sent {ret} bytes but expected to send {length} bytes");
            return Err(LkError::ERR_BAD_LEN.into());
        }

        self.state = VsockConnectionState::Active;
        self.tx_since_rx = 0;
        self.rx_pending = 0;

        debug!("sent {length} bytes to {}", self.tipc_port_name());

        Ok(())
    }
}

/// The action to take after running the `f` closure in [`vsock_connection_lookup`].
#[derive(PartialEq, Eq)]
enum ConnectionStateAction {
    /// No action needs to be taken, so the connection stays open.
    None,

    /// TIPC has requested that the connection be closed.
    /// This closes the connection and waits for the peer to acknowledge before removing it.
    Close,

    /// We want to close the connection and remove it
    /// without waiting for the peer to acknowledge it,
    /// such as when there is an error (but also potentially other reasons).
    Remove,
}

fn vsock_connection_lookup_by(
    connections: &mut Vec<VsockConnection>,
    predicate: impl Fn(&VsockConnection) -> bool,
    f: impl FnOnce(&mut VsockConnection) -> ConnectionStateAction,
) -> Result<(), ()> {
    let index = connections.iter().position(predicate).ok_or(())?;
    let action = f(&mut connections[index]);
    if action == ConnectionStateAction::None {
        return Ok(());
    }

    if vsock_connection_close(&mut connections[index], action) {
        connections.swap_remove(index);
    }

    Ok(())
}

fn vsock_connection_lookup_peer(
    connections: &mut Vec<VsockConnection>,
    peer: VsockAddr,
    local_port: u32,
    f: impl FnOnce(&mut VsockConnection) -> ConnectionStateAction,
) -> Result<(), ()> {
    vsock_connection_lookup_by(
        connections,
        |c: &VsockConnection| c.peer == peer && c.local_port == local_port,
        f,
    )
}

fn vsock_connection_lookup_cookie(
    connections: &mut Vec<VsockConnection>,
    cookie: *mut c_void,
    f: impl FnOnce(&mut VsockConnection) -> ConnectionStateAction,
) -> Result<(), ()> {
    vsock_connection_lookup_by(
        connections,
        |c: &VsockConnection| eq(c.href.as_ptr().cast::<c_void>(), cookie),
        f,
    )
}

fn vsock_connection_close(c: &mut VsockConnection, action: ConnectionStateAction) -> bool {
    info!(
        "remote_port {}, tipc_port_name {}, state {:?}",
        c.peer.port,
        c.tipc_port_name(),
        c.state
    );

    if c.state == VsockConnectionState::VsockOnly {
        info!("tipc vsock only connection closed");
        c.state = VsockConnectionState::TipcClosed;
    }

    if c.state == VsockConnectionState::Active
        || c.state == VsockConnectionState::TipcConnecting
        || c.state == VsockConnectionState::TipcSendBlocked
    {
        // The handle set owns the only reference we have to the handle and
        // handle_set_wait might have already returned a pointer to c
        c.href.detach();
        c.href.handle_close();
        c.href.set_cookie(null_mut());
        info!("tipc handle closed");
        c.state = VsockConnectionState::TipcClosed;
    }
    if action == ConnectionStateAction::Remove && c.state == VsockConnectionState::TipcClosed {
        info!("vsock closed");
        c.state = VsockConnectionState::Closed;
    }
    if c.state == VsockConnectionState::Closed && c.href.cookie().is_null() {
        info!("remove connection");
        c.print_stats();
        return true; // remove connection
    }
    false // keep connection
}

pub struct VsockDevice<H, T>
where
    H: Hal,
    T: Transport,
{
    connections: Mutex<Vec<VsockConnection>>,
    handle_set: HandleSet,
    connection_manager: Mutex<VsockConnectionManager<H, T, 4096>>,
}

impl<H, T> VsockDevice<H, T>
where
    H: Hal,
    T: Transport,
{
    pub(crate) fn new(manager: VsockConnectionManager<H, T, 4096>) -> Self {
        Self {
            connections: Mutex::new(Vec::new()),
            handle_set: HandleSet::new(),
            connection_manager: Mutex::new(manager),
        }
    }

    fn vsock_rx_op_request(&self, peer: VsockAddr, local: VsockAddr) -> Result<(), Error> {
        debug!("dst_port {}, src_port {}", local.port, peer.port);

        // do we already have a connection?
        let mut guard = self.connections.lock();
        if guard
            .deref()
            .iter()
            .any(|connection| connection.peer == peer && connection.local_port == local.port)
        {
            return Err(LkError::ERR_ALREADY_EXISTS.into());
        };

        let mut c = VsockConnection::new(peer, local.port);

        // ports greater than 1 use port map to determine what tipc port to connect to
        if [0, 1].contains(&local.port) {
            // wait on peer to send tipc port name
        } else if (local.port as usize) < PORT_MAP.len() {
            if PORT_MAP[local.port as usize].enabled {
                c.tipc_port_name = Some(PORT_MAP[local.port as usize].name.to_owned());
                self.vsock_connect_tipc(&mut c)?;
            } else {
                return Err(LkError::ERR_NOT_VALID.into());
            }
        } else {
            return Err(LkError::ERR_OUT_OF_RANGE.into());
        }

        guard.deref_mut().push(c);

        Ok(())
    }

    fn vsock_connect_on_rx(
        &self,
        c: &mut VsockConnection,
        length: usize,
        source: VsockAddr,
        destination: VsockAddr,
    ) -> Result<(), Error> {
        // destination port should be zero or one, otherwise, connection should not
        // be in VsockOnly state (not already connected/connecting to tipc).
        assert!([0, 1].contains(&destination.port));

        let mut buffer = [0; IPC_PORT_PATH_MAX as usize];
        assert!(length < buffer.len());
        let mut data_len = self
            .connection_manager
            .lock()
            .deref_mut()
            .recv(source, destination.port, &mut buffer)
            .unwrap();
        assert!(data_len == length);
        // allow manual connect from nc in line mode
        if buffer[data_len - 1] == b'\n' as _ {
            data_len -= 1;
        }
        let port_name = &buffer[0..data_len];
        info!("port_name is {:?}", port_name);

        // should not contain any null bytes
        c.tipc_port_name = CString::new(port_name).ok();
        info!("tipc port name set to {}", c.tipc_port_name());

        self.vsock_connect_tipc(c)
    }

    fn vsock_connect_tipc(&self, c: &mut VsockConnection) -> Result<(), Error> {
        let port_name = c.tipc_port_name.as_ref().expect("tipc port name has been set");
        // invariant: port_name.count_bytes() + 1 <= IPC_PORT_PATH_MAX
        debug_assert!(port_name.count_bytes() < IPC_PORT_PATH_MAX as usize);

        // Safety:
        // - `cid`` is a valid uuid because we use a bindgen'd constant
        // - `path` points to a null-terminated C-string. The null byte was appended by
        //   `CString::new`.
        // - `max_path` is the length of `path` in bytes including the null terminator.
        //   It is always less than or equal to IPC_PORT_PATH_MAX.
        // - `flags` contains a flag value accepted by the callee
        // - `chandle_ptr` points to memory that the kernel can store a pointer into
        //   after the callee returns.
        let ret = unsafe {
            ipc_port_connect_async(
                &zero_uuid,
                port_name.as_ptr(),
                port_name.count_bytes() + 1, /* count_bytes excludes null-byte */
                IPC_CONNECT_WAIT_FOR_PORT,
                &mut (*c.href.as_mut_ptr()).handle,
            )
        };
        if ret != 0 {
            warn!(
                "failed to connect to {}, remote {}, connect err {ret}",
                c.tipc_port_name(),
                c.peer.port
            )
        }

        debug!("wait for connection to {}, remote {}", c.tipc_port_name(), c.peer.port);

        c.state = VsockConnectionState::TipcConnecting;

        // We cannot use the address of the connection as the cookie as it may move.
        // Use the heap address of the `handle_ref` instead as it will not get moved.
        let cookie = c.href.as_mut_ptr() as *mut c_void;
        c.href.set_cookie(cookie);
        c.href.set_emask(!0);
        c.href.set_id(c.peer.port);

        self.handle_set.attach(&mut c.href).map_err(|e| {
            c.href.handle_close();
            Error::Lk(e)
        })
    }

    fn vsock_tx_tipc_ready(&self, c: &mut VsockConnection) {
        if c.state != VsockConnectionState::TipcConnecting {
            panic!("warning, got poll ready in unexpected state: {:?}", c.state);
        }
        info!("connected to {}, remote {:?}", c.tipc_port_name(), c.peer.port);
        c.state = VsockConnectionState::Active;

        let buffer = [0u8];
        let res = self.connection_manager.lock().send(c.peer, c.local_port, &buffer);
        if res.is_err() {
            warn!("failed to send connected status message");
        }
    }

    fn vsock_rx_channel(
        &self,
        c: &mut VsockConnection,
        length: usize,
        source: VsockAddr,
        destination: VsockAddr,
    ) -> Result<(), Error> {
        assert_eq!(c.state, VsockConnectionState::Active);

        // multiple messages may be available when we call recv but we want to forward
        // them on the tipc connection one by one. Pass a slice of the rx_buffer so
        // we only drain the number of bytes that correspond to a single vsock event.
        c.rx_pending = self
            .connection_manager
            .lock()
            .deref_mut()
            .recv(source, destination.port, &mut c.rx_buffer[..length])
            .unwrap();

        // TODO: handle large messages properly
        assert_eq!(c.rx_pending, length);

        c.rx_count += 1;
        c.rx_since_tx += 1;

        c.tipc_try_send()?;

        self.connection_manager.lock().deref_mut().update_credit(c.peer, c.local_port).unwrap();

        Ok(())
    }

    fn vsock_send_reset(&self, peer: VsockAddr, local_port: u32) {
        let _ = self.connection_manager.lock().deref_mut().force_close(peer, local_port);
    }

    fn print_stats(&self) {
        let guard = self.connections.lock();
        let connections = guard.deref();
        for connection in connections {
            connection.print_stats();
        }
    }
}

pub(crate) fn vsock_rx_loop<H, T>(device: Arc<VsockDevice<H, T>>) -> Result<(), Error>
where
    H: Hal,
    T: Transport,
{
    let ten_ms = Duration::from_millis(10);
    let mut pending: Vec<VsockEvent> = vec![];

    debug!("starting vsock_rx_loop");

    // Accept connections on port zero and each name port in the port map
    {
        let mut connection_manager_guard = device.connection_manager.lock();
        let connection_manager = connection_manager_guard.deref_mut();

        for port in 0..PORT_MAP.len() as u32 {
            connection_manager.listen(port);
        }
    }

    loop {
        // TODO: use interrupts instead of polling
        // TODO: handle case where poll returns SocketError::OutputBufferTooShort
        let event = pending
            .pop()
            .or_else(|| device.connection_manager.lock().deref_mut().poll().expect("poll failed"));

        if event.is_none() {
            sleep(ten_ms);
            continue;
        }

        let VsockEvent { source, destination, event_type, buffer_status } = event.unwrap();

        match event_type {
            VsockEventType::ConnectionRequest => {
                if let Err(e) = device.vsock_rx_op_request(source, destination) {
                    error!("error during vsock connection request: {e:?}");
                    device.vsock_send_reset(source, destination.port);
                }
            }
            VsockEventType::Connected => {
                panic!("outbound connections not supported");
            }
            VsockEventType::Received { length } => {
                debug!("recv destination: {destination:?}");

                let connections = &mut *device.connections.lock();
                let lp = destination.port;
                let _ = vsock_connection_lookup_peer(connections, source, lp, |mut connection| {
                    if let Err(e) = match connection {
                        ref mut c @ VsockConnection {
                            state: VsockConnectionState::VsockOnly, ..
                        } => device.vsock_connect_on_rx(c, length, source, destination),
                        ref mut c @ VsockConnection {
                            state: VsockConnectionState::Active, ..
                        } => device.vsock_rx_channel(c, length, source, destination),
                        VsockConnection {
                            state: VsockConnectionState::TipcSendBlocked, ..
                        } => {
                            // requeue pending event.
                            pending.push(VsockEvent {
                                source,
                                destination,
                                event_type,
                                buffer_status,
                            });
                            // TODO: on one hand, we want to wait for the tipc connection to unblock
                            // on the other, we want to pick up incoming events as soon as we can...
                            // NOTE: Adding support for interrupts means we no longer have to sleep.
                            sleep(ten_ms);
                            Ok(())
                        }
                        VsockConnection { state: VsockConnectionState::TipcConnecting, .. } => {
                            warn!("got data while still waiting for tipc connection");
                            Err(LkError::ERR_BAD_STATE.into())
                        }
                        VsockConnection { state: s, .. } => {
                            error!("got data for connection in state {s:?}");
                            Err(LkError::ERR_BAD_STATE.into())
                        }
                    } {
                        error!("failed to receive data from vsock connection:  {e:?}");
                        device.vsock_send_reset(connection.peer, connection.local_port);

                        return ConnectionStateAction::Remove;
                    }
                    ConnectionStateAction::None
                })
                .inspect_err(|_| {
                    warn!("got packet for unknown connection");
                });
            }
            VsockEventType::Disconnected { reason } => {
                debug!("disconnected from peer. reason: {reason:?}");
                let connections = &mut *device.connections.lock();
                let lp = destination.port;
                let _ = vsock_connection_lookup_peer(connections, source, lp, |_connection| {
                    ConnectionStateAction::Remove
                })
                .inspect_err(|_| {
                    warn!("got disconnect ({reason:?}) for unknown connection");
                });
            }
            VsockEventType::CreditUpdate => { /* nothing to do */ }
            VsockEventType::CreditRequest => {
                // Polling the VsockConnectionManager won't return this event type
                panic!("don't know how to handle credit requests");
            }
        }
    }
}

pub(crate) fn vsock_tx_loop<H, T>(device: Arc<VsockDevice<H, T>>) -> Result<(), Error>
where
    H: Hal,
    T: Transport,
{
    let mut timeout = Duration::MAX;
    let ten_secs = Duration::from_secs(10);
    let mut tx_buffer = vec![0u8; PAGE_SIZE].into_boxed_slice();
    loop {
        let mut href = HandleRef::default();
        let mut ret = device.handle_set.handle_set_wait(&mut href, timeout);
        if ret == Err(LkError::ERR_NOT_FOUND) {
            // handle_set_wait returns ERR_NOT_FOUND if the handle_set is empty
            // but we can wait for it to become non-empty using handle_wait.
            // Once that that returns we have to call handle_set_wait again to
            // get the event we care about.
            ret = device.handle_set.handle_wait(&mut href.emask(), timeout);
            if ret != Err(LkError::ERR_TIMED_OUT) {
                info!("handle_wait on handle set returned: {ret:?}");
                continue;
            }
            // fall through to ret == ERR_TIMED_OUT case, then continue
        }
        if ret == Err(LkError::ERR_TIMED_OUT) {
            info!("tx inactive for {timeout:?} ms");
            timeout = Duration::MAX;
            device.print_stats();
            continue;
        }
        if ret.is_err() {
            warn!("handle_set_wait failed: {}", ret.unwrap_err());
            thread::sleep(ten_secs);
            continue;
        }

        let cookie = href.cookie();
        let _ = vsock_connection_lookup_cookie(&mut device.connections.lock(), cookie, |c| {
            if href.id() != c.href.id() {
                panic!(
                    "unexpected id {:?} != {:?} for connection {}",
                    href.id(),
                    c.href.id(),
                    c.tipc_port_name()
                );
            }

            if href.emask() & IPC_HANDLE_POLL_READY != 0 {
                device.vsock_tx_tipc_ready(c);
            }
            if href.emask() & IPC_HANDLE_POLL_MSG != 0 {
                // Print stats if we don't send any more packets for a while
                timeout = ACTIVE_TIMEOUT;
                // TODO: loop and read all messages?
                let mut msg_info = ipc_msg_info::default();

                // TODO: add more idiomatic Rust interface
                // Safety:
                // `c.href.handle` is a valid handle to a tipc channel.
                // `ipc_get_msg` can store a message descriptor in `msg_info`.
                let ret = unsafe { ipc_get_msg(c.href.handle(), &mut msg_info) };
                if ret == rust_support::Error::NO_ERROR.into() {
                    let mut iov: iovec_kern = tx_buffer.as_mut().into();
                    let mut msg = ipc_msg_kern::new(&mut iov);

                    // Safety:
                    // `c.href.handle` is a valid handle to a tipc channel.
                    // `msg_info` holds the results of a successful call to `ipc_get_msg`
                    // using the same handle.
                    let ret = unsafe { ipc_read_msg(c.href.handle(), msg_info.id, 0, &mut msg) };

                    // Safety:
                    // `ipc_put_msg` was called with the same handle and msg_info arguments.
                    unsafe { ipc_put_msg(c.href.handle(), msg_info.id) };
                    if ret >= 0 && ret as usize == msg_info.len {
                        c.tx_count += 1;
                        c.tx_since_rx += 1;
                        c.rx_since_tx = 0;
                        match device.connection_manager.lock().send(
                            c.peer,
                            c.local_port,
                            &tx_buffer[..msg_info.len],
                        ) {
                            Err(err) => {
                                if err == VirtioError::SocketDeviceError(SocketError::NotConnected)
                                {
                                    debug!(
                                        "failed to send {} bytes from {}. Connection closed",
                                        msg_info.len,
                                        c.tipc_port_name()
                                    );
                                } else {
                                    // TODO: close connection instead
                                    panic!(
                                        "failed to send {} bytes from {}: {:?}",
                                        msg_info.len,
                                        c.tipc_port_name(),
                                        err
                                    );
                                }
                            }
                            Ok(_) => {
                                debug!("sent {} bytes from {}", msg_info.len, c.tipc_port_name());
                            }
                        }
                    } else {
                        error!("ipc_read_msg failed: {ret}");
                    }
                }
            }
            if href.emask() & IPC_HANDLE_POLL_SEND_UNBLOCKED != 0 {
                assert_eq!(c.state, VsockConnectionState::TipcSendBlocked);
                assert_ne!(c.rx_pending, 0);

                debug!("tipc connection unblocked {}", c.tipc_port_name());

                if let Err(e) = c.tipc_try_send() {
                    error!("failed to send pending message to {}: {e:?}", c.tipc_port_name());
                }
            }
            if href.emask() & IPC_HANDLE_POLL_HUP != 0 {
                // Print stats if we don't send any more packets for a while
                timeout = ACTIVE_TIMEOUT;
                info!("got hup");
                debug!(
                    "shut down connection {}, {:?}, {:?}",
                    c.tipc_port_name(),
                    c.peer,
                    c.local_port
                );
                let res = device.connection_manager.lock().shutdown(c.peer, c.local_port);
                if res.is_ok() {
                    return ConnectionStateAction::Close;
                } else {
                    warn!(
                        "failed to send shutdown command, connection removed? {}",
                        res.unwrap_err()
                    );
                }
            }
            ConnectionStateAction::None
        })
        .inspect_err(|_| {
            warn!("got event for non-existent remote {}, was it closed?", href.id());
        });
        href.handle_decref();
    }
}

pub(crate) fn vsock_init<T: Transport + 'static + Send, H: Hal + 'static>(
    driver: VirtIOSocket<H, T, 4096>,
) -> Result<(), Error> {
    let manager = VsockConnectionManager::new_with_capacity(driver, 4096);
    let device_for_rx = Arc::new(VsockDevice::new(manager));
    let device_for_tx = device_for_rx.clone();

    // In some builds, stack overflows can occur on both threads when using 4k stacks
    let stack_size = 8192usize;
    Builder::new()
        .name(c"virtio_vsock_rx")
        .priority(Priority::HIGH)
        .stack_size(stack_size)
        .spawn(move || {
            let ret = vsock_rx_loop(device_for_rx);
            error!("vsock_rx_loop returned {:?}", ret);
            ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
        })
        .map_err(|e| LkError::from_lk(e).unwrap_err())?;

    Builder::new()
        .name(c"virtio_vsock_tx")
        .priority(Priority::HIGH)
        .stack_size(stack_size)
        .spawn(move || {
            let ret = vsock_tx_loop(device_for_tx);
            error!("vsock_tx_loop returned {:?}", ret);
            ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
        })
        .map_err(|e| LkError::from_lk(e).unwrap_err())?;

    Ok(())
}
