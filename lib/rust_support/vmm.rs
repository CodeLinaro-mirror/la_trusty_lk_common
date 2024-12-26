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

use core::ffi::c_char;
use core::ffi::c_uint;
use core::ffi::c_void;
use core::ptr::{addr_of, addr_of_mut};

use crate::paddr_t;
use crate::status_t;
use crate::Error;

pub use crate::sys::vaddr_to_paddr;
pub use crate::sys::vmm_alloc;
pub use crate::sys::vmm_alloc_contiguous;
pub use crate::sys::vmm_alloc_physical_etc;
pub use crate::sys::vmm_aspace_t;
pub use crate::sys::vmm_free_region;
pub use crate::sys::vmm_get_obj;
pub use crate::sys::vmm_obj_service;
pub use crate::sys::vmm_obj_service_add;
pub use crate::sys::vmm_obj_service_create_ro;
pub use crate::sys::vmm_obj_service_destroy;
pub use crate::sys::vmm_obj_slice;
pub use crate::sys::vmm_obj_slice_init;
pub use crate::sys::vmm_obj_slice_release;

use core::ffi::CStr;

#[inline]
pub fn vmm_get_kernel_aspace() -> *mut vmm_aspace_t {
    // SAFETY: The returned raw pointer holds the same safety invariants as accessing a `static mut`,
    // so this `unsafe` is unconditionally sound, and may become safe in edition 2024:
    // <https://github.com/rust-lang/rust/issues/114447>.
    unsafe { addr_of_mut!(crate::sys::_kernel_aspace) }
}

/// # Safety
///
/// Same as [`vmm_alloc_physical_etc`].
#[allow(clippy::too_many_arguments)]
#[inline]
pub unsafe fn vmm_alloc_physical(
    aspace: *mut vmm_aspace_t,
    name: *const c_char,
    size: usize,
    ptr: *const *mut c_void,
    align_log2: u8,
    paddr: paddr_t,
    vmm_flags: c_uint,
    arch_mmu_flags: c_uint,
) -> status_t {
    // SAFETY: Delegated.
    unsafe {
        vmm_alloc_physical_etc(
            aspace,
            name,
            size,
            ptr.cast_mut(),
            align_log2,
            addr_of!(paddr),
            1,
            vmm_flags,
            arch_mmu_flags,
        )
    }
}

/// A wrapper for an array allocated by Trusty's vmm library.
pub struct VmmPageArray<'a> {
    pub arr: &'a mut [u8],
}

impl VmmPageArray<'_> {
    /// Allocates memory with vmm_allox. size if automatically aligned up to the next page size.
    /// Memory is automatically freed in drop.
    pub fn new(
        name: &'static CStr,
        size: usize,
        align_log2: u8,
        vmm_flags: c_uint,
    ) -> Result<Self, Error> {
        let aspace = vmm_get_kernel_aspace();
        let mut aligned_ptr: *mut c_void = core::ptr::null_mut();
        // SAFETY: Name is static and will therefore outlive the allocation. The return code is
        // checked before returning the array to the caller.
        let rc = unsafe {
            crate::sys::vmm_alloc(
                aspace,
                name.as_ptr(),
                size,
                &mut aligned_ptr,
                align_log2,
                vmm_flags,
                crate::mmu::ARCH_MMU_FLAG_CACHED | crate::mmu::ARCH_MMU_FLAG_PERM_NO_EXECUTE,
            )
        };
        if rc < 0 {
            Error::from_lk(rc)?;
        }
        // SAFETY: Aligned pointer was successfully allocated by vmm_alloc and the same size is
        // being used.
        let arr: &mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(aligned_ptr as *mut u8, size) };
        Ok(Self { arr })
    }
}

impl Drop for VmmPageArray<'_> {
    fn drop(&mut self) {
        let aspace = vmm_get_kernel_aspace();
        // SAFETY: Freeing a pointer allocated by vmm_alloc.
        unsafe { vmm_free_region(aspace, self.arr.as_ptr() as usize) };
    }
}
