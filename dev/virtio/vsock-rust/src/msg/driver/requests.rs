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

// glob import since we only allowlist virtio_msg.h, VirtioMsgFFA.h and virtio_config.h in bindgen
use crate::sys::*;
use crate::sys_dev2;

use crate::msg::{VirtioMsg, VirtioMsgFFA, MAX_VIRTIO_MSG_SIZE};
use arm_ffa::ARM_FFA_MSG_EXTENDED_ARGS_COUNT;
use core::mem::{size_of, size_of_val};
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
        let msg = sys_dev2::virtio_msg::from_bytes_mut(&mut buf);
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
                msg.type_ = sys_dev2::VIRTIO_MSG_TYPE_BUS as u8;
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

    fn new_req<F: FnOnce(&mut VirtioMsg)>(id: u32, dev_id: u16, init_fn: F) -> Self {
        let mut buf = [0; ARM_FFA_MSG_EXTENDED_ARGS_COUNT];
        let msg = VirtioMsg::from_bytes_mut(&mut buf);
        msg.id = u8::try_from(id).unwrap();
        msg.dev_id = dev_id;
        init_fn(msg);
        Self(buf)
    }

    fn new_ffa_req<F: FnOnce(&mut VirtioMsgFFA)>(id: u32, init_fn: F) -> Self {
        let mut buf = [0; ARM_FFA_MSG_EXTENDED_ARGS_COUNT];
        let msg = VirtioMsgFFA::from_bytes_mut(&mut buf);
        msg.type_ = VIRTIO_MSG_TYPE_BUS as u8;
        msg.id = u8::try_from(id).unwrap();
        init_fn(msg);
        Self(buf)
    }

    pub fn activate(driver_version: u32) -> Self {
        Self::new_ffa_req(VIRTIO_MSG_FFA_ACTIVATE, |msg| {
            msg.__bindgen_anon_1.bus_activate = bus_activate { driver_version };
        })
    }

    // TODO: Add arguments for direct and indirect message support once Trusty has the option.
    pub fn configure(num_shm: u8) -> Self {
        Self::new_ffa_req(VIRTIO_MSG_FFA_CONFIGURE, |msg| {
            // Trusty only support direct messages so just hard-code this
            let features =
                u64::from(VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_SUPP) | (u64::from(num_shm) << 8);
            msg.__bindgen_anon_1.bus_configure = bus_configure { features };
        })
    }

    /// Get device info for the specified device
    pub fn get_device_info(dev_id: u16) -> Self {
        Self::new_req(VIRTIO_MSG_DEVICE_INFO, dev_id, |_msg| {})
    }

    pub fn set_device_status(dev_id: u16, status: DeviceStatus) -> Self {
        Self::new_req(VIRTIO_MSG_SET_DEVICE_STATUS, dev_id, |msg| {
            let status = status.bits();
            msg.__bindgen_anon_1.set_device_status = set_device_status { status };
        })
    }

    pub fn get_device_status(dev_id: u16) -> Self {
        Self::new_req(VIRTIO_MSG_GET_DEVICE_STATUS, dev_id, |_msg| {})
    }

    pub fn get_features(dev_id: u16, index: u32) -> Self {
        Self::new_req(VIRTIO_MSG_GET_FEATURES, dev_id, |msg| {
            msg.__bindgen_anon_1.get_features = get_features { index };
        })
    }

    pub fn set_features(dev_id: u16, index: u32, features: [u64; 4]) -> Self {
        Self::new_req(VIRTIO_MSG_SET_FEATURES, dev_id, |msg| {
            msg.__bindgen_anon_1.set_features = set_features { index, features };
        })
    }

    pub fn get_config_gen(dev_id: u16) -> Self {
        Self::new_req(VIRTIO_MSG_GET_CONFIG_GEN, dev_id, |_msg| {})
    }

    pub fn get_config(dev_id: u16, offset: usize, size: u8) -> Self {
        Self::new_req(VIRTIO_MSG_GET_CONFIG, dev_id, |msg| {
            let offset = (offset as u64).to_le_bytes();
            let offset = [offset[1], offset[2], offset[3]];
            msg.__bindgen_anon_1.get_config = get_config { offset, size };
        })
    }

    pub fn get_vqueue(dev_id: u16, queue: u16) -> Self {
        Self::new_req(VIRTIO_MSG_GET_VQUEUE, dev_id, |msg| {
            let index = u32::from(queue);
            msg.__bindgen_anon_1.get_vqueue = get_vqueue { index };
        })
    }

    pub fn set_vqueue(
        dev_id: u16,
        queue: u16,
        size: u32,
        descriptor_addr: u64,
        driver_addr: u64,
        device_addr: u64,
    ) -> Self {
        Self::new_req(VIRTIO_MSG_SET_VQUEUE, dev_id, |msg| {
            let index = u32::from(queue);
            msg.__bindgen_anon_1.set_vqueue =
                set_vqueue { index, unused: 0, size, descriptor_addr, driver_addr, device_addr };
        })
    }

    pub fn area_share(area_id: u32, mem_handle: u64) -> Self {
        Self::new_ffa_req(VIRTIO_MSG_FFA_AREA_SHARE, |msg| {
            msg.__bindgen_anon_1.bus_area_share = bus_area_share { area_id, mem_handle };
        })
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
        let resp = VirtioMsgFFA::from_bytes(&buf);
        let msg_ty = u32::from(resp.type_);
        if msg_ty & VIRTIO_MSG_TYPE_RESPONSE == 0 {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        Ok(Self { buf })
    }

    fn is_bus_msg(&self) -> bool {
        let resp = VirtioMsgFFA::from_bytes(&self.buf);
        u32::from(resp.type_) & VIRTIO_MSG_TYPE_BUS != 0
    }

    fn into_resp(self, msg_ty: u32) -> Result<VirtioMsg> {
        if self.is_bus_msg() {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        let resp = VirtioMsg::from_bytes(&self.buf);
        if u32::from(resp.id) != msg_ty {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        Ok(*resp)
    }

    fn into_ffa_resp(self, msg_ty: u32) -> Result<VirtioMsgFFA> {
        if !self.is_bus_msg() {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        let resp = VirtioMsgFFA::from_bytes(&self.buf);
        if u32::from(resp.id) != msg_ty {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        Ok(*resp)
    }

    pub fn into_activate(self) -> Result<bus_activate_resp> {
        let resp = self.into_ffa_resp(VIRTIO_MSG_FFA_ACTIVATE)?;
        // SAFETY: `resp` is derived from an array of bytes which is sufficient
        // to initialize all union variants with valid values.
        Ok(unsafe { resp.__bindgen_anon_1.bus_activate_resp })
    }

    pub fn into_configure(self) -> Result<bus_configure_resp> {
        let resp = self.into_ffa_resp(VIRTIO_MSG_FFA_CONFIGURE)?;
        // SAFETY: `resp` is derived from an array of bytes which is sufficient
        // to initialize all union variants with valid values.
        Ok(unsafe { resp.__bindgen_anon_1.bus_configure_resp })
    }

    pub fn into_get_device_info(self) -> Result<get_device_info_resp> {
        let resp = self.into_resp(VIRTIO_MSG_DEVICE_INFO)?;
        // SAFETY: `resp` is derived from an array of bytes which is sufficient
        // to initialize all union variants with valid values.
        Ok(unsafe { resp.__bindgen_anon_1.get_device_info_resp })
    }

    pub fn into_get_features(self) -> Result<get_features_resp> {
        let resp = self.into_resp(VIRTIO_MSG_GET_FEATURES)?;
        // SAFETY: `resp` is derived from an array of bytes which is sufficient
        // to initialize all union variants with valid values.
        Ok(unsafe { resp.__bindgen_anon_1.get_features_resp })
    }

    pub fn into_get_device_status(self) -> Result<get_device_status_resp> {
        let resp = self.into_resp(VIRTIO_MSG_GET_DEVICE_STATUS)?;
        // SAFETY: `resp` is derived from an array of bytes which is sufficient
        // to initialize all union variants with valid values.
        Ok(unsafe { resp.__bindgen_anon_1.get_device_status_resp })
    }

    pub fn into_get_config_gen(self) -> Result<get_config_gen_resp> {
        let resp = self.into_resp(VIRTIO_MSG_GET_CONFIG_GEN)?;
        // SAFETY: `resp` is derived from an array of bytes which is sufficient
        // to initialize all union variants with valid values.
        Ok(unsafe { resp.__bindgen_anon_1.get_config_gen_resp })
    }

    pub fn into_get_config(self) -> Result<get_config_resp> {
        let resp = self.into_resp(VIRTIO_MSG_GET_CONFIG)?;
        // SAFETY: `resp` is derived from an array of bytes which is sufficient
        // to initialize all union variants with valid values.
        Ok(unsafe { resp.__bindgen_anon_1.get_config_resp })
    }

    pub fn into_get_vqueue(self) -> Result<get_vqueue_resp> {
        let resp = self.into_resp(VIRTIO_MSG_GET_VQUEUE)?;
        // SAFETY: `resp` is derived from an array of bytes which is sufficient
        // to initialize all union variants with valid values.
        Ok(unsafe { resp.__bindgen_anon_1.get_vqueue_resp })
    }
}
