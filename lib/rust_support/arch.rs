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

pub fn clean_cache_range<T>(slice: &[T]) {
    let range = slice.as_ptr_range();
    // SAFETY: The caller passes in a valid slice and the
    // only side effects of the kernel function are on the cache.
    unsafe {
        crate::sys::arch_clean_cache_range(
            range.start.addr(),
            range.end.byte_offset_from_unsigned(range.start),
        );
    }
}

pub fn sync_cache_range<T>(slice: &[T]) {
    let range = slice.as_ptr_range();
    // SAFETY: The caller passes in a valid slice and the
    // only side effects of the kernel function are on the cache.
    unsafe {
        crate::sys::arch_sync_cache_range(
            range.start.addr(),
            range.end.byte_offset_from_unsigned(range.start),
        );
    }
}
