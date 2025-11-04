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
use crate::sys_dev2;
use arm_ffa::ARM_FFA_MSG_EXTENDED_ARGS_COUNT;
use core::mem::align_of;
use core::mem::offset_of;
use core::mem::size_of;
use peer_id::Uuid;
use rust_support::mmu::ArchMmuFlags;
use static_assertions::const_assert;
use virtio_drivers_and_devices::PhysAddr;

#[cfg(all(feature = "virtio_msg_device", feature = "virtio_msg_driver"))]
compile_error!(
    "Features 'virtio_msg_device' and 'virtio_msg_driver' cannot be enabled simultaneously."
);

#[cfg(feature = "virtio_msg_device")]
mod device;
#[cfg(feature = "virtio_msg_driver")]
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

#[cfg(feature = "virtio_msg_driver")]
fn bus_address(area_id: AreaId, offset: u64) -> BusAddress {
    (BusAddress::from(area_id) << 56) | (offset as BusAddress)
}

// virtio-msg over FF-A only supports up to 255 shared memory regions
#[cfg(feature = "virtio_msg_device")]
const MAX_NUM_SHM: usize = 255;

const VIRTIO_MSG_FFA_UUID: Uuid =
    Uuid::new(0xc66028b5, 0x2498, 0x4aa1, [0x9d, 0xe7, 0x77, 0xda, 0x61, 0x22, 0xab, 0xf0]);

// virtio-msg spec 7.2: Total length of the message in bytes, include the 6-byte header.
// Must be between 6 and 96.
const MAX_VIRTIO_MSG_SIZE: usize = 96;

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

impl sys_dev2::virtio_msg {
    fn from_bytes(buf: &[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]) -> &Self {
        let buf = buf.as_ptr().cast::<Self>();
        // SAFETY:
        // - The input reference `buf` is valid for the lifetime of this function.
        // - The returned reference has the same lifetime as the input reference by elision rules.
        // - `Self` and `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` have compatible layouts:
        //   - `Self` is a packed struct with a size of 6 bytes and specific field offsets. It is
        //     terminated by a bindgen-generated ZST representing a flexible array member which may
        //     only be accessed as a `&[u8]` via unsafe functions where the caller has to ensure the
        //     offset into it is valid for the buffer from which the virtio_msg struct is derived.
        //   - `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` is a 112 byte (14 * 8) array.
        // - The layout of `Self` is such that its size and fields match the first 6 bytes of the input array.
        // - `Self` and `[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]` have compatible alignments.
        // - Therefore, reinterpreting the first 6 bytes of the input array reference as `Self` is safe.
        unsafe { buf.as_ref().unwrap() }
    }

    fn from_bytes_mut(buf: &mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]) -> &mut Self {
        let buf = buf.as_mut_ptr().cast::<Self>();
        // SAFETY:
        // - Same safety considerations as `Self::from_bytes` apply. The
        //   mutability does not affect the validity of the reinterpretation.
        // - `Self` is a packed struct.
        unsafe { buf.as_mut().unwrap() }
    }
}

// The virtio-msg-ffa spec describes shared memory attributes using a different format than FF-A
// memory attributes. The following defines a MemShareAttr to describe these attributes and allows
// converting LK's ArchMmuFlags to it and converting MemShareAttr to its u32 representation.
#[repr(u32)]
enum ShareType {
    Share = 0b00,
    Lend = 0b01,
    Donate = 0b10,
}

#[allow(clippy::enum_variant_names)]
#[repr(u32)]
enum Shareability {
    NonShareable = 0b00,
    OuterShareable = 0b10,
    InnerShareable = 0b11,
}

#[repr(u32)]
enum Cacheability {
    NonCacheable = 0b01,
    WriteBack = 0b11,
}

#[repr(u32)]
enum DeviceMemoryAttr {
    nGnRnE = 0b00,
    nGnRE = 0b01,
    nGRE = 0b10,
    GRE = 0b11,
}

enum MemoryType {
    Device(DeviceMemoryAttr),
    Normal(Cacheability),
}

impl MemoryType {
    const DEVICE: u32 = 0b01;
    const NORMAL: u32 = 0b10;
}

struct MemShareAttr {
    share_type: ShareType,
    read_write: bool,
    executable: bool,
    shareability: Shareability,
    memory_type: MemoryType,
    non_secure: bool,
}

impl MemShareAttr {
    const WRITEABLE_SHIFT: u32 = 2;
    const EXECUTABLE_SHIFT: u32 = 3;
    const SHAREABILITY_SHIFT: u32 = 4;
    const CACHEABILITY_SHIFT: u32 = 6;
    const DEVICE_MEMORY_ATTR_SHIFT: u32 = 6;
    const MEMORY_TYPE_SHIFT: u32 = 8;
    const NS_BIT_SHIFT: u32 = 10;
}

impl From<MemShareAttr> for u32 {
    fn from(attr: MemShareAttr) -> u32 {
        let mut res = 0;
        res |= attr.share_type as u32;
        if attr.read_write {
            res |= 1 << MemShareAttr::WRITEABLE_SHIFT;
        }
        if attr.executable {
            res |= 1 << MemShareAttr::EXECUTABLE_SHIFT;
        }
        res |= (attr.shareability as u32) << MemShareAttr::SHAREABILITY_SHIFT;
        match attr.memory_type {
            MemoryType::Normal(cacheability) => {
                res |= MemoryType::NORMAL << MemShareAttr::MEMORY_TYPE_SHIFT;
                res |= (cacheability as u32) << MemShareAttr::CACHEABILITY_SHIFT;
            }
            MemoryType::Device(device_memory_attr) => {
                res |= MemoryType::DEVICE << MemShareAttr::MEMORY_TYPE_SHIFT;
                res |= (device_memory_attr as u32) << MemShareAttr::DEVICE_MEMORY_ATTR_SHIFT;
            }
        }
        if attr.non_secure {
            res |= 1 << MemShareAttr::NS_BIT_SHIFT;
        }
        res
    }
}

impl MemShareAttr {
    fn from_lk_flags(flags: ArchMmuFlags) -> Self {
        let non_secure = flags.contains(ArchMmuFlags::NS);
        let read_write = !flags.contains(ArchMmuFlags::PERM_RO);
        let executable = !flags.contains(ArchMmuFlags::PERM_NO_EXECUTE);
        let (shareability, memory_type) = if flags.contains(ArchMmuFlags::UNCACHED_DEVICE) {
            (Shareability::NonShareable, MemoryType::Device(DeviceMemoryAttr::nGnRE))
        } else if flags.contains(ArchMmuFlags::UNCACHED) {
            (Shareability::NonShareable, MemoryType::Normal(Cacheability::NonCacheable))
        } else {
            (Shareability::InnerShareable, MemoryType::Normal(Cacheability::WriteBack))
        };
        Self {
            share_type: ShareType::Share,
            read_write,
            executable,
            shareability,
            memory_type,
            non_secure,
        }
    }
}
