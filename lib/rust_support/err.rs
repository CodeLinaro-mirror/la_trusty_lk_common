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

// Bindgen emits this constant as a u32 so declare it as i32 here instead.
pub const NO_ERROR: i32 = 0;
pub use crate::sys::ERR_ACCESS_DENIED;
pub use crate::sys::ERR_ALREADY_EXISTS;
pub use crate::sys::ERR_ALREADY_EXPIRED;
pub use crate::sys::ERR_ALREADY_MOUNTED;
pub use crate::sys::ERR_ALREADY_STARTED;
pub use crate::sys::ERR_BAD_HANDLE;
pub use crate::sys::ERR_BAD_LEN;
pub use crate::sys::ERR_BAD_PATH;
pub use crate::sys::ERR_BAD_STATE;
pub use crate::sys::ERR_BUSY;
pub use crate::sys::ERR_CANCELLED;
pub use crate::sys::ERR_CHANNEL_CLOSED;
pub use crate::sys::ERR_CHECKSUM_FAIL;
pub use crate::sys::ERR_CMD_UNKNOWN;
pub use crate::sys::ERR_CRC_FAIL;
pub use crate::sys::ERR_FAULT;
pub use crate::sys::ERR_GENERIC;
pub use crate::sys::ERR_I2C_NACK;
pub use crate::sys::ERR_INVALID_ARGS;
pub use crate::sys::ERR_IO;
pub use crate::sys::ERR_NOT_ALLOWED;
pub use crate::sys::ERR_NOT_BLOCKED;
pub use crate::sys::ERR_NOT_CONFIGURED;
pub use crate::sys::ERR_NOT_DIR;
pub use crate::sys::ERR_NOT_ENOUGH_BUFFER;
pub use crate::sys::ERR_NOT_FILE;
pub use crate::sys::ERR_NOT_FOUND;
pub use crate::sys::ERR_NOT_IMPLEMENTED;
pub use crate::sys::ERR_NOT_MOUNTED;
pub use crate::sys::ERR_NOT_READY;
pub use crate::sys::ERR_NOT_SUPPORTED;
pub use crate::sys::ERR_NOT_SUSPENDED;
pub use crate::sys::ERR_NOT_VALID;
pub use crate::sys::ERR_NO_MEMORY;
pub use crate::sys::ERR_NO_MSG;
pub use crate::sys::ERR_NO_RESOURCES;
pub use crate::sys::ERR_OBJECT_DESTROYED;
pub use crate::sys::ERR_OFFLINE;
pub use crate::sys::ERR_OUT_OF_RANGE;
pub use crate::sys::ERR_PARTIAL_WRITE;
pub use crate::sys::ERR_RECURSE_TOO_DEEP;
pub use crate::sys::ERR_THREAD_DETACHED;
pub use crate::sys::ERR_TIMED_OUT;
pub use crate::sys::ERR_TOO_BIG;
