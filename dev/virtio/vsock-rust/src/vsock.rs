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
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;
use core::time::Duration;

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::ffi::CString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use log::debug;
use log::error;
use log::info;
use log::warn;

use peer_id::Uuid;
use rand::rand_get_bytes;
use rust_support::event::Event;
use rust_support::event::EVENT_FLAG_AUTOUNSIGNAL;
use rust_support::handle::IPC_HANDLE_POLL_HUP;
use rust_support::handle::IPC_HANDLE_POLL_MSG;
use rust_support::handle::IPC_HANDLE_POLL_READY;
use rust_support::handle::IPC_HANDLE_POLL_SEND_UNBLOCKED;
use rust_support::ipc::iovec_kern;
use rust_support::ipc::ipc_get_msg;
use rust_support::ipc::ipc_msg_info;
use rust_support::ipc::ipc_msg_kern;
use rust_support::ipc::ipc_port_accept;
use rust_support::ipc::ipc_port_connect_async;
use rust_support::ipc::ipc_port_create;
use rust_support::ipc::ipc_port_publish;
use rust_support::ipc::ipc_put_msg;
use rust_support::ipc::ipc_read_msg;
use rust_support::ipc::ipc_send_msg;
use rust_support::ipc::IPC_CONNECT_WAIT_FOR_PORT;
use rust_support::ipc::IPC_PORT_ALLOW_TA_CONNECT;
use rust_support::ipc::IPC_PORT_PATH_MAX;
use rust_support::sync::Mutex;
use rust_support::thread;
use rust_support::thread::sleep;
use rust_support::thread::Builder;
use rust_support::thread::Priority;
use trusty::EventClient;
use virtio_drivers_and_devices::device::socket::SocketError;
use virtio_drivers_and_devices::device::socket::VirtIOSocket;
use virtio_drivers_and_devices::device::socket::VsockAddr;
use virtio_drivers_and_devices::device::socket::VsockConnectionManager;
use virtio_drivers_and_devices::device::socket::VsockEvent;
use virtio_drivers_and_devices::device::socket::VsockEventType;
use virtio_drivers_and_devices::device::socket::VsockManager;
use virtio_drivers_and_devices::transport::Transport;
use virtio_drivers_and_devices::Error as VirtioError;
use virtio_drivers_and_devices::Hal;
use virtio_drivers_and_devices::PAGE_SIZE;

use rust_support::handle::HandleRef;
use rust_support::handle_set::HandleSet;

use rust_support::Error as LkError;

use crate::err::Error;
use crate::FFAClientId;

#[cfg(feature = "virtio_msg_device")]
use sm::VmRef;
// For non-TZ builds we can't import sm::VmRef since lib/sm and its rust bindings are not supported.
// Since vsock_{rx,tx}_loop take an Option<VmRef> arg and non-TZ builds always pass in None we
// redefine it as an arbitrary type for those builds.
#[cfg(not(feature = "virtio_msg_device"))]
pub(crate) struct VmRef;

const ACTIVE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
struct TipcPort {
    port: u32,
    name: &'static CStr,
}

const PORT_MAP: &[TipcPort] = &[
    // Reserved privileged ports
    // connections on port zero must send port name in first packet
    TipcPort { port: 0, name: c"" },
    // Privileged ports
    #[cfg(feature = "authmgr")]
    TipcPort { port: 1, name: c"ahss.authmgr.IAuthMgrAuthorization/default.bnd" },
    #[cfg(feature = "widevine_aidl_comm")]
    TipcPort { port: 6, name: c"com.android.trusty.widevine.transact" },
    #[cfg(feature = "gatekeeper")]
    TipcPort { port: 8, name: c"com.android.trusty.gatekeeper" },
    #[cfg(feature = "keymint")]
    TipcPort { port: 9, name: c"com.android.trusty.keymint" },
    #[cfg(feature = "vintf_ta")]
    TipcPort { port: 10, name: c"com.android.trusty.vintf" },
    #[cfg(feature = "keymint_commservice")]
    TipcPort { port: 11, name: c"com.android.trusty.keymint.commservice" },
];

/// Finds the TIPC name associated with a given vsock port number.
fn get_port_name(port: u32) -> Option<&'static CStr> {
    PORT_MAP.iter().find(|entry| entry.port == port).map(|entry| entry.name)
}

// Different targets may support different vsock transports so we need this attribute to avoid
// breaking the build for targets that only construct a subset of them.
#[allow(dead_code)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum TransportKind {
    DriverFFAMsg(FFAClientId),
    DeviceFFAMsg(FFAClientId),
    DriverPCI,
}

struct TipcToVsockMapping {
    /// Local port name to listen on.
    name: &'static CStr,

    /// Kind of transport used to connect to the destination.
    transport_kind: TransportKind,

    /// Destination address to connect to.
    addr: VsockAddr,

    /// List of allowed UUIDs that can connect to this port.
    /// All clients are allowed if this list is empty.
    allowed_uuids: &'static [Uuid],
}

// TODO (b/433489263): get this from FFA_PARTITION_INFO_GET
#[allow(dead_code)]
const TRUSTY_SP_ID: u16 = 0x8001u16;

const TIPC_TO_VSOCK_MAPPINGS: &[TipcToVsockMapping] = &[
    #[cfg(feature = "tipc_vsock_forwarder")]
    TipcToVsockMapping {
        name: c"com.android.trusty.vsock.forwarder",
        transport_kind: TransportKind::DriverFFAMsg(TRUSTY_SP_ID),
        addr: VsockAddr { cid: 2, port: 0 },
        allowed_uuids: &[],
    },
    #[cfg(feature = "tipc_vsock_authmgr")]
    TipcToVsockMapping {
        name: c"ahss.authmgr.IAuthMgrAuthorization/default.bnd",
        transport_kind: TransportKind::DriverFFAMsg(TRUSTY_SP_ID),
        addr: VsockAddr { cid: 2, port: 1 },
        allowed_uuids: &[
            Uuid::new(
                // trusty/user/app/authmgr/authmgr-fe/app/manifest.json
                0x9b3c1e9e,
                0x1808,
                0x4b98,
                [0x8f, 0xa9, 0x85, 0x92, 0xdf, 0xf3, 0xa3, 0x37],
            ),
            Uuid::new(
                // trusty/user/app/authmgr/authmgr-be/lib/manifest.json
                0x1c966e25,
                0x7729,
                0x4122,
                [0x8f, 0xb6, 0xcc, 0xd2, 0xb6, 0x12, 0x43, 0x0c],
            ),
            Uuid::new(
                // trusty/user/app/sample/vintf/app/manifest.json
                0xd2d10228,
                0x107c,
                0x4f7b,
                [0x9c, 0x52, 0x86, 0xdc, 0xe8, 0x00, 0x70, 0x49],
            ),
        ],
    },
];

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
    href: HandleRef<c_void>,
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

fn vsock_connection_close_all(connections: &mut Vec<VsockConnection>) {
    for mut c in connections.drain(..) {
        vsock_connection_close(&mut c, ConnectionStateAction::Remove);
    }
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
        || c.state == VsockConnectionState::TipcOnly
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

// Bitflags for the VsockRxEvent wake_reason field. Enum variants must be smaller than 32 since this
// field is an AtomicU32
#[repr(u32)]
enum WakeReasonFlag {
    Terminate = 0,
}

pub(crate) struct VsockRxEvent {
    event: Event,
    wake_reason: AtomicU32,
}

impl VsockRxEvent {
    const NONE: u32 = 0;
    const TERMINATE: u32 = 1 << WakeReasonFlag::Terminate as u32;

    #[allow(dead_code)]
    pub(crate) fn signal_stop(&self) {
        self.wake_reason.fetch_or(Self::TERMINATE, Ordering::Relaxed);
        self.event.signal();
    }
}

pub struct VsockDevice<M>
where
    M: VsockManager,
{
    connections: Mutex<Vec<VsockConnection>>,
    handle_set: HandleSet<c_void>,
    connection_manager: Mutex<M>,
    vsock_drop: Arc<Event>,
    rx_event: Arc<VsockRxEvent>,
}

impl<M> VsockDevice<M>
where
    M: VsockManager,
{
    pub(crate) fn new(manager: M) -> Self {
        let rx_event = VsockRxEvent {
            event: Event::new(false, EVENT_FLAG_AUTOUNSIGNAL),
            wake_reason: AtomicU32::new(VsockRxEvent::NONE),
        };
        Self {
            connections: Mutex::new(Vec::new()),
            handle_set: HandleSet::new(),
            connection_manager: Mutex::new(manager),
            vsock_drop: Arc::new(Event::new(false, EVENT_FLAG_AUTOUNSIGNAL)),
            rx_event: Arc::new(rx_event),
        }
    }

    // Some builds may not need to get an event to wait for the VsockDevice to drop (e.g. in cases
    // like pVM builds where we know the peer will always be there). We allow this function to
    // remain unused in those builds instead of gating to avoid needing to gate individual imports
    // as well.
    #[allow(dead_code)]
    pub(crate) fn get_vsock_drop_event(&self) -> Arc<Event> {
        self.vsock_drop.clone()
    }

    #[allow(dead_code)]
    pub(crate) fn get_vsock_rx_event(&self) -> Arc<VsockRxEvent> {
        self.rx_event.clone()
    }

    fn port_is_listening(&self, port: u32) -> bool {
        get_port_name(port).is_some()
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
        let port_name = get_port_name(local.port).ok_or(LkError::ERR_OUT_OF_RANGE)?;
        if port_name != c"" {
            c.tipc_port_name = Some(port_name.to_owned());
            self.vsock_connect_tipc(&mut c)?;
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
        assert!(get_port_name(destination.port) == Some(c""));

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
        info!("port_name is {port_name:?}");

        // should not contain any null bytes
        c.tipc_port_name = CString::new(port_name).ok();
        info!("tipc port name set to {}", c.tipc_port_name());

        self.vsock_connect_tipc(c)
    }

    fn create_tipc_ports(
        &self,
        transport_kind: TransportKind,
    ) -> [HandleRef<c_void>; TIPC_TO_VSOCK_MAPPINGS.len()] {
        let mut port_hrefs: [HandleRef<c_void>; TIPC_TO_VSOCK_MAPPINGS.len()] = Default::default();
        for (port, phref) in TIPC_TO_VSOCK_MAPPINGS.iter().zip(port_hrefs.iter_mut()) {
            if port.transport_kind != transport_kind {
                continue;
            }

            // Safety:
            // - `sid` is a valid uuid with static lifetime
            // - `path` points to a null-terminated C-string. The null byte was appended by
            //   `CString::new`.
            // - `num_recv_bufs` is a primitive value.
            // - `recv_buf_size` is a primitive value.
            // - `flags` contains a flag value accepted by the callee
            // - `phandle_ptr` points to memory that the kernel can store a pointer into
            //   after the callee returns.
            let ret = unsafe {
                ipc_port_create(
                    Uuid::zero(),
                    port.name.as_ptr(),
                    1,
                    PAGE_SIZE,
                    IPC_PORT_ALLOW_TA_CONNECT,
                    &raw mut (*phref.as_mut_ptr()).handle,
                )
            };
            if ret != 0 {
                warn!("failed to create {:?}, remote {:?}, err {ret}", port.name, port.addr);
                continue;
            }

            // Safety:
            // - `phandle` is a valid port handle from ipc_port_create
            let ret = unsafe { ipc_port_publish(phref.handle()) };
            if ret != 0 {
                warn!("failed to publish {:?}, remote {:?}, err {ret}", port.name, port.addr);
                phref.handle_close();
                continue;
            }

            phref.set_emask(!0);
            if let Err(e) = self.handle_set.attach(phref) {
                warn!("failed to attach port {:?}, remote {:?}, err {e}", port.name, port.addr);
                phref.handle_close();
                continue;
            };

            debug!("tipc to vsock mapping enabled on port {:?}", port.name);
        }

        port_hrefs
    }

    fn tipc_connect_vsock(
        &self,
        port: &TipcToVsockMapping,
        href: &mut HandleRef<c_void>,
    ) -> Result<VsockConnection, Error> {
        debug!("got tipc connection on {:?}", port.name);

        // Pick a random unused 32-bit source port; the probability
        // of collision should be pretty low if we pick randomly.
        let mut cm = self.connection_manager.lock();
        let mut src_port;
        loop {
            let mut src_port_bytes = [0; 4];
            rand_get_bytes(&mut src_port_bytes[..]);
            src_port = u32::from_ne_bytes(src_port_bytes);
            if self.port_is_listening(src_port) {
                // Don't use listening port numbers for outgoing connections
                // to avoid conflicts with incoming connections.
                continue;
            }

            match cm.connect(port.addr, src_port) {
                Ok(()) => break,
                Err(VirtioError::SocketDeviceError(SocketError::ConnectionExists)) => continue,
                Err(e) => return Err(Error::Virtio(e)),
            }
        }

        let mut c = VsockConnection::new(port.addr, src_port);
        c.tipc_port_name = Some(port.name.to_owned());
        c.state = VsockConnectionState::TipcOnly;

        let mut peer_uuid_ptr = core::ptr::null();
        // Safety:
        // - `phandle` is a valid port from href
        // - `chandle` is a zeroed HandleRef from c
        // - `peer` is the zero-initialized pointer from above
        let ret = unsafe {
            ipc_port_accept(
                href.handle(),
                &raw mut (*c.href.as_mut_ptr()).handle,
                &raw mut peer_uuid_ptr,
            )
        };
        if ret < 0 {
            error!("failed to accept connection on {:?}: {ret} ", port.name);
            let _ = cm.force_close(c.peer, c.local_port);
            LkError::from_lk(ret)?;
        }

        debug_assert!(!peer_uuid_ptr.is_null());
        // Safety:
        //   Since `ipc_port_accept` returned without error, it has stored into `peer_uuid_ptr` a
        //   non-null pointer which is valid for reads of the type `uuid`.
        let peer_uuid = unsafe { *peer_uuid_ptr };
        if !port.allowed_uuids.is_empty() && !port.allowed_uuids.contains(&peer_uuid) {
            error!("client {:?} not allowed on {:?}: {ret} ", peer_uuid, port.name);
            c.href.handle_close();
            let _ = cm.force_close(c.peer, c.local_port);
            return Err(LkError::ERR_NOT_ALLOWED.into());
        }

        // Initialize the cookie here so vsock_connection_lookup_cookie works
        // correctly past this point. See comment in vsock_connect_tipc w.r.t.
        // the choice of what to use as the cookie.
        let cookie = c.href.as_mut_ptr() as *mut c_void;
        c.href.set_cookie(cookie);
        c.href.set_emask(!0);
        c.href.set_id(c.peer.port);

        debug!("accepted tipc connection on {:?}", port.name);

        Ok(c)
    }

    fn vsock_connect_tipc(&self, c: &mut VsockConnection) -> Result<(), Error> {
        let port_name = c.tipc_port_name.as_ref().expect("tipc port name has been set");
        // invariant: port_name.count_bytes() + 1 <= IPC_PORT_PATH_MAX
        debug_assert!(port_name.count_bytes() < IPC_PORT_PATH_MAX as usize);

        // Safety:
        // - `sid`` is a valid uuid with static lifetime
        // - `path` points to a null-terminated C-string. The null byte was appended by
        //   `CString::new`.
        // - `max_path` is the length of `path` in bytes including the null terminator.
        //   It is always less than or equal to IPC_PORT_PATH_MAX.
        // - `flags` contains a flag value accepted by the callee
        // - `chandle_ptr` points to memory that the kernel can store a pointer into
        //   after the callee returns.
        let ret = unsafe {
            ipc_port_connect_async(
                Uuid::zero(),
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

// The unused VM ref argument is only passed in if the peer is a VM. When this function returns the
// VmRef gets dropped decreasing the VM's refcount by 1. The Arc<VsockDevice> also gets dropped. If
// it's the last refcount that triggers the VsockDevice Drop impl which signals the `vsock_drop`
// event.
pub(crate) fn vsock_rx_loop<M>(
    device: Arc<VsockDevice<M>>,
    _transport_kind: TransportKind,
    _vm_ref: Option<VmRef>,
) -> Result<(), Error>
where
    M: VsockManager,
{
    let ten_ms = Duration::from_millis(10);
    let mut pending: VecDeque<VsockEvent> = VecDeque::new();

    debug!("starting vsock_rx_loop");

    // Accept connections on port zero and each name port in the port map
    {
        let mut connection_manager_guard = device.connection_manager.lock();
        let connection_manager = connection_manager_guard.deref_mut();

        for entry in PORT_MAP {
            connection_manager.listen(entry.port);
        }
    }

    loop {
        // TODO: use interrupts instead of polling
        // TODO: handle case where poll returns SocketError::OutputBufferTooShort
        let event = pending
            .pop_front()
            .or_else(|| device.connection_manager.lock().deref_mut().poll().expect("poll failed"));

        if event.is_none() {
            let res = device.rx_event.event.wait_timeout(ten_ms);
            match res {
                Ok(()) => {
                    let wake_reason = device.rx_event.wake_reason.load(Ordering::Relaxed);
                    if (wake_reason & VsockRxEvent::TERMINATE) != 0 {
                        vsock_connection_close_all(&mut device.connections.lock());
                        return Ok(());
                    }
                }
                Err(LkError::ERR_TIMED_OUT) => (),
                Err(e) => {
                    unreachable!("failed to wait for rx loop event {e:?}")
                }
            }
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
                debug!("connected destination: {destination:?}");

                let connections = &mut *device.connections.lock();
                let lp = destination.port;
                let _ = vsock_connection_lookup_peer(connections, source, lp, |connection| {
                    debug_assert!(connection.state == VsockConnectionState::TipcOnly);

                    if let Err(e) = device.handle_set.attach(&mut connection.href) {
                        error!("failed to attach connection: {e:?}");
                        device.vsock_send_reset(connection.peer, connection.local_port);
                        return ConnectionStateAction::Remove;
                    }

                    connection.state = VsockConnectionState::Active;
                    ConnectionStateAction::None
                })
                .inspect_err(|_| {
                    warn!("got packet for unknown connection");
                });
            }
            VsockEventType::Received { length } => {
                debug!("recv destination: {destination:?}");

                let connections = &mut *device.connections.lock();
                let lp = destination.port;
                let _ = vsock_connection_lookup_peer(connections, source, lp, |connection| {
                    let res = match connection {
                        VsockConnection { state: VsockConnectionState::VsockOnly, .. } => {
                            device.vsock_connect_on_rx(connection, length, source, destination)
                        }
                        VsockConnection { state: VsockConnectionState::Active, .. } => {
                            device.vsock_rx_channel(connection, length, source, destination)
                        }
                        // We requeue a vsock event in these two connection states:
                        // 1. `TipcConnecting`: The underlying TIPC connection is not yet ready.
                        //    Requeuing the event here fixes a race condition (b/406418102) and
                        //    allows the client to use the standard vsock protocol which does not
                        //    include a way to tell the peer to retry the connection attempt. In the
                        //    case where the TIPC port name will be sent in the first message the
                        //    client should wait for the status byte so we send a reset if we get
                        //    data in this state.
                        // 2. `TipcSendBlocked`: The TIPC connection is ready but last attempt to
                        //    send data on the connection blocked due to lack of buffer space.
                        VsockConnection { state: VsockConnectionState::TipcConnecting, .. }
                        | VsockConnection {
                            state: VsockConnectionState::TipcSendBlocked, ..
                        } => {
                            // TODO (b/443749488): Make whether a port name is expected or not more
                            // explicit in the VsockConnectionState
                            let port_name_expected = get_port_name(lp) == Some(c"");
                            if connection.state == VsockConnectionState::TipcConnecting
                                && port_name_expected
                            {
                                warn!("got data while still waiting for tipc connection");
                                Err(LkError::ERR_BAD_STATE.into())
                            } else {
                                // requeue pending event.
                                pending.push_back(VsockEvent {
                                    source,
                                    destination,
                                    event_type,
                                    buffer_status,
                                });
                                // NOTE: on one hand, we want to wait for the tipc connection to become ready
                                // or unblocked. on the other, we want to pick up incoming events as soon as we
                                // can...
                                // TODO: We could wait on an event here rather than sleeping until tipc is ready.
                                sleep(ten_ms);
                                Ok(())
                            }
                        }
                        VsockConnection { state: s, .. } => {
                            error!("got data for connection in state {s:?}");
                            Err(LkError::ERR_BAD_STATE.into())
                        }
                    };
                    if let Err(e) = res {
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

// This function takes an evt_client if the peer is a VM which may be torn down. It waits on the
// event client's handle for a potential VM destruction event and once it receives it this thread
// just notifies the client source and returns. The unused VM ref is also only passed in if the peer
// is a VM. When this function returns the VmRef gets dropped decreasing the VM's refcount by 1.
// The Arc<VsockDevice> also gets dropped. If it's the last refcount that triggers the VsockDevice
// Drop impl which signals the `vsock_drop` event.
pub(crate) fn vsock_tx_loop<M>(
    device: Arc<VsockDevice<M>>,
    transport_kind: TransportKind,
    evt_client: Option<EventClient>,
    _vm_ref: Option<VmRef>,
) -> Result<(), Error>
where
    M: VsockManager,
{
    debug!("starting vsock_tx_loop");

    let mut port_hrefs = device.create_tipc_ports(transport_kind);
    let mut timeout = Duration::MAX;
    let ten_secs = Duration::from_secs(10);
    let mut tx_buffer = vec![0u8; PAGE_SIZE].into_boxed_slice();

    let _evt_href = match &evt_client {
        Some(evt_client) => {
            // SAFETY: The handle argument is in an EventClient so it must have been initialized by
            // a call to event_source_open which calls handle_init
            let mut evt_href = unsafe { HandleRef::new(evt_client.handle()) };
            evt_href.set_emask(!0);
            evt_href.set_id(0);
            evt_href.set_cookie(null_mut());
            device.handle_set.attach(&mut evt_href).unwrap();
            Some(evt_href)
        }
        None => None,
    };

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

        if let Some(ref evt_client) = &evt_client {
            if href.handle() == evt_client.handle() {
                debug!("stopping vsock tx loop");
                return Ok(());
            }
        };
        let connections = &mut *device.connections.lock();
        let cookie = href.cookie();
        let _ = vsock_connection_lookup_cookie(connections, cookie, |c| {
            if href.id() != c.href.id() {
                panic!(
                    "unexpected id {:?} != {:?} for connection {}",
                    href.id(),
                    c.href.id(),
                    c.tipc_port_name()
                );
            }

            if href.emask() & IPC_HANDLE_POLL_READY != 0 {
                assert_eq!(
                    c.state,
                    VsockConnectionState::TipcConnecting,
                    "got poll ready in unexpected state: {:?}",
                    c.state
                );
                info!("connected to {}, remote {:?}", c.tipc_port_name(), c.peer.port);
                c.state = VsockConnectionState::Active;

                // Send a status byte as the first message for ports that expect a TIPC port name in the
                // first message to signal a successful connection to the client.
                if get_port_name(c.local_port) == Some(c"") {
                    let buffer = [0u8];
                    let res = device.connection_manager.lock().send(c.peer, c.local_port, &buffer);
                    if res.is_err() {
                        warn!("failed to send connected status message");
                    }
                }
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
            if let Some(idx) =
                port_hrefs.iter_mut().position(|phref| phref.handle() == href.handle())
            {
                if href.emask() & IPC_HANDLE_POLL_READY != 0 {
                    match device.tipc_connect_vsock(&TIPC_TO_VSOCK_MAPPINGS[idx], &mut href) {
                        Ok(c) => connections.push(c),
                        Err(e) => error!("failed to accept tipc connection {e:?})"),
                    }
                } else if href.emask() != 0 {
                    warn!("unexpected port emask {:x}", href.emask());
                }
                return;
            }

            warn!("got event for non-existent remote {}, was it closed?", href.id());
        });
        // SAFETY: The refcount was incremented by the handle_set_wait or handle_wait
        unsafe { href.handle_decref() };
    }
}

impl<M: VsockManager> Drop for VsockDevice<M> {
    fn drop(&mut self) {
        debug!("dropped VsockDevice");
        // On trusty builds that do not grab another refcount to self.vsock_drop this will signal
        // the LK event then immediately free the VsockDevice, freeing the event and calling
        // event_destroy in the process. Builds that do grab another refcount signal it, free the
        // VsockDevice here but the keep the event around until all references are dropped.
        self.vsock_drop.signal();
    }
}

pub(crate) fn vsock_init<T: Transport + 'static + Send, H: Hal + 'static>(
    driver: VirtIOSocket<H, T, 4096>,
    transport_kind: TransportKind,
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
            let ret = vsock_rx_loop(device_for_rx, transport_kind, None);
            error!("vsock_rx_loop returned {ret:?}");
            ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
        })
        .map_err(|e| LkError::from_lk(e).unwrap_err())?;

    Builder::new()
        .name(c"virtio_vsock_tx")
        .priority(Priority::HIGH)
        .stack_size(stack_size)
        .spawn(move || {
            let ret = vsock_tx_loop(device_for_tx, transport_kind, None, None);
            error!("vsock_tx_loop returned {ret:?}");
            ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
        })
        .map_err(|e| LkError::from_lk(e).unwrap_err())?;

    Ok(())
}
