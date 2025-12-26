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

use crate::msg::driver::hal::{MsgHal, VsockMemAllocator};
use crate::msg::driver::requests::{VirtioMsgReq, VirtioMsgResp};
use crate::msg::driver::transport::FFAMsgTransport;
use crate::msg::VIRTIO_MSG_FFA_UUID;
use crate::sys_dev2::{
    bus_ffa_version_resp as BusFFAVersionResp, get_device_info_resp as GetDeviceInfoResp,
    VIRTIO_MSG_FFA_BUS_VERSION_1_0, VIRTIO_MSG_REVISION_1,
};
use crate::vsock::{vsock_init, TransportKind};
use alloc::vec::Vec;
use arm_ffa::{
    msg_send_direct_req2, partition_info_get_count, partition_info_get_desc, FFAInitState,
};
use core::ffi::c_uint;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};
use lazy_static::lazy_static;
use log::{debug, error, info, warn};
use rust_support::init::lk_init_level;
use rust_support::mmu::{ArchMmuFlags, PAGE_SIZE};
use rust_support::sync::Mutex;
use rust_support::{Error as LkError, LK_INIT_HOOK};
use virtio_drivers_and_devices::device::socket::VirtIOSocket;
use virtio_drivers_and_devices::transport::DeviceType;
use virtio_drivers_and_devices::{BufferDirection, PhysAddr};

mod hal;
mod requests;
mod transport;

type Result<T> = core::result::Result<T, LkError>;

// RECEIVER_ID is an FFA ID so it's only 16 bits, but we use the upper 16 bits as a sentinel to
// ensure it's been validated.
static RECEIVER_ID: AtomicU32 = AtomicU32::new(u32::MAX);

fn get_receiver_id() -> u16 {
    let receiver_id = RECEIVER_ID.load(Ordering::Relaxed);
    // u32::MAX is the initial sentinel value, but any valid value will fit in a u16
    receiver_id.try_into().expect("RECEIVER_ID has not been initialized")
}

// An arbitrary and easily identifiable area id which the main heap will always use. Once the
// virtio-msg driver supports allocating memory on demand the other shared memory regions must make
// sure to not use this area id.
const INITIAL_AREA_ID: u16 = 0x001E;

// This shared memory area contains the virtqueues so it cannot be unshared/deallocate until the
// driver is torn down.
lazy_static! {
    static ref MAIN_HEAP: Mutex<SharedHeap> = Mutex::new(SharedHeap::new());
}

const VIRTIO_MSG_SHARED_MEMORY_SIZE: usize = {
    let env_var = env!("VSOCK_VIRTIO_MSG_SHARED_MEMORY_SIZE");
    let mem_size = match usize::from_str_radix(env_var, 10) {
        Ok(num_vms) => num_vms,
        Err(_) => panic!("could not convert VSOCK_VIRTIO_MSG_SHARED_MEMORY_SIZE to usize"),
    };
    let page_size = PAGE_SIZE as usize;
    // Round up the size to a multiple of the page size
    let num_pages = mem_size.div_ceil(page_size);
    if num_pages < 14 {
        panic!("VSOCK_VIRTIO_MSG_SHARED_MEMORY_SIZE should be at least 14 pages");
    }
    num_pages * page_size
};

#[derive(Debug)]
struct SharedHeap {
    paddr: PhysAddr,
    vaddr: usize,
    shared: bool,
    allocator: VsockMemAllocator,
}

impl SharedHeap {
    const fn new() -> Self {
        Self { paddr: 0, vaddr: 0, shared: false, allocator: VsockMemAllocator::new() }
    }

    fn init(&mut self, heap_size: usize, area_id: u16) -> Result<()> {
        if self.shared {
            return Err(LkError::ERR_ALREADY_STARTED);
        }
        let num_pages = heap_size / PAGE_SIZE as usize;
        if !heap_size.is_multiple_of(PAGE_SIZE as usize) {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        // Call the Trusty-specific DMA allocation function to pre-allocate memory which can be
        // shared over FFA. The returned `SharedHeap` will then allocate out of this memory region
        // to implement the virtio `Hal` trait's `dma_alloc` method. When pointers into this shared
        // heap are created it's only for the virtqueues and descriptor buffers which synchronize
        // accesses on both sides according to the virtio spec. We also prevent a situation where
        // the other side can trigger UB in rust by always copying data into and out of the shared
        // memory region instead of creating references directly to it.
        //
        // Do not use the restricted DMA pools because those are shared with the host.
        let (paddr, vaddr) = crate::hal::dma_alloc(num_pages, BufferDirection::Both, false);
        let vaddr = vaddr.as_ptr().addr();
        let arch_mmu_flags = ArchMmuFlags::PERM_NO_EXECUTE;
        // SAFETY: This memory came from `vmm_alloc_contiguous` with the same arch_mmu_flags
        // (NO_EXECUTE) used below so it's safe to share with another FFA endpoint.
        let ffa_handle = unsafe {
            arm_ffa::mem_share_kernel_buffer(get_receiver_id(), paddr, num_pages, arch_mmu_flags)?
        };
        let req =
            VirtioMsgReq::new_bus_area_share(area_id, ffa_handle.get(), num_pages, arch_mmu_flags);
        send_virtio_msg_request(req)?;

        self.allocator.init(num_pages)?;

        self.paddr = paddr;
        self.vaddr = vaddr;
        self.shared = true;
        Ok(())
    }
}

fn init_receiver_id() -> Result<u16> {
    let num_desc = partition_info_get_count(VIRTIO_MSG_FFA_UUID).inspect_err(|e| {
        error!("arm_ffa_partition_info_get_count failed with {e:?}");
    })?;
    if num_desc != 1 {
        error!(
                "arm_ffa_partition_info_get_count: expected 1 partition descriptor, received {num_desc:?}"
            );
        return Err(LkError::ERR_GENERIC);
    }

    let mut desc = [MaybeUninit::uninit()];
    let mut desc_iter =
        partition_info_get_desc(VIRTIO_MSG_FFA_UUID, &mut desc).inspect_err(|e| {
            error!("arm_ffa_partition_info_get_desc failed with {e:?}");
        })?;
    let receiver_id = match desc_iter.next() {
        Some(init_desc) => init_desc.partition_id,
        None => {
            error!("arm_ffa_partition_info_get_desc did not return any descriptors");
            return Err(LkError::ERR_GENERIC);
        }
    };
    RECEIVER_ID.store(u32::from(receiver_id), Ordering::Relaxed);
    Ok(receiver_id)
}

fn send_virtio_msg_request(req: VirtioMsgReq) -> Result<VirtioMsgResp> {
    let receiver_id = get_receiver_id();
    let resp =
        msg_send_direct_req2(VIRTIO_MSG_FFA_UUID, receiver_id, req.get_buf()).inspect_err(|e| {
            warn!("msg_send_direct_req2 failed {e:?}");
        })?;
    VirtioMsgResp::new(resp.params)
}

fn negotiate_version(
    driver_version: u32,
    vmsg_revision: u32,
    num_shm: u16,
) -> Result<BusFFAVersionResp> {
    let req = VirtioMsgReq::new_bus_ffa_version(driver_version, vmsg_revision, num_shm);
    let resp = send_virtio_msg_request(req)?;
    resp.read_bus_ffa_version()
}

// Enumerate devices and return a Vec with the IDs of the devices available.
fn enumerate_devices() -> Result<Vec<u16>> {
    let mut device_ids = Vec::new();
    debug!("enumerating virtio-msg devices");
    // virtio-msg spec 4.4.7.1: The offset and number of device numbers requested MUST be
    // multiples of 8.
    // This driver implementation requests 8 devices at a time.
    let num_req_devices = 8;
    let mut next_bitmap_offset = 0;

    // The next_offset resp field is a u16 which should increase on every iteration or go to zero.
    // That ensures that this loop will terminate after a fixed amount of time.
    loop {
        let current_bitmap_offset = next_bitmap_offset;
        // Send the BUS_MSG_GET_DEVICES request with bitmap offset = 0 or the value specified by the
        // response in the previous iteration
        let req = VirtioMsgReq::new_bus_get_devices(current_bitmap_offset, num_req_devices);
        let resp = send_virtio_msg_request(req)?;
        let (get_devices_resp, bitmap) = resp.read_bus_get_devices()?;

        let num_devices = get_devices_resp.num;
        if usize::from(num_devices) != bitmap.len() * 8 {
            debug!(
                "virtio-msg GET_DEVICES returned wrong bitmap for num_devices ({num_devices:?})"
            );
            return Err(LkError::ERR_INVALID_ARGS);
        }

        // virtio-msg spec 4.4.7.1: The next offset MUST also be a multiple of 8.
        let next_is_multiple_of_8 = get_devices_resp.next_offset % 8 == 0;

        let next_increased = get_devices_resp.next_offset > current_bitmap_offset;

        if !next_is_multiple_of_8 || (get_devices_resp.next_offset != 0 && !next_increased) {
            let bad_next_offset = get_devices_resp.next_offset;
            debug!("virtio-msg GET_DEVICES returned invalid next_offset {bad_next_offset:?}");
            return Err(LkError::ERR_INVALID_ARGS);
        }

        for bit in 0..get_devices_resp.num {
            let mask = 1 << bit;
            let idx = usize::from(bit / 8);
            let dev_avail = (bitmap[idx] & mask) != 0;
            if dev_avail {
                device_ids.push(bit + current_bitmap_offset);
            }
        }

        next_bitmap_offset = get_devices_resp.next_offset;
        if next_bitmap_offset == 0 {
            break;
        }
    }

    Ok(device_ids)
}

fn get_device_info(dev_id: u16) -> Result<GetDeviceInfoResp> {
    let req = VirtioMsgReq::new_get_device_info(dev_id);
    let resp = send_virtio_msg_request(req)?;
    resp.read_get_device_info()
}

fn driver_init() -> Result<()> {
    match arm_ffa::get_init_state() {
        FFAInitState::InitFailed => {
            // FFA init hook failed so log that the vsock driver is not enabled and continue booting
            info!("disabling virtio-msg vsock driver (FFA init failed)");
            return Ok(());
        }
        FFAInitState::Uninit => {
            error!("virtio-msg vsock driver hook ran before ARM FFA hook");
            return Err(LkError::ERR_NOT_CONFIGURED);
        }
        FFAInitState::InitSuccess { major_version: 1, minor_version } if minor_version >= 2 => {
            // If FFA 1.x where x >= 2 was negotiated continue driver init
        }
        FFAInitState::InitSuccess { major_version, minor_version } => {
            info!(
                "disabling virtio-msg vsock driver (FFA version {:?}.{:?} unsupported)",
                major_version, minor_version
            );
            return Ok(());
        }
    }

    // Call FFA_PARTITION_INFO_GET to get the FFA ID for the partition with the virtio-msg device
    let ffa_id = init_receiver_id()?;

    let negotiate_resp = negotiate_version(
        VIRTIO_MSG_FFA_BUS_VERSION_1_0,
        VIRTIO_MSG_REVISION_1,
        /* FEATURE_DIRECT_MSG_TX_SUPP is hard-coded since that's the only thing Trusty supports */
        1, /* num_shm */
    )
    .inspect_err(|e| {
        error!("virtio-msg version request failed with {e}");
    })?;
    debug!("received {negotiate_resp:?} as response to virtio-msg version request");
    MAIN_HEAP.lock().init(VIRTIO_MSG_SHARED_MEMORY_SIZE, INITIAL_AREA_ID)?;

    let device_ids = enumerate_devices()?;
    if device_ids.is_empty() {
        warn!("no virtio-msg devices found");
    }
    // TODO: Add FFA_BUS_MSG_EVENT_CONFIGURE request/response structs to bindgen'ed headers and
    // configure the event delivery mechanism as described in ARM's virtio-msg-ffa spec section 2.4.
    // All device/driver implementations currently behave as if polling was configured.

    // Go through all the devices on the virtio-msg bus and initialize the vsock devices
    for dev_id in device_ids {
        debug!("getting info for device #{dev_id:?}");
        let dev_info_resp = get_device_info(dev_id)?;
        debug!("get_device_info returned {dev_info_resp:?}");

        let dev_ty = dev_info_resp.device_id;
        if dev_ty != DeviceType::Socket as u32 {
            // Non vsock virtio-msg devices are not currently expected but should just be ignored
            info!("ignoring unexpected non-vsock virtio-msg device with type {dev_ty:?}");
            continue;
        }
        // Since virtio-driver Transport trait doesn't allow specifying a device ID we need to
        // create a FFAMsgTransport for each vsock device
        let transport = FFAMsgTransport::new(dev_id);
        // Use page sized buffers for the rx virtqueue
        let driver: VirtIOSocket<MsgHal, FFAMsgTransport, { PAGE_SIZE as usize }> =
            VirtIOSocket::new(transport).map_err(|e| {
                error!("could not create VirtIOSocket {e:?}");
                LkError::ERR_GENERIC
            })?;
        vsock_init(driver, TransportKind::DriverFFAMsg(ffa_id)).map_err(|e| {
            error!("vsock_init failed {e:?}");
            LkError::ERR_GENERIC
        })?;
    }

    Ok(())
}

extern "C" fn virtio_msg_driver_init_func(_: c_uint) {
    debug!("initializing virtio-msg vsock driver...");
    if driver_init().is_err() {
        panic!("failed to initialize virtio-msg vsock driver");
    };
}

LK_INIT_HOOK!(
    virtio_msg_driver_init,
    virtio_msg_driver_init_func,
    lk_init_level::LK_INIT_LEVEL_PLATFORM
);
