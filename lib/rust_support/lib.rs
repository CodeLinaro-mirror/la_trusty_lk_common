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

//! Rust support library for the Trusty kernel

#![no_std]
#![feature(c_str_literals)]
#![deny(unsafe_op_in_unsafe_fn)]

use alloc::format;
use core::ffi::CStr;
use core::panic::PanicInfo;

mod sys {
    #![allow(unused)]
    #![allow(non_camel_case_types)]
    #![allow(non_upper_case_globals)]
    include!(env!("BINDGEN_INC_FILE"));
}

pub mod err;
pub mod init;
pub mod mmu;
pub mod sync;
pub mod vmm;

pub use sys::paddr_t;
pub use sys::status_t;
pub use sys::vaddr_t;

#[panic_handler]
fn handle_panic(info: &PanicInfo) -> ! {
    let panic_message = format!("{info}\0");
    let panic_message_c = CStr::from_bytes_with_nul(panic_message.as_bytes())
        .expect("Unexpected null byte in panic message");
    // SAFETY: Calling C function with string pointers that outlive the call
    unsafe { sys::_panic(c"Rust in Trusty kernel %s\n".as_ptr(), panic_message_c.as_ptr()) }
}
