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

use alloc::boxed::Box;

use core::ffi::c_void;
use core::ptr::null_mut;

pub use crate::sys::handle_close;
pub use crate::sys::handle_decref;
pub use crate::sys::handle_wait;

pub use crate::sys::IPC_HANDLE_POLL_ERROR;
pub use crate::sys::IPC_HANDLE_POLL_HUP;
pub use crate::sys::IPC_HANDLE_POLL_MSG;
pub use crate::sys::IPC_HANDLE_POLL_NONE;
pub use crate::sys::IPC_HANDLE_POLL_READY;
pub use crate::sys::IPC_HANDLE_POLL_SEND_UNBLOCKED;

pub use crate::sys::handle;
pub use crate::sys::handle_ref;

use crate::handle_set::handle_set_detach_ref;
use crate::sys::handle_ref_is_attached;
use crate::sys::list_node;

impl Default for list_node {
    fn default() -> Self {
        Self { prev: core::ptr::null_mut(), next: core::ptr::null_mut() }
    }
}

// nodes in a linked list refer to adjacent nodes by address and should be pinned
// TODO: add Unpin as a negative trait bound once the rustc feature is stabilized.
// impl !Unpin for list_node {}

impl Default for handle_ref {
    fn default() -> Self {
        Self {
            set_node: Default::default(),
            ready_node: Default::default(),
            uctx_node: Default::default(),
            waiter: Default::default(),
            parent: core::ptr::null_mut(),
            handle: core::ptr::null_mut(),
            id: 0,
            emask: 0,
            cookie: core::ptr::null_mut(),
        }
    }
}

// `handle_ref`s should not move since they are inserted as nodes in linked lists
// and the kernel may write back to the non-node fields as well.
// TODO: add Unpin as a negative trait bound once the rustc feature is stabilized.
// impl !Unpin for handle_ref {}

#[derive(Default)]
pub struct HandleRef {
    // Box the `handle_ref` so it doesn't get moved with the `HandleRef`
    inner: Box<handle_ref>,
}

impl HandleRef {
    pub fn is_attached(&self) -> bool {
        // SAFETY: `self.inner` was initialized, and `handle_ref_is_attached`
        // is otherwise safe to call no matter the state of the `handle_ref`.
        unsafe { handle_ref_is_attached(&*self.inner) }
    }

    pub fn detach(&mut self) {
        if self.is_attached() {
            // Safety: `inner` was initialized and attached to a handle set
            unsafe { handle_set_detach_ref(&mut *self.inner) }
        }
    }

    pub fn handle_close(&mut self) {
        if !self.inner.handle.is_null() {
            // Safety: `handle` is non-null so it wasn't closed
            unsafe { handle_close(self.inner.handle) };
            self.inner.handle = null_mut();
        }
    }

    pub fn handle_decref(&mut self) {
        if self.inner.handle.is_null() {
            panic!("handle is null; can't decrease its reference count");
        }

        // Safety: `handle` is non-null so it wasn't closed
        unsafe { handle_decref(self.inner.handle) };
    }

    pub fn as_mut_ptr(&mut self) -> *mut handle_ref {
        &mut *self.inner
    }

    pub fn cookie(&self) -> *mut c_void {
        self.inner.cookie
    }

    pub fn set_cookie(&mut self, cookie: *mut c_void) {
        self.inner.cookie = cookie;
    }

    pub fn emask(&self) -> u32 {
        self.inner.emask
    }

    pub fn set_emask(&mut self, emask: u32) {
        self.inner.emask = emask;
    }

    pub fn handle(&mut self) -> *mut handle {
        self.inner.handle
    }

    pub fn id(&mut self) -> u32 {
        self.inner.id
    }

    pub fn set_id(&mut self, id: u32) {
        self.inner.id = id;
    }
}

impl Drop for HandleRef {
    fn drop(&mut self) {
        self.detach()
    }
}

// Safety: the kernel synchronizes operations on handle refs so they can be passed
// from one thread to another
unsafe impl Send for HandleRef {}

// Safety: the kernel synchronizes operations on handle refs so it safe to share
// references between threads
unsafe impl Sync for HandleRef {}
