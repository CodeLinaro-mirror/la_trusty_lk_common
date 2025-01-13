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

use alloc::collections::btree_map::BTreeMap;

use lazy_static::lazy_static;

use core::ffi::c_void;
use core::ops::DerefMut;
use core::ptr::copy_nonoverlapping;
use core::ptr::NonNull;

use crate::kvm::share_pages;
use crate::kvm::unshare_pages;

use crate::pci::hal::TrustyHal;

use rust_support::paddr_t;
use rust_support::sync::Mutex;
use rust_support::vaddr_t;

use static_assertions::assert_cfg;

use virtio_drivers::BufferDirection;
use virtio_drivers::Hal;
use virtio_drivers::PhysAddr;
use virtio_drivers::PAGE_SIZE;

// This code will only work on x86_64 or aarch64
assert_cfg!(any(target_arch = "x86_64", target_arch = "aarch64"), "Must target x86_64 or aarch64");

lazy_static! {
    /// Stores the paddr to vaddr mapping in `share` for use in `unshare`
    static ref VADDRS: Mutex<BTreeMap<paddr_t, vaddr_t>> = Mutex::new(BTreeMap::new());
}

/// Perform architecture-specific DMA allocation
pub(crate) fn dma_alloc_share(paddr: usize, size: usize) {
    share_pages(paddr, size).expect("failed to share pages");
}

/// Perform architecture-specific DMA deallocation
pub(crate) fn dma_dealloc_unshare(paddr: PhysAddr, size: usize) {
    unshare_pages(paddr, size).expect("failed to unshare pages");
}

// Safety: buffer must be a valid kernel virtual address that is not already mapped for DMA.
pub(crate) unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
    let size = buffer.len();
    let pages = to_pages(size);

    let (paddr, vaddr) = TrustyHal::dma_alloc(pages, direction);
    if let Some(old_vaddr) = VADDRS.lock().deref_mut().insert(paddr, vaddr.as_ptr() as usize) {
        panic!("paddr ({:#x}) was already mapped to vaddr ({:#x})", paddr, old_vaddr);
    }

    let dst_ptr = vaddr.as_ptr() as *mut c_void;

    if direction != BufferDirection::DeviceToDriver {
        let src_ptr = buffer.as_ptr() as *const u8 as *const c_void;
        // Safety: Both regions are valid, properly aligned, and don't overlap.
        // - Because `vaddr` is a virtual address returned by `dma_alloc`, it is
        // properly aligned and does not overlap with `buffer`.
        // - There are no particular alignment requirements on `buffer`.
        unsafe { copy_nonoverlapping(src_ptr, dst_ptr, size) };
    }

    paddr
}

// Safety:
// - paddr is a valid physical address returned by call to `share`
// - buffer must be a valid kernel virtual address previously passed to `share` that
//   has not already been `unshare`d by this function.
pub(crate) unsafe fn unshare(paddr: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
    let size = buffer.len();
    let vaddr = VADDRS.lock().deref_mut().remove(&paddr).expect("paddr was inserted by share")
        as *const c_void;

    if direction != BufferDirection::DriverToDevice {
        let dest = buffer.as_ptr() as *mut u8 as *mut c_void;
        // Safety: Both regions are valid, properly aligned, and don't overlap.
        // - Because `vaddr` was retrieved from `VADDRS`, it must have been returned
        //   from the call to `dma_alloc` in `share`.
        // - Because `vaddr` is a virtual address returned by `dma_alloc`, it is
        //   properly aligned and does not overlap with `buffer`.
        // - There are no particular alignment requirements on `buffer`.
        unsafe { copy_nonoverlapping(vaddr, dest, size) };
    }

    let vaddr = NonNull::<u8>::new(vaddr as *mut u8).unwrap();
    // Safety: memory was allocated by `share` and not previously `unshare`d.
    unsafe {
        TrustyHal::dma_dealloc(paddr, vaddr, to_pages(size));
    }
}

fn to_pages(size: usize) -> usize {
    size.div_ceil(PAGE_SIZE)
}
