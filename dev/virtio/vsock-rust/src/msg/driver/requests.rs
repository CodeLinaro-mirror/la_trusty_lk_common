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

use crate::sys;
#[cfg(feature = "virtio_msg_min_spec_version_dev2")]
use crate::sys::VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_TX_SUPP;
use crate::sys::VIRTIO_MSG_TYPE_RESPONSE;

use crate::msg::{MemShareAttr, MAX_VIRTIO_MSG_SIZE};
use crate::VsockVirtioFeatures;
use arm_ffa::ARM_FFA_MSG_EXTENDED_ARGS_COUNT;
use core::mem::{size_of, size_of_val};
use core::ptr::{read_unaligned, write_unaligned};
use rust_support::mmu::ArchMmuFlags;
use rust_support::Error as LkError;
use virtio_drivers_and_devices::transport::DeviceStatus;
use zerocopy::{FromBytes, IntoBytes, KnownLayout};

type Result<T> = core::result::Result<T, LkError>;

#[derive(Debug)]
pub struct VirtioMsgReq([u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]);

impl VirtioMsgReq {
    /// Creates a VirtioMsgReq using the virtio_msg struct from the new bindings with no payload.
    ///
    /// If `dev_id` is `None` the buffer is initialized as a bus message.
    fn new_v2_req(msg_id: u32, dev_id: Option<u16>) -> Self {
        // Use a no-op closure for `init_fn` with an arbitrary payload type.
        Self::new_v2_req_with_payload(msg_id, dev_id, |_: &mut ()| {})
    }

    /// Creates a VirtioMsgReq using the virtio_msg struct from the new bindings.
    ///
    /// If `dev_id` is `None` the buffer is initialized as a bus message.  Unlike the constructors
    /// for the old bindings this function's `init_fn` takes a mutable reference to the payload
    /// rather than the entire virtio_msg struct.
    fn new_v2_req_with_payload<T: KnownLayout + FromBytes + IntoBytes, F: FnOnce(&mut T)>(
        msg_id: u32,
        dev_id: Option<u16>,
        init_fn: F,
    ) -> Self {
        let mut buf = [0; ARM_FFA_MSG_EXTENDED_ARGS_COUNT];
        let buf_size = size_of_val(&buf);
        let msg = sys::virtio_msg::from_bytes_mut(&mut buf);
        msg.msg_id = u8::try_from(msg_id).unwrap();
        match dev_id {
            Some(id) => {
                msg.dev_id = id;
                // MBZ for device messages
                msg.type_ = 0;
            }
            None => {
                // MBZ for bus messages
                msg.dev_id = 0;
                msg.type_ = sys::VIRTIO_MSG_TYPE_BUS as u8;
            }
        };
        // Cannot be const since `T` is a generic on the function, but the assertion should be
        // optimized out.
        let payload_size = size_of::<T>();
        let total_size = size_of_val(msg) + payload_size;
        // virtio-msg spec 7.2: Total length of the message in bytes, include the 6-byte header.
        // Must be between 6 and 96.
        assert!(total_size < MAX_VIRTIO_MSG_SIZE);

        // conversion to u16 should not fail since we asserted the max payload size in the spec
        msg.msg_size = total_size.try_into().unwrap();

        // SAFETY: The `payload` field on `msg` is right after the header fields and this creates a
        // `&mut [u8]` of the remaining portion of the buffer from which `msg` is derived.
        let payload_slice = unsafe { msg.payload.as_mut_slice(buf_size - size_of_val(msg)) };

        // The payload slice is at least as big as `T` since we asserted the total size above so
        // this should never panic. We use the _from_prefix method since the payload may be smaller
        // than the slice.
        let (payload_ref, _remainder) = FromBytes::mut_from_prefix(payload_slice).unwrap();
        init_fn(payload_ref);

        Self(buf)
    }

    // TODO: Add arguments for direct and indirect message support once Trusty has the option.
    #[cfg(feature = "virtio_msg_min_spec_version_dev2")]
    pub fn new_bus_ffa_version(driver_version: u32, vmsg_revision: u32, num_shm: u16) -> Self {
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_FFA_BUS_VERSION,
            None,
            |payload: &mut sys::bus_ffa_version| {
                payload.driver_version = driver_version;
                payload.vmsg_revision = vmsg_revision;
                payload.vmsg_features = 0;
                // Trusty VMs only support sending direct messages so just hard-code this
                payload.features = VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_TX_SUPP;
                payload.area_num = num_shm;
            },
        )
    }

    pub fn new_bus_get_devices(offset: u16, num_devs: u16) -> Self {
        // virtio-msg spec 4.4.7.1: The offset and number of device numbers requested MUST be
        // multiples of 8.
        assert!(offset.is_multiple_of(8));
        assert!(num_devs.is_multiple_of(8));
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_BUS_GET_DEVICES,
            None,
            |payload: &mut sys::bus_get_devices| {
                payload.offset = offset;
                payload.num = num_devs;
            },
        )
    }

    /// Get device info for the specified device
    pub fn new_get_device_info(dev_id: u16) -> Self {
        Self::new_v2_req(sys::VIRTIO_MSG_DEVICE_INFO, Some(dev_id))
    }

    pub fn new_set_device_status(dev_id: u16, status: DeviceStatus) -> Self {
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_SET_DEVICE_STATUS,
            Some(dev_id),
            |payload: &mut sys::set_device_status| {
                payload.status = status.bits();
            },
        )
    }

    pub fn new_get_device_status(dev_id: u16) -> Self {
        Self::new_v2_req(sys::VIRTIO_MSG_GET_DEVICE_STATUS, Some(dev_id))
    }

    pub fn new_get_device_features(dev_id: u16, index: u32, num_blocks: u32) -> Self {
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_GET_DEV_FEATURES,
            Some(dev_id),
            |payload: &mut sys::get_features| {
                payload.index = index;
                payload.num = num_blocks;
            },
        )
    }

    pub fn new_set_driver_features(dev_id: u16, index: u32, features: u64) -> Self {
        let mut req = Self::new_v2_req(sys::VIRTIO_MSG_SET_DRV_FEATURES, Some(dev_id));
        let msg = sys::virtio_msg::from_bytes_mut(&mut req.0);

        let payload_size = size_of::<sys::set_features>() + size_of::<u64>();
        let total_size = size_of::<sys::virtio_msg>() + payload_size;
        msg.msg_size = total_size.try_into().unwrap();

        // This pointer may be unaligned since the virtio_msg struct is packed
        let payload_ptr = msg.payload.as_mut_ptr().cast::<sys::set_features>();

        // Feature blocks are groups of 32 bits
        let num_blocks = size_of::<VsockVirtioFeatures>() / size_of::<u32>();
        let payload =
            sys::set_features { index, num: num_blocks.try_into().unwrap(), ..Default::default() };
        // SAFETY: payload_ptr is non-null and points to a byte buffer at least as big as
        // `set_features`.
        unsafe { write_unaligned(payload_ptr, payload) }
        // This pointer may be unaligned since the set_features struct is packed
        // SAFETY: payload_ptr is non-null and points to a valid bus_get_devices_resp struct
        let features_ptr = unsafe { &raw mut (*payload_ptr).features };
        // SAFETY: features_ptr is non-null and points to a byte buffer at least as big as `u64`.
        unsafe { write_unaligned(features_ptr.cast::<u64>(), features) }

        req
    }

    pub fn new_get_config(dev_id: u16, offset: u32, size: u8) -> Self {
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_GET_CONFIG,
            Some(dev_id),
            |payload: &mut sys::get_config| {
                payload.offset = offset;
                payload.size = u32::from(size);
            },
        )
    }

    pub fn new_get_vqueue(dev_id: u16, queue: u16) -> Self {
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_GET_VQUEUE,
            Some(dev_id),
            |payload: &mut sys::get_vqueue| {
                payload.index = u32::from(queue);
            },
        )
    }

    pub fn new_set_vqueue(
        dev_id: u16,
        queue: u16,
        size: u32,
        descriptor_addr: u64,
        driver_addr: u64,
        device_addr: u64,
    ) -> Self {
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_SET_VQUEUE,
            Some(dev_id),
            |payload: &mut sys::set_vqueue| {
                payload.index = u32::from(queue);
                payload.unused = 0;
                payload.size = size;
                payload.descriptor_addr = descriptor_addr;
                payload.driver_addr = driver_addr;
                payload.device_addr = device_addr;
            },
        )
    }

    pub fn new_bus_area_share(
        area_id: u16,
        mem_handle: u64,
        num_pages: usize,
        arch_mmu_flags: ArchMmuFlags,
    ) -> Self {
        let attr = MemShareAttr::from_lk_flags(arch_mmu_flags);
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_FFA_BUS_AREA_SHARE,
            None,
            |payload: &mut sys::bus_area_share| {
                payload.area_id = area_id;
                payload.mem_handle = mem_handle;
                payload.tag = 0;
                payload.count = u32::try_from(num_pages).unwrap();
                payload.attr = u32::from(attr);
            },
        )
    }

    pub fn new_event_avail(dev_id: u16, queue: u16) -> Self {
        Self::new_v2_req_with_payload(
            sys::VIRTIO_MSG_EVENT_AVAIL,
            Some(dev_id),
            |payload: &mut sys::event_avail| {
                payload.index = u32::from(queue);
                payload.next_offset_wrap = 0;
            },
        )
    }

    pub fn get_buf(&self) -> &[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT] {
        &self.0
    }
}

pub struct VirtioMsgResp {
    buf: [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT],
}

impl VirtioMsgResp {
    pub fn new(buf: [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]) -> Result<Self> {
        let resp = sys::virtio_msg::from_bytes(&buf);
        let msg_ty = u32::from(resp.type_);
        if msg_ty & VIRTIO_MSG_TYPE_RESPONSE == 0 {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        Ok(Self { buf })
    }

    /// Returns a copy of the payload in a virtio_msg response
    fn read_v2_resp<T: FromBytes>(self, msg_id: u32) -> Result<T> {
        let resp = sys::virtio_msg::from_bytes(&self.buf);
        if u32::from(resp.msg_id) != msg_id {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        let payload_size = size_of::<T>();
        let total_size = size_of_val(resp) + payload_size;
        // virtio-msg spec 7.2: Total length of the message in bytes, include the 6-byte header.
        // Must be between 6 and 96.
        assert!(total_size <= MAX_VIRTIO_MSG_SIZE);

        if usize::from(resp.msg_size) != total_size {
            return Err(LkError::ERR_INVALID_ARGS);
        }

        // SAFETY: The `payload` field on `resp` is right after the header fields and this creates a
        // `&[u8]` of the remaining portion of the buffer from which `resp` is derived.
        let payload_slice =
            unsafe { resp.payload.as_slice(size_of_val(&self.buf) - size_of_val(resp)) };

        // The payload slice is at least as big as `T` since we asserted the total size above so
        // this should never panic. We use the _from_prefix method since the payload may be smaller
        // than the slice.
        let (payload_copy, remainder) = FromBytes::read_from_prefix(payload_slice).unwrap();

        if remainder.iter().any(|&b| b != 0) {
            return Err(LkError::ERR_INVALID_ARGS);
        }

        Ok(payload_copy)
    }

    /// Returns the payload in a virtio_msg response for types which can't impl zerocopy traits.
    ///
    /// Returns a copy of the fixed-sized portion of a payload `T` and a slice of the subsection of
    /// the buffer containing the variably-sized portion of the payload. This is provided for types
    /// which cannot implement zerocopy traits and `Self::into_v2_resp` should be used if possible.
    pub fn read_v2_resp_variable_size<T>(&self, msg_id: u32) -> Result<(T, &[u8])> {
        let resp = sys::virtio_msg::from_bytes(&self.buf);
        if u32::from(resp.msg_id) != msg_id {
            return Err(LkError::ERR_INVALID_ARGS);
        }

        let fixed_payload_size = size_of::<T>();
        let total_fixed_size = size_of_val(resp) + fixed_payload_size;

        let resp_msg_size = usize::from(resp.msg_size);
        if resp_msg_size < total_fixed_size {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        let variable_payload_size = resp_msg_size - total_fixed_size;

        let payload_ptr: *const u8 = resp.payload.as_ptr();
        // SAFETY: payload_ptr is non-null and points to a byte buffer at least as big as `T`.
        let fixed_payload_copy = unsafe { read_unaligned(payload_ptr.cast::<T>()) };

        // SAFETY: The `payload` field on `resp` is right after the header fields and this creates a
        // `&[u8]` of the remaining portion of the buffer from which `resp` is derived.
        let payload_slice =
            unsafe { resp.payload.as_slice(size_of_val(&self.buf) - size_of_val(resp)) };

        // Only return the portion of the buffer containing the variably-sized payload.
        let variable_payload_end = fixed_payload_size + variable_payload_size;
        let variable_payload_slice = &payload_slice[fixed_payload_size..variable_payload_end];

        if payload_slice[variable_payload_end..].iter().any(|&b| b != 0) {
            return Err(LkError::ERR_INVALID_ARGS);
        }

        Ok((fixed_payload_copy, variable_payload_slice))
    }

    pub fn read_bus_ffa_version(self) -> Result<sys::bus_ffa_version_resp> {
        self.read_v2_resp(sys::VIRTIO_MSG_FFA_BUS_VERSION)
    }

    pub fn read_bus_get_devices(&self) -> Result<(sys::bus_get_devices_resp, &[u8])> {
        self.read_v2_resp_variable_size(sys::VIRTIO_MSG_BUS_GET_DEVICES)
    }

    pub fn read_get_device_info(self) -> Result<sys::get_device_info_resp> {
        self.read_v2_resp(sys::VIRTIO_MSG_DEVICE_INFO)
    }

    pub fn read_get_device_features(&self) -> Result<(sys::get_features_resp, &[u8])> {
        self.read_v2_resp_variable_size(sys::VIRTIO_MSG_GET_DEV_FEATURES)
    }

    pub fn read_get_device_status(self) -> Result<sys::get_device_status_resp> {
        self.read_v2_resp(sys::VIRTIO_MSG_GET_DEVICE_STATUS)
    }

    pub fn read_set_device_status(self) -> Result<sys::set_device_status_resp> {
        self.read_v2_resp(sys::VIRTIO_MSG_SET_DEVICE_STATUS)
    }

    pub fn read_get_config(&self) -> Result<(sys::get_config_resp, &[u8])> {
        self.read_v2_resp_variable_size(sys::VIRTIO_MSG_GET_CONFIG)
    }

    pub fn read_get_vqueue(self) -> Result<sys::get_vqueue_resp> {
        self.read_v2_resp(sys::VIRTIO_MSG_GET_VQUEUE)
    }

    pub fn read_bus_area_share(self) -> Result<sys::bus_area_share_resp> {
        self.read_v2_resp(sys::VIRTIO_MSG_FFA_BUS_AREA_SHARE)
    }
}
