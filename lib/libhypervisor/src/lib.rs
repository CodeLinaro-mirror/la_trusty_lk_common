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
#![no_std]
#![feature(unsigned_is_multiple_of)]

use core::ffi::c_int;

use rust_support::paddr_t;
use rust_support::Error as LkError;

#[cfg(target_arch = "aarch64")]
mod hyp;

// TODO: Enable for x86_64 once it's supported.
#[cfg(target_arch = "aarch64")]
pub use hyp::mmio_map_region;

/// # Safety
/// Actually not unsafe for targets other then aarch64
#[cfg(not(target_arch = "aarch64"))]
pub unsafe fn mmio_map_region(_paddr: usize, _size: usize) -> Result<(), LkError> {
    Err(LkError::ERR_NOT_SUPPORTED)
}

/// # Safety
///  - paddr must be a valid physical address
///  - paddr + size must be a valid physical address
///  - the caller must be aware that after the call the [paddr .. paddr + size] memory
///    is available for reading by the host.
#[no_mangle]
pub unsafe extern "C" fn hypervisor_mmio_map_region(paddr: paddr_t, size: usize) -> c_int {
    crate::mmio_map_region(paddr, size).err().unwrap_or(LkError::NO_ERROR).into()
}
