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

use crate::msg::driver::{SharedHeap, INITIAL_AREA_ID, MAIN_HEAP};
use crate::msg::{area_id_and_offset, bus_address, BusAddress};
use alloc::vec::Vec;
use core::ptr::{copy_nonoverlapping, NonNull};
use rust_support::mmu::PAGE_SIZE;
use rust_support::Error as LkError;
use virtio_drivers_and_devices::{BufferDirection, Hal, PhysAddr};

type PageIdx = usize;

/// A basic allocator for the vsock driver's shared memory.
///
/// It rounds allocations up to a multiple of a page and supports block sizes of one and two pages.
/// The first `Self::DOUBLE_BLOCK_PAGES` pages are used for two page blocks and the rest are for one
/// page blocks.
#[derive(Debug)]
pub struct VsockMemAllocator {
    num_pages: usize,
    free_double_blocks: Vec<PageIdx>,
    free_single_blocks: Vec<PageIdx>,
}

impl VsockMemAllocator {
    // This vsock driver uses two pages for the three virtqueues, and one page for the 8 rx
    // descriptor buffers so we need at least 14 pages.
    const MINIMUM_PAGES: usize = 14;

    // Only the virtqueues use double-page allocations so we reserve the first 6 pages for the 3
    // vsock queues
    const DOUBLE_BLOCK_PAGES: usize = 6;

    pub const fn new() -> Self {
        Self { num_pages: 0, free_double_blocks: Vec::new(), free_single_blocks: Vec::new() }
    }

    pub fn init(&mut self, num_pages: usize) -> Result<(), LkError> {
        if self.num_pages != 0 {
            return Err(LkError::ERR_ALREADY_STARTED);
        }
        if num_pages < Self::MINIMUM_PAGES {
            return Err(LkError::ERR_NOT_ENOUGH_BUFFER);
        }
        self.num_pages = num_pages;
        for i in (0..Self::DOUBLE_BLOCK_PAGES).step_by(2) {
            self.free_double_blocks.push(i);
        }
        for i in Self::DOUBLE_BLOCK_PAGES..num_pages {
            self.free_single_blocks.push(i);
        }
        Ok(())
    }

    pub fn allocate(&mut self, num_pages: usize) -> Result<PageIdx, LkError> {
        let free_blocks = match num_pages {
            1 => &mut self.free_single_blocks,
            2 => &mut self.free_double_blocks,
            _ => return Err(LkError::ERR_NOT_SUPPORTED),
        };
        let new_alloc = free_blocks.pop();
        match new_alloc {
            Some(alloc) => Ok(alloc),
            None => Err(LkError::ERR_NO_MEMORY),
        }
    }

    pub fn deallocate(&mut self, page_idx: PageIdx) -> Result<(), LkError> {
        if self.free_double_blocks.contains(&page_idx)
            || self.free_single_blocks.contains(&page_idx)
        {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        let free_blocks = if page_idx < Self::DOUBLE_BLOCK_PAGES {
            &mut self.free_double_blocks
        } else if page_idx < self.num_pages {
            &mut self.free_single_blocks
        } else {
            return Err(LkError::ERR_INVALID_ARGS);
        };
        free_blocks.push(page_idx);
        Ok(())
    }
}

pub struct MsgHal;

impl MsgHal {
    // Allocates `num_pages` pages out of a preshared `&mut SharedHeap`. Returns the physical
    // address, virtual address and virtio-msg bus address.
    // TODO(b/433488987): Support allocating more memory on demand by sharing additional memory regions
    fn allocate(
        heap: &mut SharedHeap,
        num_pages: usize,
    ) -> Option<(PhysAddr, NonNull<u8>, BusAddress)> {
        let page_idx = match heap.allocator.allocate(num_pages) {
            Ok(idx) => idx,
            Err(_) => return None,
        };
        let offset = page_idx * (PAGE_SIZE as usize);
        let vaddr = NonNull::new((heap.vaddr + offset) as *mut u8).unwrap();
        let paddr = heap.paddr + offset;
        let bus_addr = bus_address(INITIAL_AREA_ID, offset as u64);
        let addrs = (paddr, vaddr, bus_addr);
        Some(addrs)
    }

    fn deallocate(heap: &mut SharedHeap, bus_addr: BusAddress) {
        let (_area_id, offset) = area_id_and_offset(bus_addr);
        assert!(offset % u64::from(PAGE_SIZE) == 0);
        let page_idx = offset / u64::from(PAGE_SIZE);
        heap.allocator.deallocate(page_idx as usize).expect("allocation not found");
    }
}

// SAFETY: See safety comments on individual methods. These functions are only intended to be called
// from the virtio_drivers_and_devices crate.
unsafe impl Hal for MsgHal {
    // Sharing addresses over virtio-msg requires using bus addresses, but virtio-drivers only uses
    // the first element in the return value as the argument to dma_dealloc so it can stay a
    // PhysAddr for simplicity
    fn dma_alloc(num_pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let mut main_heap = MAIN_HEAP.lock();
        // This function is only called to allocate virtqueues which are always 2 pages since this
        // driver always uses the legacy virtqueue layout. It should never panic because vsock only
        // creates 3 virtqueues
        let (paddr, vaddr, _bus_addr) = Self::allocate(&mut main_heap, num_pages)
            .expect("tried to allocate more than 3 virtqueues");
        (paddr, vaddr)
    }

    /// # Safety
    ///
    /// This function should never be called.
    unsafe fn dma_dealloc(_paddr: PhysAddr, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        // This function is only called to deallocate virtqueues when the VirtIOSocket is dropped
        // which is currently unsupported on the driver side and can never be reached.
        panic!("tried to deallocate a virtqueue")
    }

    /// virtio-drivers uses this to populate the addr field in virtqueue descriptors so this must
    /// return a bus address. The memory region represented by the returned bus address is derived
    /// from `vmm_alloc_contiguous` made by initializing the `SharedHeap` so it will not alias any
    /// variables.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `buffer` does not alias the shared memory region initialized by
    /// the `SharedHeap`
    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> BusAddress {
        let num_pages = buffer.len().div_ceil(PAGE_SIZE as usize);
        let mut main_heap = MAIN_HEAP.lock();
        let allocation = Self::allocate(&mut main_heap, num_pages);
        let (_paddr, vaddr, bus_addr) = allocation.expect("could not allocate buffer");

        // If the buffer contains data going from driver to the device copy it into the buffer
        if direction == BufferDirection::DriverToDevice {
            let dst = vaddr.as_ptr();
            let src = buffer.as_ptr().cast::<u8>();
            let size = buffer.len();
            // SAFETY: Both regions are valid, properly aligned, and don't overlap.
            // - Because `vaddr` is a virtual address derived from `dma_alloc`, it is
            // properly aligned and does not overlap with `buffer`.
            // - There are no particular alignment requirements on `buffer`.
            // The dst memory also has not been shared over FFA so it does not have aliasing
            // references.
            unsafe { copy_nonoverlapping(src, dst, size) };
        }
        bus_addr
    }

    /// Unshares a buffer previously shared via `share`.
    ///
    /// If the `direction` is `BufferDirection::DeviceToDriver` this also copies the data written by
    /// the device from the shared memory region back into the provided `buffer`.
    ///
    /// # Safety
    ///
    /// The caller must ensure the memory was previously shared by the same `Hal` implementation,
    /// with the same arguments/return values and only unshared once. Also `buffer` must not overlap
    /// the shared memory region represented by `bus_addr`.
    unsafe fn unshare(bus_addr: BusAddress, buffer: NonNull<[u8]>, direction: BufferDirection) {
        let mut main_heap = MAIN_HEAP.lock();
        let (area_id, offset) = area_id_and_offset(bus_addr);
        let offset = offset as usize;
        assert!(area_id == INITIAL_AREA_ID);

        // If the buffer contains data going from the device to the driver copy it into the buffer
        if direction == BufferDirection::DeviceToDriver {
            let src = main_heap.vaddr + offset;
            let dst = buffer.as_ptr().cast::<u8>();
            let size = buffer.len();
            // SAFETY: Both regions are valid, properly aligned, and don't overlap.
            // - Because `main_heap.vaddr` is a virtual address returned by `dma_alloc`, it is
            // properly aligned and does not overlap with `buffer`.
            // - There are no particular alignment requirements on `buffer`.
            // Also the src memory has been shared over FFA but the driver has successfully sent a
            // virtio-msg area_unshare request so the driver should no longer access it.
            unsafe { copy_nonoverlapping(src as *const u8, dst, size) };
        }

        Self::deallocate(&mut main_heap, bus_addr);
    }

    unsafe fn mmio_phys_to_virt(_paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        unreachable!("virtio-msg dma HAL should not do this conversion")
    }
}
