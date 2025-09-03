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
use crate::vsock::VsockDevice;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use arm_ffa::register_direct_req2_handler;
use core::ffi::c_uint;
use extmem::ExtMemObj;
use lazy_static::lazy_static;
use log::debug;
use log::trace;
use rust_support::event::Event;
use rust_support::event::EVENT_FLAG_AUTOUNSIGNAL;
use rust_support::init::lk_init_level;
use rust_support::spinlock::IRQSpinLock;
use rust_support::thread;
use rust_support::thread::Priority;
use rust_support::Error as LkError;
use rust_support::LK_INIT_HOOK;
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

type ClientId = u16;

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
}

pub struct VirtioMsgDevice {
    guest_cid: u32,
    unshare: Event,
    state: Arc<IRQSpinLock<DeviceState>>,
    unshare_requests: IRQSpinLock<Box<[Option<ExtMemObj>; MAX_NUM_SHM]>>,
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
            unshare: Event::new(false, EVENT_FLAG_AUTOUNSIGNAL),
            state: Arc::new(IRQSpinLock::new_irq(DeviceState::new())),
            unshare_requests,
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
    vm_ids: IRQSpinLock<[Option<ClientId>; VIRTIO_MSG_NUM_VMS]>,
    // This event is shared by all VMs to avoid creating a deferred init thread per VM
    device_init: Event,
}

impl VirtioMsgTransport {
    pub fn new() -> Self {
        let devices = core::array::from_fn(|_| VirtioMsgDevice::new(VIRTIO_MSG_DEVICE_GUEST_CID));
        let device_init = Event::new(false, EVENT_FLAG_AUTOUNSIGNAL);
        Self { devices, vm_ids: IRQSpinLock::new_irq([None; VIRTIO_MSG_NUM_VMS]), device_init }
    }

    pub fn get_device(&self, client_id: ClientId) -> &VirtioMsgDevice {
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

fn start_per_device_threads(device: &'static VirtioMsgDevice, client_id: ClientId) {
    let transport = FFAMsgTransport::new(device, client_id);
    let virtio_socket_device = VirtIOSocketDevice::<TrustyDeviceHal, _>::new(transport).unwrap();
    let manager = VsockDeviceConnectionManager::new_with_capacity(virtio_socket_device, 4096);
    let device_for_rx = Arc::new(VsockDevice::new(manager));
    let device_for_tx = device_for_rx.clone();

    // Start threads for device's vsock RX and TX loops. Mapping shared memory will be done in these
    // threads by the dma_map method in the DeviceHal trait.
    thread::Builder::new()
        .name(c"virtio_vsock_rx")
        .priority(Priority::HIGH)
        .spawn(move || {
            let ret = crate::vsock::vsock_rx_loop(device_for_rx);
            debug!("vsock_rx_loop returned {ret:?}");
            ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
        })
        .expect("Failed to spawn thread for virtio_vsock_rx loop");

    thread::Builder::new()
        .name(c"virtio_vsock_tx")
        .priority(Priority::HIGH)
        .spawn(move || {
            let ret = crate::vsock::vsock_tx_loop(device_for_tx, None);
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

fn virtio_msg_deferred_init() -> i32 {
    loop {
        // Wait until any device signals it's been initialized
        TRANSPORT.device_init.wait();
        trace!("received device_init signal for virtio-msg device");

        for (n, device) in TRANSPORT.devices.iter().enumerate() {
            if device.set_threads_started_if_ready() {
                let client_id = TRANSPORT.vm_ids.lock_save()[n].unwrap();
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
