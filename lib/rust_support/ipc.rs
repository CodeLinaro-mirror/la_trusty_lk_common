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

use core::ptr::null_mut;

pub use crate::sys::ipc_get_msg;
pub use crate::sys::ipc_port_accept;
pub use crate::sys::ipc_port_connect_async;
pub use crate::sys::ipc_port_create;
pub use crate::sys::ipc_port_publish;
pub use crate::sys::ipc_put_msg;
pub use crate::sys::ipc_read_msg;
pub use crate::sys::ipc_send_msg;

pub use crate::sys::iovec_kern;
pub use crate::sys::ipc_msg_info;
pub use crate::sys::ipc_msg_kern;

pub use crate::sys::kernel_uuid;
pub use crate::sys::zero_uuid;
pub use crate::sys::IPC_CONNECT_WAIT_FOR_PORT;
pub use crate::sys::IPC_PORT_ALLOW_NS_CONNECT;
pub use crate::sys::IPC_PORT_ALLOW_TA_CONNECT;
pub use crate::sys::IPC_PORT_PATH_MAX;

impl Default for ipc_msg_kern {
    fn default() -> Self {
        Self { iov: null_mut(), num_iov: 0, handles: null_mut(), num_handles: 0 }
    }
}

impl ipc_msg_kern {
    pub fn new(iov: &mut iovec_kern) -> Self {
        Self { iov, num_iov: 1, ..ipc_msg_kern::default() }
    }
}

impl From<&mut [u8]> for iovec_kern {
    fn from(value: &mut [u8]) -> Self {
        Self { iov_base: value.as_mut_ptr() as _, iov_len: value.len() }
    }
}
