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
use core::ops::Deref;
use core::ops::DerefMut;
use core::ptr::eq;
use core::ptr::null_mut;
use core::time::Duration;

use alloc::boxed::Box;
use alloc::ffi::CString;
use alloc::format;
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
use virtio_drivers::device::socket::VsockAddr;
use virtio_drivers::device::socket::VsockConnectionManager;
use virtio_drivers::device::socket::VsockEvent;
use virtio_drivers::device::socket::VsockEventType;
use virtio_drivers::transport::Transport;
use virtio_drivers::Hal;
use virtio_drivers::PAGE_SIZE;

use rust_support::handle::HandleRef;
use rust_support::handle_set::HandleSet;

use rust_support::Error as LkError;

use crate::err::Error;

const ACTIVE_TIMEOUT: Duration = Duration::from_secs(5);

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum VsockConnectionState {
    #[default]
    Invalid = 0,
    VsockOnly,
    TipcOnly,
    TipcConnecting,
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
}

impl VsockConnection {
    fn new(peer: VsockAddr, local_port: u32) -> Self {
        Self {
            peer,
            local_port,
            state: VsockConnectionState::VsockOnly,
            tipc_port_name: None,
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
}

fn vsock_connection_lookup(
    connections: &mut Vec<VsockConnection>,
    remote_port: u32,
) -> Option<(usize, &mut VsockConnection)> {
    connections
        .iter_mut()
        .enumerate()
        .find(|(_idx, connection)| connection.peer.port == remote_port)
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

    fn vsock_rx_op_request(&self, peer: VsockAddr, local: VsockAddr) {
        debug!("dst_port {}, src_port {}", local.port, peer.port);

        // do we already have a connection?
        let mut guard = self.connections.lock();
        if let Some(_) = guard
            .deref()
            .iter()
            .find(|connection| connection.peer == peer && connection.local_port == local.port)
        {
            panic!("connection already exists");
        };

        guard.deref_mut().push(VsockConnection::new(peer, local.port));
    }

    fn vsock_connect_tipc(
        &self,
        c: &mut VsockConnection,
        length: usize,
        source: VsockAddr,
        destination: VsockAddr,
    ) -> Result<(), Error> {
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
        if buffer[data_len - 1] == '\n' as _ {
            data_len -= 1;
        }
        let port_name = &buffer[0..data_len];
        // should not contain any null bytes
        c.tipc_port_name = CString::new(port_name).ok();

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
                c.tipc_port_name.as_ref().unwrap().as_ptr(),
                data_len + /* null byte added by CString::new */ 1,
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

        info!("wait for connection to {}, remote {}", c.tipc_port_name(), c.peer.port);

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
        rx_buffer: &mut Box<[u8]>,
    ) -> Result<(), Error> {
        assert!(length <= rx_buffer.len());
        let data_len = self
            .connection_manager
            .lock()
            .deref_mut()
            .recv(source, destination.port, rx_buffer)
            .unwrap();

        // TODO: handle large messages properly
        assert!(data_len == length);

        let mut iov = iovec_kern { iov_base: rx_buffer.as_mut_ptr() as _, iov_len: data_len };
        let mut msg = ipc_msg_kern::new(&mut iov);

        c.rx_count += 1;
        c.rx_since_tx += 1;
        c.tx_since_rx = 0;
        // Safety:
        // `c.href.handle` is a handle attached to a tipc channel.
        // `msg` contains an `iov` which points to a buffer from which
        // the kernel can read `iov_len` bytes.
        let ret = unsafe { ipc_send_msg(c.href.handle(), &mut msg) };
        if ret < 0 {
            error!("failed to send {length} bytes to {}: {ret} ", c.tipc_port_name());
            LkError::from_lk(ret)?;
        }
        if ret as usize != length {
            error!("sent {ret} bytes but expected to send {length} bytes");
            Err(LkError::ERR_IO)?;
        }

        debug!("sent {length} bytes to {}", c.tipc_port_name());
        self.connection_manager.lock().deref_mut().update_credit(c.peer, c.local_port).unwrap();

        Ok(())
    }

    fn vsock_connection_close(&self, c: &mut VsockConnection, vsock_done: bool) -> bool {
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
        {
            // The handle set owns the only reference we have to the handle and
            // handle_set_wait might have already returned a pointer to c
            c.href.detach();
            c.href.handle_close();
            c.href.set_cookie(null_mut());
            info!("tipc handle closed");
            c.state = VsockConnectionState::TipcClosed;
        }
        if vsock_done && c.state == VsockConnectionState::TipcClosed {
            info!("vsock closed");
            c.state = VsockConnectionState::Closed;
        }
        if c.state == VsockConnectionState::Closed && c.href.cookie().is_null() {
            info!("remove connection");
            c.print_stats();
            return true; // remove connection
        }
        return false; // keep connection
    }

    fn print_stats(&self) {
        let guard = self.connections.lock();
        let connections = guard.deref();
        for connection in connections {
            connection.print_stats();
        }
    }
}

// Safety: each field of a `VsockDevice` is safe to transfer across thread boundaries
// TODO: remove this once https://github.com/rcore-os/virtio-drivers/pull/146 lands
unsafe impl<H, T> Send for VsockDevice<H, T>
where
    H: Hal,
    T: Transport,
{
}

// Safety: each field of a `VsockDevice` is safe to share between threads
// TODO: remove this once https://github.com/rcore-os/virtio-drivers/pull/146 lands
unsafe impl<H, T> Sync for VsockDevice<H, T>
where
    H: Hal,
    T: Transport,
{
}

pub(crate) fn vsock_rx_loop<H, T>(device: Arc<VsockDevice<H, T>>) -> Result<(), Error>
where
    H: Hal,
    T: Transport,
{
    let local_port = 1;
    let ten_ms = Duration::from_millis(10);
    let mut rx_buffer = vec![0u8; PAGE_SIZE].into_boxed_slice();

    debug!("starting vsock_rx_loop");
    device.connection_manager.lock().deref_mut().listen(local_port);

    loop {
        // TODO: use interrupts instead of polling
        let event = device.connection_manager.lock().deref_mut().poll()?;
        if let Some(VsockEvent { source, destination, event_type, .. }) = event {
            match event_type {
                VsockEventType::ConnectionRequest => {
                    device.vsock_rx_op_request(source, destination);
                }
                VsockEventType::Connected => {
                    panic!("outbound connections not supported");
                }
                VsockEventType::Received { length } => {
                    debug!("recv destination: {destination:?}");

                    let mut guard = device.connections.lock();
                    if let Some((conn_idx, mut connection)) =
                        vsock_connection_lookup(guard.deref_mut(), source.port)
                    {
                        if let Err(e) = match connection {
                            ref mut c @ VsockConnection {
                                state: VsockConnectionState::VsockOnly,
                                ..
                            } => device.vsock_connect_tipc(c, length, source, destination),
                            ref mut c @ VsockConnection {
                                state: VsockConnectionState::Active,
                                ..
                            } => device.vsock_rx_channel(
                                *c,
                                length,
                                source,
                                destination,
                                &mut rx_buffer,
                            ),
                            VsockConnection {
                                state: VsockConnectionState::TipcConnecting, ..
                            } => {
                                warn!("got data while still waiting for tipc connection");
                                Err(LkError::ERR_BAD_STATE.into())
                            }
                            VsockConnection { state: s, .. } => {
                                error!("got data for connection in state {s:?}");
                                Err(LkError::ERR_BAD_STATE.into())
                            }
                        } {
                            error!("failed to receive data from vsock connection:  {e:?}");
                            // TODO: add reset function to device or connection?
                            let _ = device
                                .connection_manager
                                .lock()
                                .deref_mut()
                                .force_close(connection.peer, connection.local_port);

                            if device.vsock_connection_close(connection, true) {
                                // TODO: find a proper way to satisfy the borrow checker
                                guard.deref_mut().swap_remove(conn_idx);
                            }
                        }
                    } else {
                        warn!("got packet for unknown connection");
                    }
                }
                VsockEventType::Disconnected { reason } => {
                    debug!("disconnected from peer. reason: {reason:?}");
                    let mut guard = device.connections.lock();
                    let connections = guard.deref_mut();
                    if let Some((c_idx, c)) = vsock_connection_lookup(connections, source.port) {
                        let vsock_done = true;
                        if device.vsock_connection_close(c, vsock_done) {
                            // TODO: find a proper way to satisfy the borrow checker
                            connections.swap_remove(c_idx);
                        }
                    } else {
                        warn!("got disconnect ({reason:?}) for unknown connection");
                    }
                }
                VsockEventType::CreditUpdate => { /* nothing to do */ }
                VsockEventType::CreditRequest => {
                    // Polling the VsockConnectionManager won't return this event type
                    panic!("don't know how to handle credit requests");
                }
            }
        } else {
            sleep(ten_ms);
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
            info!("handle_set_wait failed: {}", ret.unwrap_err());
            ret = device.handle_set.handle_wait(&mut href.emask(), timeout);
            if ret != Err(LkError::ERR_TIMED_OUT.into()) {
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

        let mut guard = device.connections.lock();
        let connections = guard.deref_mut();
        if let Some((_, c)) = vsock_connection_lookup(connections, href.id()) {
            if !eq(c.href.as_mut_ptr() as *mut c_void, href.cookie()) {
                panic!(
                    "unexpected cookie {:?} != {:?} for connection {}",
                    href.cookie(),
                    c.href.as_mut_ptr(),
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
                        device
                            .connection_manager
                            .lock()
                            .send(c.peer, c.local_port, &tx_buffer[..msg_info.len])
                            .expect(&format!("failed to send message from {}", c.tipc_port_name()));
                        debug!("sent {} bytes from {}", msg_info.len, c.tipc_port_name());
                    } else {
                        error!("ipc_read_msg failed: {ret}");
                    }
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
                device.connection_manager.lock().shutdown(c.peer, c.local_port)?;
                device.vsock_connection_close(c, /* vsock_done */ false);
            }
        }
        drop(guard);
        href.handle_decref();
    }
}
