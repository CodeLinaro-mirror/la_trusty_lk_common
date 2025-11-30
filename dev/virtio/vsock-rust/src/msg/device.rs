/*
 * Copyright (c) 2025 Google Inc. All rights reserved
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

use crate::msg::device::hal::TrustyDeviceHal;
use crate::msg::device::handle_req::handle_req_callback;
use crate::msg::device::requests::VirtioMsgPayload;
use crate::msg::device::requests::VirtioMsgReq;
use crate::msg::device::requests::VirtioMsgResp;
use crate::msg::device::transport::FFAMsgTransport;
use crate::msg::BusAddress;
use crate::msg::MAX_NUM_SHM;
use crate::msg::VIRTIO_MSG_FFA_UUID;
use crate::sys::VIRTIO_CONFIG_S_DRIVER_OK;
use crate::vsock::TransportKind;
use crate::vsock::VsockDevice;
use crate::vsock::VsockRxEvent;
use crate::FFAClientId;
use alloc::boxed::Box;
use alloc::ffi::CString;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use arm_ffa::register_direct_req2_handler;
use core::ffi::c_uint;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering;
use extmem::ext_mem_obj_id_t;
use extmem::ExtMemObj;
use lazy_static::lazy_static;
use log::debug;
use log::trace;
use peer_id::Uuid;
use rust_support::event::Event;
use rust_support::event::EVENT_FLAG_AUTOUNSIGNAL;
use rust_support::init::lk_init_level;
use rust_support::spinlock::IRQSpinLock;
use rust_support::sync::Mutex;
use rust_support::thread;
use rust_support::thread::Priority;
use rust_support::Error as LkError;
use rust_support::LK_INIT_HOOK;
use sm::VmNotifier;
use sm::VmRef;
use trusty::EventClient;
use trusty::EventSource;
use virtio_drivers_and_devices::device::socket::VirtIOSocketDevice;
use virtio_drivers_and_devices::device::socket::VsockDeviceConnectionManager;

mod hal;
mod handle_req;
mod requests;
mod transport;

type FFAMemHandle = u64;
const VSOCK_QUEUE_SIZE: u32 = 8;

const VIRTIO_MSG_DEVICE_GUEST_CID: u32 = {
    let env_var = env!("VSOCK_VIRTIO_MSG_DEVICE_GUEST_CID");
    let guest_cid = match u32::from_str_radix(env_var, 10) {
        Ok(guest_cid) => guest_cid,
        Err(_) => panic!("could not convert VSOCK_VIRTIO_MSG_DEVICE_GUEST_CID to u32"),
    };
    // virtio spec 5.10.4: The upper 32 bits of the CID are reserved and zeroed. The following CIDs are
    // reserved and cannot be used as the guest's context ID: 0, 1, 2, 0xffffffff, 0xffffffffffffffff.
    // `guest_cid` is a u32 so we only need to check it doesn't match the first four reserved values
    if matches!(guest_cid, 0..=2 | u32::MAX) {
        panic!("VSOCK_VIRTIO_MSG_DEVICE_GUEST_CID set to reserved value");
    }
    guest_cid
};

const VIRTIO_MSG_NUM_VMS: usize = {
    let env_var = env!("VSOCK_VIRTIO_MSG_NUM_VMS");
    match usize::from_str_radix(env_var, 10) {
        Ok(num_vms) => num_vms,
        Err(_) => panic!("could not convert VSOCK_VIRTIO_MSG_NUM_VMS to usize"),
    }
};

#[derive(Debug)]
struct VirtQueue {
    size: u32,
    pub desc_table: BusAddress,
    pub avail_ring: BusAddress,
    pub used_ring: BusAddress,
}

#[derive(Debug)]
struct DeviceState {
    status: u32,
    // These are only boxed to avoid large temporaries on the stack
    memory_map: Box<[Option<ExtMemObj>; MAX_NUM_SHM]>,
    share_requests: Box<[Option<FFAMemHandle>; MAX_NUM_SHM]>,

    vqueues: [Option<VirtQueue>; 3],
    threads_started: bool,
}

impl DeviceState {
    pub fn new() -> Self {
        // We use `Vec::into_boxed_slice` to create the `Box<[T]>` fields since `Box::new` tends to
        // generate code with an intermediate array on the stack causing an overflow. We push the
        // elements individually instead of using `vec![None; MAX_NUM_SHM]` since the latter
        // requires that T implement Clone.
        let mut memory_map = Vec::with_capacity(MAX_NUM_SHM);
        for _ in 0..MAX_NUM_SHM {
            memory_map.push(None);
        }
        let memory_map = memory_map.into_boxed_slice().try_into().unwrap();

        let mut share_requests = Vec::with_capacity(MAX_NUM_SHM);
        for _ in 0..MAX_NUM_SHM {
            share_requests.push(None);
        }
        let share_requests = share_requests.into_boxed_slice().try_into().unwrap();

        Self {
            status: 0,
            memory_map,
            share_requests,
            vqueues: [None, None, None],
            threads_started: false,
        }
    }

    /// Resets the `DeviceState` instance to its initial state to allow another client to use it.
    ///
    /// Note that the `memory_map` field must be reset separately since dropping `ExtMemObj`s unmaps
    /// memory which cannot be done while holding the lock allowing us to get a mutable reference to
    /// the `DeviceState`.
    pub fn reset(&mut self) {
        self.status = 0;
        for req in self.share_requests.iter_mut() {
            *req = None;
        }
        self.vqueues = [None, None, None];
        self.threads_started = false;
    }
}

struct VsockEvents {
    // An EventSource used to wake the EventClient in the vsock tx thread. We must use event_source
    // here since the tx loop also waits on other handles. event_source requires a unique name so we
    // use the client's VM ID to ensure this. That means EventSource must be lazily initialized
    // since we only assign the VirtioMsgDevice's VM ID (stored in vm_ids in VirtioMsgTransport)
    // when we receive its first virtio-msg request.
    tx_stop: EventSource,
    rx_stop: Arc<VsockRxEvent>,
    drop_evt: Arc<Event>,
}

pub struct VirtioMsgDevice {
    guest_cid: u32,
    state: Arc<IRQSpinLock<DeviceState>>,
    unshare_requests: IRQSpinLock<Box<[Option<ExtMemObj>; MAX_NUM_SHM]>>,

    // The memory_unmap thread may be woken up either when area_unshare requests come in or when the
    // peer VM gets torn down. Each time it gets woken it checks the peer_dying flag to see what
    // signaled the event.
    wake_memory_unmap: Event,
    peer_dying: AtomicBool,

    vsock_evts: Mutex<Option<VsockEvents>>,
}

impl VirtioMsgDevice {
    pub fn new(guest_cid: u32) -> Self {
        let mut unshare_requests = Vec::with_capacity(MAX_NUM_SHM);
        for _ in 0..MAX_NUM_SHM {
            unshare_requests.push(None);
        }
        let unshare_requests =
            IRQSpinLock::new_irq(unshare_requests.into_boxed_slice().try_into().unwrap());

        Self {
            guest_cid,
            state: Arc::new(IRQSpinLock::new_irq(DeviceState::new())),
            unshare_requests,
            wake_memory_unmap: Event::new(false, EVENT_FLAG_AUTOUNSIGNAL),
            peer_dying: AtomicBool::new(false),
            vsock_evts: Mutex::new(None),
        }
    }

    pub fn unmap_memory(&self) {
        // We can't use an iterator since we can't hold the memory_map field's spinlock while
        // unmapping memory
        for n in 0..MAX_NUM_SHM {
            // Drop mapped memory objects which were previously in use by the vsock loops
            let mapped_memory = self.state.lock_unsaved().memory_map[n].take();
            if let Some(mapped_memory) = mapped_memory {
                debug!("unmapping memory obj {mapped_memory:x?}");
                // ExtMemObj Drop impl also unmaps the memory but doesn't allow returning an error
                mapped_memory.unmap_obj().expect("Failed to unmap memory");
            }

            // Drop mapped memory objects which we had previously received an unshare request for
            // but which the memory_unmap thread had not had a chance to drop. Depending on when a
            // VM is killed it may be possible that it did not have a chance to signal the unshare
            // event so we can't rely on that thread to clean up everything. Dropping a mapped
            // memory object requires taking ownership of it ensuring only one thread will unmap
            // each object.
            let unshare_req_mem = self.unshare_requests.lock_unsaved()[n].take();
            if let Some(mapped_memory) = unshare_req_mem {
                debug!("unmapping memory obj {mapped_memory:x?}");
                mapped_memory.unmap_obj().expect("Failed to unmap memory");
            }
        }
    }

    pub fn set_threads_started_if_ready(&self) -> bool {
        let mut state = self.state.lock_save();

        let device_ok = state.status & VIRTIO_CONFIG_S_DRIVER_OK != 0;
        if !device_ok {
            return false;
        }

        let threads_started = core::mem::replace(&mut state.threads_started, true);
        !threads_started
    }
}

// Stores the state for all VMs using virtio-msg devices. Only one vsock device per VM is currently
// supported.
pub struct VirtioMsgTransport {
    // TODO(b/433488986): Replace this with a bus to support multiple devices per VM
    devices: [VirtioMsgDevice; VIRTIO_MSG_NUM_VMS],
    vm_ids: IRQSpinLock<[Option<FFAClientId>; VIRTIO_MSG_NUM_VMS]>,
    vm_notifiers: Mutex<[Option<VmNotifier>; VIRTIO_MSG_NUM_VMS]>,
    // This event is shared by all VMs to avoid creating a deferred init thread per VM
    device_init: Event,
}

impl VirtioMsgTransport {
    pub fn new() -> Self {
        let devices = core::array::from_fn(|_| VirtioMsgDevice::new(VIRTIO_MSG_DEVICE_GUEST_CID));
        let device_init = Event::new(false, EVENT_FLAG_AUTOUNSIGNAL);
        Self {
            devices,
            vm_ids: IRQSpinLock::new_irq([None; VIRTIO_MSG_NUM_VMS]),
            vm_notifiers: Mutex::new([const { None }; VIRTIO_MSG_NUM_VMS]),
            device_init,
        }
    }

    pub fn get_device(&self, client_id: FFAClientId) -> &VirtioMsgDevice {
        let mut vm_ids = self.vm_ids.lock_save();

        for (n, id) in vm_ids.iter().enumerate() {
            match id {
                Some(registered_id) if client_id == *registered_id => {
                    trace!("Found device #{n:?} for VM {client_id:?}");
                    return &self.devices[n];
                }
                Some(_registered_id) => {}
                None => {
                    trace!("Registering device #{n:?} for VM {client_id:?}");
                    vm_ids[n] = Some(client_id);
                    return &self.devices[n];
                }
            }
        }
        vm_ids.unlock_restore();

        let max_cid = VIRTIO_MSG_NUM_VMS - 1;
        panic!(
            "Could not register device for VM with CID {client_id:?}. \
            Only VMs with a CID between 0 and {max_cid} can register virtio-msg devices."
        );
    }
}

lazy_static! {
    static ref TRANSPORT: VirtioMsgTransport = VirtioMsgTransport::new();
}

fn start_per_device_threads(device: &'static VirtioMsgDevice, client_id: FFAClientId) {
    let transport = FFAMsgTransport::new(device, client_id);
    let virtio_socket_device = VirtIOSocketDevice::<TrustyDeviceHal, _>::new(transport).unwrap();
    let manager = VsockDeviceConnectionManager::new_with_capacity(virtio_socket_device, 4096);
    let vsock_device = VsockDevice::new(manager);

    // Create a unique name based off the VM ID for the event_source
    let evt_name = "vsock-tx-loop-".to_string() + &client_id.to_string();
    let evt_name_with_nul = CString::new(evt_name).expect("client_id contains no internal 0 bytes");
    let evt_source = EventSource::create(evt_name_with_nul.clone(), vec![*Uuid::kernel()]).unwrap();

    // Publish the event_source so the client can open it
    evt_source.publish().unwrap();

    // Store evt_source in the VirtioMsgDevice and get the events for the rx loop and VsockDevice drop
    *device.vsock_evts.lock() = Some(VsockEvents {
        tx_stop: evt_source,
        rx_stop: vsock_device.get_vsock_rx_event(),
        drop_evt: vsock_device.get_vsock_drop_event(),
    });

    let device_for_rx = Arc::new(vsock_device);
    let device_for_tx = device_for_rx.clone();

    // Create the event_client for the tx loop
    let evt_client = EventClient::open(&evt_name_with_nul, *Uuid::kernel()).unwrap();

    // The VM destruction callback only signals to the rx, tx and memory_unmap thread that the VM
    // was destroyed and then returns. Since the vsock rx/tx loops may be accessing the VM's memory
    // when the signal is sent, those threads need to hold VM references to keep the VM alive. When
    // they return, the `VmRef`s get dropped, releasing their refcounts.
    // TODO: Add `impl Clone for VmRef` and replace the second VmRef::new with `.clone()` to avoid
    // checking `sm_vm_get`'s return value twice here.
    let vm_ref_for_tx = VmRef::new(u64::from(client_id)).unwrap();
    let vm_ref_for_rx = VmRef::new(u64::from(client_id)).unwrap();
    // Start threads for device's vsock RX and TX loops. Mapping shared memory will be done in these
    // threads by the dma_map method in the DeviceHal trait. These threads will return when the VM
    // destruction callback signals the vm_destroyed event
    thread::Builder::new()
        .name(c"virtio_vsock_rx")
        .priority(Priority::HIGH)
        .spawn(move || {
            let ret = crate::vsock::vsock_rx_loop(
                device_for_rx,
                TransportKind::DeviceFFAMsg(client_id),
                Some(vm_ref_for_rx),
            );
            debug!("vsock_rx_loop returned {ret:?}");
            ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
        })
        .expect("Failed to spawn thread for virtio_vsock_rx loop");

    thread::Builder::new()
        .name(c"virtio_vsock_tx")
        .priority(Priority::HIGH)
        .spawn(move || {
            let ret = crate::vsock::vsock_tx_loop(
                device_for_tx,
                TransportKind::DeviceFFAMsg(client_id),
                Some(evt_client),
                Some(vm_ref_for_tx),
            );
            debug!("vsock_tx_loop returned {ret:?}");
            ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
        })
        .expect("Failed to spawn thread for virtio_vsock_tx loop");

    // Start thread for device's unshare requests
    thread::Builder::new()
        .name(c"virtio_msg_unmap")
        .spawn(move || crate::msg::device::hal::memory_unmap(device))
        .expect("Failed to spawn thread for virtio-msg unmapping");
}

fn virtio_msg_vm_destroy(client_id: ext_mem_obj_id_t) -> Result<(), LkError> {
    debug!("received destroy notification for VM ID {client_id:?}");

    let device = TRANSPORT.get_device(client_id as FFAClientId);

    // Signal for the device's threads to terminate. This currently includes the vsock-specific rx
    // and tx loops and the virtio-msg memory unmapping thread. The memory_unmap thread is the only
    // transport-specific thread so when it is signaled and sees the peer is not alive, it calls
    // virtio_msg_cleanup and returns.

    // Wake the memory_unmap thread and make sure it sees the peer is no longer alive
    device.peer_dying.store(true, Ordering::Relaxed);
    device.wake_memory_unmap.signal();

    let vsock_evts_guard = device.vsock_evts.lock();
    let vsock_evts = vsock_evts_guard.as_ref().unwrap();
    // Signal the event_source so the event_client in the tx loop gets notified.
    vsock_evts.tx_stop.signal().expect("failed to signal tx event_source");
    // Signal for the rx loop to shutdown.
    vsock_evts.rx_stop.signal(VsockRxEvent::TERMINATE);

    // The tx and rx loops may take some time before returning but this function is called from the
    // sm-vm-notifier thread which should not block (since it handles other VMs) so just return and
    // finalize the cleanup in the memory_unmap thread.
    Ok(())
}

fn virtio_msg_cleanup(device: &'static VirtioMsgDevice) -> i32 {
    debug!("waiting for vsock rx and tx loops to stop");
    // Wait for the VsockDevice to get dropped. That happens either in the rx or tx loop thread
    // depending on which exits last.
    device.vsock_evts.lock().as_ref().unwrap().drop_evt.wait();

    // At this point we know the threads created for this virtio-msg vsock device have been
    // destroyed so we can safely reset `device` for the next client and unmap any memory it was
    // using.
    debug!("resetting vsock device");

    // Unmap memory. We do this explicitly using `unmap_obj` rather than the `ExtMemObj` `Drop`
    // impls to ensure we're not holding a spinlock while memory in being unmapped. This resets the
    // `unshare_requests` field and the `state.memory_map` field.
    device.unmap_memory();
    debug!("finished unmapping vsock device memory");

    // Reset the rest of the `state` field.
    device.state.lock_save().reset();
    device.peer_dying.store(false, Ordering::Relaxed);
    *device.vsock_evts.lock() = None;
    0
}

fn virtio_msg_deferred_init() -> i32 {
    loop {
        // Wait until any device signals it's been initialized
        TRANSPORT.device_init.wait();
        trace!("received device_init signal for virtio-msg device");

        for (n, device) in TRANSPORT.devices.iter().enumerate() {
            if device.set_threads_started_if_ready() {
                let client_id = TRANSPORT.vm_ids.lock_save()[n].unwrap();

                let notif = &mut TRANSPORT.vm_notifiers.lock()[n];
                // Assigning a new value to the vm_notifiers entry will drop the old value freeing
                // that VmNotifier's memory. If this vm_notifiers entry was previously in use this
                // will always happen after the old VM notifier runs because that callback is what
                // resets `device.state`.
                *notif = Some(
                    VmNotifier::new(ext_mem_obj_id_t::from(client_id), virtio_msg_vm_destroy)
                        .expect("Failed to register VM destroy notifier"),
                );

                trace!("initializing virtio-msg vsock device for VM ID {client_id:?}");
                start_per_device_threads(device, client_id);
            }
        }
    }
}

extern "C" fn virtio_msg_device_init_func(_: c_uint) {
    trace!("initializing virtio-msg vsock bus");
    // Explicitly initialize VirtioMsgTransport in this init hook since its constructor allocates
    lazy_static::initialize(&TRANSPORT);

    // Register the request handler for all devices
    // SAFETY: The only safety requirement on the callback is that its second argument is a pointer
    // that may be treated as an array of ARM_FFA_MSG_EXTENDED_ARGS_COUNT u64s.
    unsafe {
        register_direct_req2_handler(VIRTIO_MSG_FFA_UUID, handle_req_callback)
            .expect("failed to register handler for FFA_DIRECT_REQ2");
    }

    // Create a thread to initialize devices once they set up their virtqueues
    thread::Builder::new()
        .name(c"virtio_msg_deferred_init")
        .stack_size(8 * 1024)
        .priority(Priority::HIGH)
        .spawn(virtio_msg_deferred_init)
        .expect("virtio-msg device could not spawn thread for deferred init");
}

LK_INIT_HOOK!(
    virtio_msg_device_init,
    virtio_msg_device_init_func,
    lk_init_level::LK_INIT_LEVEL_THREADING
);
