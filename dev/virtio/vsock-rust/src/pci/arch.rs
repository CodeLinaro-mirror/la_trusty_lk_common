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

use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(target_arch = "aarch64")] {
        mod aarch64;
        pub(crate) use aarch64::*;
    } else if #[cfg(target_arch = "x86_64")] {
        mod x86_64;
        pub(crate) use x86_64::*;
    } else {
        use core::ptr::NonNull;
        use virtio_drivers::BufferDirection;
        use virtio_drivers::PhysAddr;

        pub(crate) fn dma_alloc_share(_paddr: usize, _size: usize) {
            unimplemented!();
        }

        pub(crate) fn dma_dealloc_unshare(_paddr: PhysAddr, _size: usize) {
            unimplemented!();
        }

        // Safety: unimplemented
        pub(crate) unsafe fn share(_buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
            unimplemented!();
        }

        // Safety: unimplemented
        pub(crate) unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
            unimplemented!();
        }
    }
}
