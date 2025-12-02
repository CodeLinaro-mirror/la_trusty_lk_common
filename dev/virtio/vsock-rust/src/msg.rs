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

use crate::sys_dev2;
use arm_ffa::ARM_FFA_MSG_EXTENDED_ARGS_COUNT;
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

// virtio-msg-ffa spec 4.2 Bus Address Format
// A bus address is a 64-bit value used by the device endpoint to reference a specific offset within
// a shared memory area.
//
// - Area ID (16 bits): The identifier assigned by the driver when the memory region is shared
// - Offset (48 bits): A byte offset from the start of the shared area
//
// This driver treats physical address arguments and return values in the dma HALs as bus addresses
// so we define this as PhysAddr (usize) instead of u64.
const_assert!(size_of::<BusAddress>() == size_of::<u64>());
type BusAddress = PhysAddr;
type AreaId = u16;
const BUS_ADDR_AREA_ID_SHIFT: usize = 48;

fn area_id_and_offset(bus_addr: BusAddress) -> (AreaId, u64) {
    let area_id = bus_addr >> BUS_ADDR_AREA_ID_SHIFT;
    let area_offset = bus_addr & ((1 << BUS_ADDR_AREA_ID_SHIFT) - 1);
    (area_id as AreaId, area_offset as u64)
}

#[cfg(feature = "virtio_msg_driver")]
fn bus_address(area_id: AreaId, offset: u64) -> BusAddress {
    assert!(offset < 1 << BUS_ADDR_AREA_ID_SHIFT);
    (BusAddress::from(area_id) << BUS_ADDR_AREA_ID_SHIFT) | (offset as BusAddress)
}

// virtio-msg over FF-A only supports up to 255 shared memory regions
#[cfg(feature = "virtio_msg_device")]
const MAX_NUM_SHM: usize = 255;

const VIRTIO_MSG_FFA_UUID: Uuid =
    Uuid::new(0xc66028b5, 0x2498, 0x4aa1, [0x9d, 0xe7, 0x77, 0xda, 0x61, 0x22, 0xab, 0xf0]);

// virtio-msg spec 7.2: Total length of the message in bytes, include the 6-byte header.
// Must be between 6 and 96.
const MAX_VIRTIO_MSG_SIZE: usize = 96;

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
