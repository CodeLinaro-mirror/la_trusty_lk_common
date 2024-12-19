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

pub use crate::sys::ktipc_port_acl;
pub use crate::sys::ktipc_server;
pub use crate::sys::ktipc_server_init;
pub use crate::sys::ktipc_server_start;
pub use crate::sys::uuid;

use core::ffi::CStr;
use trusty_std::boxed::Box;

// TODO(b/384572144): Port ktipc_server to rust instead of using FFI.
pub fn ktipc_server_new(name: &'static CStr) -> Box<ktipc_server> {
    let mut srv = Box::<ktipc_server>::new_uninit();
    // SAFETY: Initializes object declared above. name is static.
    unsafe {
        ktipc_server_init(srv.as_mut_ptr(), name.as_ptr());
    }
    // SAFETY: Initialized above with a function that cannot fail.
    unsafe { srv.assume_init() }
}
