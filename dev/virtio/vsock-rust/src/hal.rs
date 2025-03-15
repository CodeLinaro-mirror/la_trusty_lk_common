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

use core::ffi::CStr;
use core::ptr::NonNull;
use rust_support::mmu::ARCH_MMU_FLAG_PERM_NO_EXECUTE;
use rust_support::mmu::PAGE_SIZE_SHIFT;
use rust_support::vmm::vaddr_to_paddr;
use rust_support::vmm::vmm_alloc_contiguous;
use rust_support::vmm::vmm_free_region;
use rust_support::vmm::vmm_get_kernel_aspace;
use virtio_drivers::BufferDirection;
use virtio_drivers::PhysAddr;
use virtio_drivers::PAGE_SIZE;

pub(crate) fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
    const NAME: &CStr = c"vsock-rust";
    // dma_alloc requests num pages but vmm_alloc_contiguous expects bytes.
    let size = pages * PAGE_SIZE;
    let mut vaddr = core::ptr::null_mut(); // stores pointer to virtual memory
    let align_pow2 = PAGE_SIZE_SHIFT as u8;
    let vmm_flags = 0;
    let arch_mmu_flags = ARCH_MMU_FLAG_PERM_NO_EXECUTE;
    let aspace = vmm_get_kernel_aspace();

    // NOTE: the allocated memory will be zeroed since vmm_alloc_contiguous
    // calls vmm_alloc_pmm which does not set the PMM_ALLOC_FLAG_NO_CLEAR
    // flag.
    //
    // Safety:
    // `aspace` is `vmm_get_kernel_aspace()`.
    // `name` is a `&'static CStr`.
    // `size` is validated by the callee
    let rc = unsafe {
        vmm_alloc_contiguous(
            aspace,
            NAME.as_ptr(),
            size,
            &mut vaddr,
            align_pow2,
            vmm_flags,
            arch_mmu_flags,
        )
    };
    if rc != 0 {
        panic!("error {} allocating physical memory", rc);
    }
    if vaddr as usize & (PAGE_SIZE - 1usize) != 0 {
        panic!("error page-aligning allocation {:#x}", vaddr as usize);
    }

    // Safety: `vaddr` is valid because the call to `vmm_alloc_continuous` succeeded
    let paddr = unsafe { vaddr_to_paddr(vaddr) };

    (paddr, NonNull::<u8>::new(vaddr as *mut u8).unwrap())
}

// Safety: `vaddr` was returned by `dma_alloc` and hasn't been deallocated.
pub(crate) unsafe fn dma_dealloc(_paddr: PhysAddr, vaddr: NonNull<u8>, _pages: usize) -> i32 {
    let aspace = vmm_get_kernel_aspace();
    let vaddr = vaddr.as_ptr();
    // Safety:
    // - function-level requirements
    // - `aspace` points to the kernel address space object
    // - `vaddr` is a region in `aspace`
    unsafe { vmm_free_region(aspace, vaddr as usize) }
}
