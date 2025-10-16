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

use crate::msg::device::VirtQueue;
use crate::msg::device::VSOCK_QUEUE_SIZE;
use crate::msg::{VirtioMsg, VirtioMsgFFA};
use crate::sys_dev2;
use crate::sys_dev2::{VIRTIO_MSG_TYPE_BUS, VIRTIO_MSG_TYPE_RESPONSE};
use crate::VsockVirtioFeatures;
use arm_ffa::ARM_FFA_MSG_EXTENDED_ARGS_COUNT;
use core::mem::{size_of, size_of_val};
use core::ptr::{read_unaligned, write, write_unaligned};
use rust_support::Error as LkError;
use virtio_drivers_and_devices::transport::DeviceType;
use zerocopy::{FromBytes, IntoBytes, KnownLayout};

pub struct VirtioMsgReq<'a> {
    buf: &'a mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT],
}

pub enum VirtioMsgPayload {
    BusFFAVersion(sys_dev2::bus_ffa_version),
    BusGetDevices(sys_dev2::bus_get_devices),
    GetDeviceInfo,
    SetDeviceStatus(sys_dev2::set_device_status),
    GetDeviceStatus,
    GetFeatures(sys_dev2::get_features),
    SetFeatures((sys_dev2::set_features, VsockVirtioFeatures)),
    GetVqueue(sys_dev2::get_vqueue),
    SetVqueue(sys_dev2::set_vqueue),
    GetConfig(sys_dev2::get_config),
    AreaShare(sys_dev2::bus_area_share),
    AreaUnshare(sys_dev2::bus_area_unshare),
    // Contains the ID for a bus request not defined by the virtio-msg spec
    UnknownBusReq(u32),
    // Contains the ID for a virtio request not defined by the virtio-msg spec
    UnknownDeviceReq(u32),
}

impl VirtioMsgReq<'_> {
    pub fn parse(
        buf: &mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT],
    ) -> Result<VirtioMsgReq<'_>, LkError> {
        let req = VirtioMsgFFA::from_bytes(buf);
        let msg_ty = u32::from(req.type_);
        if msg_ty & VIRTIO_MSG_TYPE_RESPONSE != 0 {
            return Err(LkError::ERR_INVALID_ARGS);
        }
        Ok(VirtioMsgReq { buf })
    }

    pub fn is_bus_msg(&self) -> bool {
        let req = sys_dev2::virtio_msg::from_bytes(self.buf);
        // Check whether the request is a bus or virtio message
        u32::from(req.type_) & VIRTIO_MSG_TYPE_BUS != 0
    }

    fn get_v2_payload<T: FromBytes>(&self) -> T {
        let req = sys_dev2::virtio_msg::from_bytes(self.buf);
        // SAFETY: The `payload` field on `req` is right after the header fields and this creates a
        // `&[u8]` of the remaining portion of the buffer from which `req` is derived.
        let payload_slice =
            unsafe { req.payload.as_slice(size_of_val(self.buf) - size_of_val(req)) };

        // The payload slice is at least as big as any virtio-msg request which we handle so this
        // should not panic
        let (payload_copy, _remainder) = FromBytes::read_from_prefix(payload_slice).unwrap();
        // TODO: Check that _remainder is zeroed out
        payload_copy
    }

    fn get_v2_payload_variable_size<T, const N: usize>(&self) -> (T, [u8; N]) {
        let req = sys_dev2::virtio_msg::from_bytes(self.buf);

        let payload_ptr: *const u8 = req.payload.as_ptr();
        // SAFETY: payload_ptr is non-null and points to a buffer at least as bit as `T`.
        let fixed_payload_copy = unsafe { read_unaligned(payload_ptr.cast::<T>()) };

        // SAFETY: The `payload` field on `req` is right after the header fields and this creates a
        // `&[u8]` of the remaining portion of the buffer from which `req` is derived.
        let payload_slice =
            unsafe { req.payload.as_slice(size_of_val(self.buf) - size_of_val(req)) };

        let fixed_payload_size = size_of::<T>();
        let total_fixed_size = size_of_val(req) + fixed_payload_size;
        let variable_payload_size = usize::from(req.msg_size) - total_fixed_size;
        let variable_payload_end = fixed_payload_size + variable_payload_size;
        let variable_payload_slice = &payload_slice[fixed_payload_size..variable_payload_end];

        // TODO: Return a Result here instead of unwrapping
        (fixed_payload_copy, variable_payload_slice.try_into().unwrap())
    }

    pub fn get_msg_payload(&self) -> VirtioMsgPayload {
        let req = sys_dev2::virtio_msg::from_bytes(self.buf);

        // The request ID determines which field is valid in the payload union. Note that a given ID
        // may represent different requests depending on whether it's a bus or virtio message.
        let id = u32::from(req.msg_id);

        if self.is_bus_msg() {
            match id {
                sys_dev2::VIRTIO_MSG_FFA_BUS_VERSION => {
                    VirtioMsgPayload::BusFFAVersion(self.get_v2_payload())
                }
                sys_dev2::VIRTIO_MSG_BUS_GET_DEVICES => {
                    VirtioMsgPayload::BusGetDevices(self.get_v2_payload())
                }
                sys_dev2::VIRTIO_MSG_FFA_BUS_AREA_SHARE => {
                    VirtioMsgPayload::AreaShare(self.get_v2_payload())
                }
                sys_dev2::VIRTIO_MSG_FFA_BUS_AREA_UNSHARE => {
                    VirtioMsgPayload::AreaUnshare(self.get_v2_payload())
                }
                _ => VirtioMsgPayload::UnknownBusReq(id),
            }
        } else {
            match id {
                // GET_DEVICE_INFO requests don't use the payload
                sys_dev2::VIRTIO_MSG_DEVICE_INFO => VirtioMsgPayload::GetDeviceInfo,
                sys_dev2::VIRTIO_MSG_SET_DEVICE_STATUS => {
                    VirtioMsgPayload::SetDeviceStatus(self.get_v2_payload())
                }
                // GET_DEVICE_STATUS requests don't use the payload
                sys_dev2::VIRTIO_MSG_GET_DEVICE_STATUS => VirtioMsgPayload::GetDeviceStatus,
                sys_dev2::VIRTIO_MSG_GET_DEV_FEATURES => {
                    VirtioMsgPayload::GetFeatures(self.get_v2_payload())
                }
                sys_dev2::VIRTIO_MSG_SET_DRV_FEATURES => {
                    let (set_features_header, set_features_data) =
                        self.get_v2_payload_variable_size();
                    let feature_data = u64::from_le_bytes(set_features_data);
                    VirtioMsgPayload::SetFeatures((set_features_header, feature_data))
                }
                sys_dev2::VIRTIO_MSG_GET_VQUEUE => {
                    VirtioMsgPayload::GetVqueue(self.get_v2_payload())
                }
                sys_dev2::VIRTIO_MSG_SET_VQUEUE => {
                    VirtioMsgPayload::SetVqueue(self.get_v2_payload())
                }
                sys_dev2::VIRTIO_MSG_GET_CONFIG => {
                    VirtioMsgPayload::GetConfig(self.get_v2_payload())
                }
                _ => VirtioMsgPayload::UnknownDeviceReq(id),
            }
        }
    }
}

pub struct VirtioMsgResp<'a> {
    buf: &'a mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT],
}

impl VirtioMsgResp<'_> {
    pub fn new(req: VirtioMsgReq) -> VirtioMsgResp {
        let buf = req.buf;
        // Set the RESPONSE bit in the same buffer the request came in
        VirtioMsg::from_bytes_mut(buf).type_ |= VIRTIO_MSG_TYPE_RESPONSE as u8;
        VirtioMsgResp { buf }
    }

    fn as_mut_v2_payload<T: FromBytes + IntoBytes + KnownLayout>(&mut self) -> &mut T {
        let buf_size = size_of_val(self.buf);
        let resp = sys_dev2::virtio_msg::from_bytes_mut(self.buf);
        let total_size = size_of_val(resp) + size_of::<T>();
        resp.msg_size = total_size.try_into().unwrap();
        // SAFETY: The `payload` field on `resp` is right after the header fields and this creates a
        // `&mut [u8]` of the remaining portion of the buffer from which `resp` is derived.
        let payload_slice = unsafe { resp.payload.as_mut_slice(buf_size - size_of_val(resp)) };
        // The payload slice is at least as big as any virtio-msg response which we write so this
        // should not panic
        let (payload_ref, remainder) = FromBytes::mut_from_prefix(payload_slice).unwrap();
        // Zero out unused space in the response buffer
        remainder.fill(0);
        payload_ref
    }

    pub fn write_bus_ffa_version(mut self, device_version: u32, vmsg_revision: u32, features: u32) {
        let resp = self.as_mut_v2_payload::<sys_dev2::bus_ffa_version_resp>();
        resp.device_version = device_version;
        resp.vmsg_revision = vmsg_revision;
        resp.features = features;
    }

    pub fn write_bus_get_devices(self, next_offset: u16, bitmap: u8) {
        let resp = sys_dev2::virtio_msg::from_bytes_mut(self.buf);
        let bitmap_size = size_of_val(&bitmap);
        let total_size =
            size_of_val(resp) + size_of::<sys_dev2::bus_get_devices_resp>() + bitmap_size;
        resp.msg_size = total_size.try_into().unwrap();

        // This pointer may be unaligned since virtio_msg is packed
        let payload_ptr = resp.payload.as_mut_ptr().cast::<sys_dev2::bus_get_devices_resp>();

        // The number of devices described by the bitmap
        let num_devs = bitmap_size * 8;

        let payload = sys_dev2::bus_get_devices_resp {
            offset: 0,
            num: num_devs.try_into().unwrap(),
            next_offset,
            ..Default::default()
        };
        // SAFETY: payload_ptr is non-null and points to a buffer at least as big as a
        // bus_get_devices_resp struct because the pointer is derived from a reference to self.buf
        // with an offset to skip the header in the virtio_msg struct.
        unsafe { write_unaligned(payload_ptr, payload) }
        // SAFETY: payload_ptr is non-null and points to a valid bus_get_devices_resp struct
        let bitmap_ptr = unsafe { &raw mut (*payload_ptr).devices };
        // SAFETY: bitmap_ptr is non-null and points to a u8
        unsafe { write(bitmap_ptr.cast::<u8>(), bitmap) }
    }

    // Set the payload as the response to a device_info request
    pub fn write_device_info(mut self, dev_ty: DeviceType, vendor_id: u32) {
        let resp = self.as_mut_v2_payload::<sys_dev2::get_device_info_resp>();
        resp.device_id = dev_ty as u32;
        resp.vendor_id = vendor_id;
        resp.num_feature_bits = u32::try_from(size_of::<VsockVirtioFeatures>()).unwrap() * 8;
        // vsock config space only includes one u64 for CID
        resp.config_size = u32::try_from(size_of::<u64>()).unwrap();
        // vsock devices only support 3 virtqueues
        resp.max_vq_count = 3;
        resp.admin_vq_start_idx = 0;
        resp.admin_vq_count = 0;
    }

    // Set the payload as the response to a get_device_status request
    pub fn write_get_device_status(mut self, status: u32) {
        let resp = self.as_mut_v2_payload::<sys_dev2::get_device_status_resp>();
        resp.status = status;
    }

    // Set the payload as the response to a get_device_features request
    pub fn write_get_device_features(self, index: u32, features: VsockVirtioFeatures) {
        let resp = sys_dev2::virtio_msg::from_bytes_mut(self.buf);
        let feature_data_size = size_of::<VsockVirtioFeatures>();
        let total_size =
            size_of_val(resp) + size_of::<sys_dev2::get_features_resp>() + feature_data_size;
        resp.msg_size = total_size.try_into().unwrap();

        let payload_ptr = resp.payload.as_mut_ptr().cast::<sys_dev2::get_features_resp>();
        let num_blocks = feature_data_size / size_of::<u32>();
        let payload = sys_dev2::get_features_resp {
            index,
            num: num_blocks.try_into().unwrap(),
            ..Default::default()
        };
        // SAFETY: payload_ptr is non-null and points to a buffer at least as big as a
        // get_features_resp struct because the pointer is derived from a reference to self.buf
        // with an offset to skip the header in the virtio_msg struct.
        unsafe { write_unaligned(payload_ptr, payload) }
        // SAFETY: payload_ptr is non-null and points to a valid get_features_resp struct
        let features_ptr = unsafe { &raw mut (*payload_ptr).features };
        // SAFETY: features_ptr is non-null and points to a buffer at least as big as a u64 because
        // the pointer is derived from a reference to self.buf with an offset to skip the header in
        // the virtio_msg struct and the fixed-size part of the get_features_resp struct
        unsafe { write_unaligned(features_ptr.cast::<u64>(), features) }
    }

    // Set the payload as the response to a get_vqueue request
    pub fn write_get_vqueue(mut self, index: u32, vqueue: Option<&VirtQueue>) {
        let resp = self.as_mut_v2_payload::<sys_dev2::get_vqueue_resp>();
        // virtio-msg spec 3.2: If the virtqueue was configured, the current information is returned
        // otherwise all fields other than Max Virtqueue Size are 0.
        match vqueue {
            Some(vqueue) => {
                resp.index = index;
                resp.max_size = VSOCK_QUEUE_SIZE;
                resp.size = vqueue.size;
                resp.descriptor_addr = vqueue.desc_table as u64;
                resp.driver_addr = vqueue.avail_ring as u64;
                resp.device_addr = vqueue.used_ring as u64;
            }
            None => {
                resp.index = index;
                resp.max_size = VSOCK_QUEUE_SIZE;
                resp.size = 0;
                resp.descriptor_addr = 0;
                resp.driver_addr = 0;
                resp.device_addr = 0;
            }
        }
    }

    // Set the payload as the response to a get_config request
    pub fn write_get_config(self, generation: u32, offset: u32, size: u32, data: u64) {
        let resp = sys_dev2::virtio_msg::from_bytes_mut(self.buf);
        let total_size =
            size_of_val(resp) + size_of::<sys_dev2::get_config_resp>() + size_of_val(&data);
        resp.msg_size = total_size.try_into().unwrap();
        let payload_ptr = resp.payload.as_mut_ptr().cast::<sys_dev2::get_config_resp>();

        // SAFETY: All calls to write_unaligned take a non-null pointer to a buffer at least as big
        // as the type of the argument being written since payload_ptr is derived from a reference
        // self.buf with an offset to skip the header in the virtio_msg struct
        unsafe {
            let generation_ptr = &raw mut (*payload_ptr).generation;
            write_unaligned(generation_ptr, generation);

            let offset_ptr = &raw mut (*payload_ptr).offset;
            write_unaligned(offset_ptr, offset);

            let size_ptr = &raw mut (*payload_ptr).size;
            write_unaligned(size_ptr, size);

            let config_ptr = &raw mut (*payload_ptr).config;
            write_unaligned(config_ptr.cast::<u64>(), data);
        }
    }

    pub fn write_bus_area_share(mut self, area_id: u16, success: bool) {
        let resp = self.as_mut_v2_payload::<sys_dev2::bus_area_share_resp>();
        resp.area_id = area_id;
        resp.result = if success { 0 } else { 1 };
    }

    pub fn write_bus_area_unshare(mut self, area_id: u16, success: bool) {
        let resp = self.as_mut_v2_payload::<sys_dev2::bus_area_unshare_resp>();
        resp.area_id = area_id;
        resp.result = if success { 0 } else { 1 };
    }
}
