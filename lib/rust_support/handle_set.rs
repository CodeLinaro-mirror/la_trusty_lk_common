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

use core::time::Duration;

use crate::Error;
use crate::INFINITE_TIME;

pub use crate::sys::handle_set_attach;
pub use crate::sys::handle_set_create;
pub use crate::sys::handle_set_detach_ref;
pub use crate::sys::handle_set_wait;

use crate::sys::handle;
use crate::sys::handle_close;
use crate::sys::handle_wait;

use crate::handle::HandleRef;

pub struct HandleSet(*mut handle);

fn duration_as_ms(dur: Duration) -> Result<u32, Error> {
    match dur {
        Duration::MAX => Ok(INFINITE_TIME),
        dur => dur.as_millis().try_into().map_err(|_| Error::ERR_OUT_OF_RANGE),
    }
}

impl HandleSet {
    pub fn new() -> Self {
        // Safety: `handle_set_create` places no preconditions on callers.
        let handle = unsafe { handle_set_create() };
        if handle.is_null() {
            panic!("handle_set_create failed.");
        }
        Self(handle)
    }

    pub fn attach(&self, href: &mut HandleRef) -> Result<(), Error> {
        if href.attached {
            panic!("HandleRef is already attached.");
        }
        // Safety:
        // `self` contains a properly initialized handle
        // `href.inner` is a properly initialized handle_ref that is not attached
        let ret = unsafe { handle_set_attach(self.0, href.as_mut_ptr()) };
        Error::from_lk(ret)?;

        href.attached = true;
        Ok(())
    }

    pub fn handle_set_wait(&self, href: &mut HandleRef, timeout: Duration) -> Result<(), Error> {
        let timeout = duration_as_ms(timeout)?;
        // Safety:
        // `self` contains a properly initialized handle
        // `href` references a valid storage location for a handle_ref
        let ret = unsafe { handle_set_wait(self.0, href.as_mut_ptr(), timeout) };
        Error::from_lk(ret)
    }

    pub fn handle_wait(&self, event_mask: &mut u32, timeout: Duration) -> Result<(), Error> {
        let timeout = duration_as_ms(timeout)?;
        // Safety:
        // `self` contains a properly initialized handle
        // `event_mask` references a valid storage location for a u32
        let ret = unsafe { handle_wait(self.0, event_mask, timeout) };
        Error::from_lk(ret)
    }
}

impl Drop for HandleSet {
    fn drop(&mut self) {
        // Safety:
        // `handle_set_create` returned a valid handle that wasn't closed already.
        unsafe { handle_close(self.0) }
    }
}

// Safety: the kernel synchronizes operations on handle sets so they can be passed
// from one thread to another
unsafe impl Send for HandleSet {}
