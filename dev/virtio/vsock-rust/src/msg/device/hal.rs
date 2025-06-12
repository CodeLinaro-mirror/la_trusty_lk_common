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

use crate::msg::area_id_and_offset;
use crate::msg::device::virtio_msg_cleanup;
use crate::msg::device::VirtioMsgDevice;
use crate::msg::device::TRANSPORT;
use crate::msg::BusAddress;
use crate::msg::MAX_NUM_SHM;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use extmem::ExtMemObj;
use log::debug;
use log::error;
use rust_support::mmu::ARCH_MMU_FLAG_PERM_NO_EXECUTE;
use rust_support::mmu::PAGE_SIZE_SHIFT;
use virtio_drivers_and_devices::Error as VirtioError;
use virtio_drivers_and_devices::{BufferDirection, DeviceHal};

pub struct TrustyDeviceHal;

impl DeviceHal for TrustyDeviceHal {
    unsafe fn dma_map(
        bus_addr: BusAddress,
        pages: usize,
        _direction: BufferDirection,
        client_id: u16,
    ) -> Result<NonNull<u8>, VirtioError> {
        debug!("mapping bus address {bus_addr:x?} for client {client_id:?}");

        // Break down bus address into its components
        let (area_id, area_offset) = area_id_and_offset(bus_addr);
        let area_offset = area_offset as usize;

        let device = TRANSPORT.get_device(client_id);

        // Find the start virtual address of the shared memory area
        let mut vaddr = 0;
        let state = device.state.lock_save();
        let mem = &state.memory_map[usize::from(area_id)];

        // If there was already an ExtMemObj in the memory map just get its vaddr
        if let Some(ext_mem_obj) = mem {
            // If the shared memory area was previously mapped just use the cached value
            assert!(area_offset < ext_mem_obj.get_size());

            // SAFETY: This address  is only used by the vsock device with the synchronization
            // required by the virtio protocol.
            let obj_vaddr = unsafe { ext_mem_obj.get_vaddr() };
            // obj_vaddr is a NonNull<c_void> so vaddr cannot be zero
            vaddr = obj_vaddr.as_ptr() as usize
        };
        state.unlock_restore();

        // If there was no ExtMemObj in the memory map check the map requests and map one in. This
        // checks the condition vaddr == 0 because using `match mem` would not allow us to drop the
        // state spinlock in this branch. vaddr == 0 will always mean that there was no mapped
        // memory object since ExtMemObj's vaddr is NonNull
        if vaddr == 0 {
            // If the shared memory area has not been mapped, check if it
            // has a pending share area request
            debug!("looking for share request for area id {area_id:x?}");
            let mut state = device.state.lock_save();
            // This changes the state.share_requests entry to None
            let req = state.share_requests[usize::from(area_id)].take();
            state.unlock_restore();

            let mem_handle =
                req.expect("Attempted to map bus address for area that has not been shared");

            debug!("mapping {pages:?} pages for share request with FFA handle {mem_handle:x?}");
            let align_log2 = PAGE_SIZE_SHIFT as u8;
            // TODO: Set RO permission if possible. We can't set ARCH_MMU_FLAG_PERM_RO unless
            // all calls to dma_map for this object will be read-only
            let arch_mmu_flags = ARCH_MMU_FLAG_PERM_NO_EXECUTE;

            let name = c"virtio-msg:extmem";
            let ext_mem_obj = ExtMemObj::map_obj_kernel(
                name,
                u64::from(client_id),
                mem_handle,
                0,    /* tag */
                0,    /* offset */
                None, /* map in the entire object */
                align_log2,
                0, /* vmm_flags */
                arch_mmu_flags,
            )
            .map_err(|e| {
                error!("{e}");
                VirtioError::InvalidParam
            })?;

            // SAFETY: This address  is only used by the vsock device with the synchronization
            // required by the virtio protocol.
            let obj_vaddr = unsafe { ext_mem_obj.get_vaddr() };

            let size = ext_mem_obj.get_size();
            assert!(area_offset < size);

            debug!("mapped at vaddr {obj_vaddr:x?}");
            vaddr = obj_vaddr.as_ptr() as usize;
            // Record the mapped vaddr for future calls to dma_map with this shared memory area
            device.state.lock_save().memory_map[usize::from(area_id)] = Some(ext_mem_obj);
        }

        // Add the offset component of the bus address to the virtual address
        let offset_vaddr = vaddr + area_offset;
        debug!("returning at offset vaddr {offset_vaddr:x?}");

        // TODO: switch to strict provenance APIs when they're stabilized
        NonNull::new(offset_vaddr as *mut u8).ok_or(VirtioError::InvalidParam)
    }

    unsafe fn dma_unmap(_bus_addr: BusAddress, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        // Different bus addresses can correspond to a single external memory object so unmapping is
        // only done in a separate thread once the driver has requested unsharing an area.
        0
    }
}

pub fn memory_unmap(device: &'static VirtioMsgDevice) -> i32 {
    loop {
        // Start waiting to get woken up for unshare requests or because the peer was torn down
        device.wake_memory_unmap.wait();
        if device.peer_dying.load(Ordering::Relaxed) {
            debug!("stopping memory unmap thread");
            // Wait for the other vsock threads to stop in this same thread
            return virtio_msg_cleanup(device);
        }

        // There may be up to MAX_NUM_SHM requests to handle so
        // pre-allocate temporary space before taking the lock
        let mut ext_mem_objs = Vec::with_capacity(MAX_NUM_SHM);

        for req in device.unshare_requests.lock_save().iter_mut() {
            if let Some(ext_mem_obj) = req.take() {
                ext_mem_objs.push(ext_mem_obj);
            }
        }
        for obj in ext_mem_objs {
            // TODO: what should happen with failed unmappings?
            let _ = obj.unmap_obj().inspect_err(|(obj, e)| {
                debug!("Failed to unmap external memory object {obj:x?} with error code -{e:?}");
            });
        }
    }
}
