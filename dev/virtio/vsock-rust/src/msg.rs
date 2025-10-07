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

#![allow(dead_code)]

use crate::sys::virtio_msg as VirtioMsg;
use crate::sys::virtio_msg_ffa as VirtioMsgFFA;
use arm_ffa::ARM_FFA_MSG_EXTENDED_ARGS_COUNT;
use core::mem::align_of;
use core::mem::offset_of;
use core::mem::size_of;
use rust_support::uuid::Uuid;
use static_assertions::const_assert;
use virtio_drivers_and_devices::PhysAddr;

#[cfg(all(feature = "virtio_device_side", feature = "virtio_driver_side"))]
compile_error!(
    "Features 'virtio_device_side' and 'virtio_driver_side' cannot be enabled simultaneously."
);

#[cfg(feature = "virtio_device_side")]
mod device;
#[cfg(feature = "virtio_driver_side")]
mod driver;

// virtio-msg spec 4.3.1: To refer to a specific address in one of the shared area, both sides
// are using a 64bit “bus address” (in Linux this is represented by the type dma_addr_t) which
// is formed of the area numeric ID and the offset in that area in the following way:
//   - Bit 63-56: Area numerical ID
//   - Bit 56-0: Offset in the Area
// We treat physical address arguments and return values in the dma HALs as bus addresses so we
// define this as PhysAddr (usize) instead of u64.
const_assert!(size_of::<BusAddress>() == size_of::<u64>());
type BusAddress = PhysAddr;
type AreaId = u8;

fn area_id_and_offset(bus_addr: BusAddress) -> (AreaId, u64) {
    let area_id = bus_addr >> 56;
    let area_offset = bus_addr & ((1 << 56) - 1);
    (area_id as u8, area_offset as u64)
}

#[cfg(feature = "virtio_driver_side")]
fn bus_address(area_id: AreaId, offset: u64) -> BusAddress {
    (BusAddress::from(area_id) << 56) | (offset as BusAddress)
}

// virtio-msg over FF-A only supports up to 255 shared memory regions
#[cfg(feature = "virtio_device_side")]
const MAX_NUM_SHM: usize = 255;

const VIRTIO_MSG_FFA_UUID: Uuid =
    Uuid::new(0xc66028b5, 0x2498, 0x4aa1, [0x9d, 0xe7, 0x77, 0xda, 0x61, 0x22, 0xab, 0xf0]);

// Verify some of the assumptions of the safety comments below.
const_assert!(size_of::<VirtioMsgFFA>() == size_of::<VirtioMsg>());
const_assert!(size_of::<VirtioMsgFFA>() == 40);
const_assert!(align_of::<VirtioMsgFFA>() == align_of::<VirtioMsg>());
const_assert!(offset_of!(VirtioMsgFFA, type_) == offset_of!(VirtioMsg, type_));
const_assert!(offset_of!(VirtioMsgFFA, id) == offset_of!(VirtioMsg, id));
const_assert!(
    offset_of!(VirtioMsgFFA, __bindgen_anon_1) == offset_of!(VirtioMsg, __bindgen_anon_1)
);
const_assert!(size_of::<VirtioMsgFFA>() <= ARM_FFA_MSG_EXTENDED_ARGS_COUNT * size_of::<u64>());

impl VirtioMsg {
    fn from_bytes(buf: &[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]) -> &VirtioMsg {
        let buf = buf as *const [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT] as *const VirtioMsg;
        // SAFETY:
        // - The input reference `buf` is valid for the lifetime of this function.
        // - The returned reference has the same lifetime as the input reference by elision rules.
        // - `VirtioMsg` and `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` have compatible layouts:
        //   - `VirtioMsg` is a packed struct with a size of 40 bytes and specific field offsets.
        //   - `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` is a 112 byte (14 * 8) array.
        // - The layout of `VirtioMsg` is such that its size and fields match the first 40 bytes of the input array.
        // - `VirtioMsg` and `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` have compatible alignments.
        // - Therefore, reinterpreting the first 40 bytes of the input array reference as `VirtioMsg` is safe.
        unsafe { buf.as_ref().unwrap() }
    }

    fn from_bytes_mut(buf: &mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]) -> &mut VirtioMsg {
        let buf = buf as *mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT] as *mut VirtioMsg;
        // SAFETY:
        // - Same safety considerations as `VirtioMsg::from_bytes` apply. The
        //   mutability does not affect the validity of the reinterpretation.
        // - `VirtioMsg` is a packed struct.
        unsafe { buf.as_mut().unwrap() }
    }
}

impl VirtioMsgFFA {
    fn from_bytes(buf: &[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]) -> &VirtioMsgFFA {
        let buf = buf as *const [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT] as *const VirtioMsgFFA;
        // SAFETY:
        // - The input reference `buf` is valid for the lifetime of this function.
        // - `VirtioMsgFFA` and `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` have compatible layouts:
        //   - `VirtioMsgFFA` is a packed struct with a size of 40 bytes and specific field offsets.
        //   - `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` is a 112 byte (14 * 8) array.
        // - The layout of `VirtioMsgFFA` is such that its size and fields match the first 40 bytes of the input array.
        // - `VirtioMsgFFA` and `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` have compatible alignments.
        // - Therefore, reinterpreting the first 40 bytes of the input array reference as `VirtioMsg` is safe.
        unsafe { buf.as_ref().unwrap() }
    }

    fn from_bytes_mut(buf: &mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]) -> &mut VirtioMsgFFA {
        let buf = buf as *mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT] as *mut VirtioMsgFFA;
        // SAFETY:
        // - Same safety considerations as `VirtioMsgFFA::from_bytes` apply. The
        //   mutability does not affect the validity of the reinterpretation.
        // - `VirtioMsgFFA` is a packed struct.
        unsafe { buf.as_mut().unwrap() }
    }
}
