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

// TODO: replace with `trusty-log` crate once it is `no_std`-compatible

use core::ffi::c_uint;
use core::ffi::c_ulong;
use core::ffi::c_void;
use core::fmt;
use core::fmt::Result;
use core::fmt::Write;
use core::format_args;
use log::{LevelFilter, Log, Metadata, Record};

use crate::init::lk_init_level;
use crate::LK_INIT_HOOK;

use crate::sys::fflush;
use crate::sys::fwrite;
use crate::sys::lk_stderr;
use crate::sys::LK_LOGLEVEL_RUST;

static TRUSTY_LOGGER: TrustyKernelLogger = TrustyKernelLogger;

pub struct TrustyKernelLogger;

// The core::fmt::Write methods used to print formatted logs take a `&mut Self` so if
// TrustyKernelLogger were to implement them they could not be called from Log::log. Instead we
// define a private, stateless type to implement Write.
pub(super) struct TrustyKernelWriter;

impl Write for TrustyKernelWriter {
    fn write_str(&mut self, msg: &str) -> Result {
        let msg = msg.as_bytes();
        // rust formatting should not insert nulls into msg, but avoid printing messages with
        // internal null bytes in case the fwrite implementation assumes that the pointer contains
        // no internal null bytes.
        if msg.contains(&0) {
            return Err(fmt::Error);
        }
        // Safety: The pointer returned by `msg.as_ptr()` is valid for the duration of the `fwrite`
        // call and it doesn't need to be null-terminated since we're passing the message length as
        // the `count` argument to `fwrite`.
        unsafe {
            fwrite(
                msg.as_ptr().cast::<c_void>(),
                size_of::<u8>() as c_ulong,
                msg.len().try_into().unwrap(),
                lk_stderr(),
            );
        }
        Ok(())
    }
}

impl Log for TrustyKernelLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut writer = TrustyKernelWriter;
            // Use format_args! instead of format! and print with a method from the Write trait to
            // avoid heap allocations.
            writer.write_fmt(format_args!("{} - {}\n", record.level(), record.args())).ok();
        }
    }

    fn flush(&self) {
        // Safety:
        // `lk_stderr()` returns a FILE pointer that is valid or null.
        unsafe { fflush(lk_stderr()) };
    }
}

/// Initialize logging for Rust in the kernel
///
/// By default, only warnings and errors are logged (even in debug builds).
///
/// The log level (`LK_LOGLEVEL_RUST`) is controlled by these make variables:
/// - `LOG_LEVEL_KERNEL_RUST` if set,
/// - `LOG_LEVEL_KERNEL` if set, and
/// - `DEBUG` otherwise.
///
/// Values below (above) expected values sets the log level to off (trace).
extern "C" fn kernel_log_init_func(_level: c_uint) {
    log::set_logger(&TRUSTY_LOGGER).unwrap();
    // Level or LevelFilter cannot be created directly from integers
    // https://github.com/rust-lang/log/issues/460
    //
    // bindgen emits `LK_LOGLEVEL_RUST` as `u32` when the value is
    // a positive integer and omits it otherwise thus causing the
    // build to fail.
    log::set_max_level(match LK_LOGLEVEL_RUST {
        0 => LevelFilter::Off,
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn, // the default for Trusty
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        _ => LevelFilter::Trace, // enable trace! at 5+
    });
}

LK_INIT_HOOK!(kernel_log_init, kernel_log_init_func, lk_init_level::LK_INIT_LEVEL_HEAP);
