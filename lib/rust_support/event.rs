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

use crate::sys::{event_init, event_signal, event_wait_timeout};
use crate::sys::{event_t, status_t, uint};
use crate::INFINITE_TIME;
use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::mem;

pub use crate::sys::EVENT_FLAG_AUTOUNSIGNAL;

// `event_t`s should not move since they contain wait queues which contains
// linked list nodes. The kernel may write back to the non-node fields as well.
// TODO: add Unpin as a negative trait bound once the rustc feature is stabilized.
// impl !Unpin for event_t {}

#[derive(Debug)]
pub struct Event(Box<UnsafeCell<event_t>>);

impl Event {
    pub fn new(initial: bool, flags: uint) -> Self {
        // SAFETY: event_t is a C type which can be zeroed out
        let event = unsafe { mem::zeroed() };
        let mut boxed_event = Box::new(UnsafeCell::new(event));
        // SAFETY: event_init just writes to the fields of event_t. The self-referential `wait`
        // field is written to after the event_t is placed on the heap so it won't become
        // invalidated until it's dropped.
        unsafe { event_init(boxed_event.get_mut(), initial, flags) };
        Self(boxed_event)
    }

    pub fn wait(&self) -> status_t {
        // SAFETY: One or more threads are allowed to wait for an event to be signaled
        unsafe { event_wait_timeout(self.0.get(), INFINITE_TIME) }
    }

    pub fn signal(&self) -> status_t {
        // SAFETY: Events may be signaled from any thread or interrupt context if the reschedule
        // parameter is false
        unsafe {
            event_signal(self.0.get(), false /* reschedule */)
        }
    }
}

// SAFETY: More than one thread may wait using an &Event but only one will be woken up and change
// its signaled field. &Event provides no other APIs to modify its state
unsafe impl Sync for Event {}

// SAFETY: Event is heap allocated so it may be freely sent across threads without invalidating it.
// It may also be waited on and signaled from any thread.
unsafe impl Send for Event {}
