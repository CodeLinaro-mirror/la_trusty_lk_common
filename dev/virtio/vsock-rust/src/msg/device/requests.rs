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
use crate::VsockVirtioFeatures;
use core::mem::size_of_val;
use core::ptr::{write, write_unaligned};
// glob import since we only allowlist virtio_msg.h, VirtioMsgFFA.h and virtio_config.h in bindgen
use crate::sys::*;
use arm_ffa::ARM_FFA_MSG_EXTENDED_ARGS_COUNT;
use rust_support::Error as LkError;
use virtio_drivers_and_devices::transport::DeviceType;
use zerocopy::{FromBytes, IntoBytes, KnownLayout};

pub struct VirtioMsgReq<'a> {
    buf: &'a mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT],
}

#[derive(Debug)]
pub enum VirtioMsgPayload {
    BusFFAVersion(sys_dev2::bus_ffa_version),
    BusGetDevices(sys_dev2::bus_get_devices),
    GetDeviceInfo,
    SetDeviceStatus(set_device_status),
    GetDeviceStatus,
    GetFeatures(get_features),
    SetFeatures(set_features),
    GetVqueue(get_vqueue),
    SetVqueue(set_vqueue),
    GetConfig(get_config),
    EventAvail(event_avail),
    EventUsed(event_used),
    EventConfig(event_config),
    AreaShare(bus_area_share),
    AreaUnshare(bus_area_unshare),
    ResetVqueue(reset_vqueue),
    GetConfigGen,
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
        let req = VirtioMsgFFA::from_bytes(self.buf);
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

    pub fn get_msg_payload(&self) -> VirtioMsgPayload {
        let req = VirtioMsgFFA::from_bytes(self.buf);

        // The request ID determines which field is valid in the payload union. Note that a given ID
        // may represent different requests depending on whether it's a bus or virtio message.
        let id = u32::from(req.id);

        if self.is_bus_msg() {
            match id {
                sys_dev2::VIRTIO_MSG_FFA_BUS_VERSION => {
                    VirtioMsgPayload::BusFFAVersion(self.get_v2_payload())
                }
                sys_dev2::VIRTIO_MSG_BUS_GET_DEVICES => {
                    VirtioMsgPayload::BusGetDevices(self.get_v2_payload())
                }
                VIRTIO_MSG_FFA_AREA_SHARE => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::AreaShare(unsafe { req.__bindgen_anon_1.bus_area_share })
                }
                VIRTIO_MSG_FFA_AREA_UNSHARE => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::AreaUnshare(unsafe { req.__bindgen_anon_1.bus_area_unshare })
                }
                VIRTIO_MSG_FFA_ERROR => todo!("support VIRTIO_MSG_FFA_ERROR"),
                _ => VirtioMsgPayload::UnknownBusReq(id),
            }
        } else {
            let req = VirtioMsg::from_bytes(self.buf);
            match id {
                // GET_DEVICE_INFO requests don't use the payload
                sys_dev2::VIRTIO_MSG_DEVICE_INFO => VirtioMsgPayload::GetDeviceInfo,
                VIRTIO_MSG_SET_DEVICE_STATUS => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::SetDeviceStatus(unsafe {
                        req.__bindgen_anon_1.set_device_status
                    })
                }
                // GET_DEVICE_STATUS requests don't use the payload
                VIRTIO_MSG_GET_DEVICE_STATUS => VirtioMsgPayload::GetDeviceStatus,
                VIRTIO_MSG_GET_FEATURES => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::GetFeatures(unsafe { req.__bindgen_anon_1.get_features })
                }
                VIRTIO_MSG_SET_FEATURES => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::SetFeatures(unsafe { req.__bindgen_anon_1.set_features })
                }
                VIRTIO_MSG_GET_VQUEUE => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::GetVqueue(unsafe { req.__bindgen_anon_1.get_vqueue })
                }
                VIRTIO_MSG_SET_VQUEUE => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::SetVqueue(unsafe { req.__bindgen_anon_1.set_vqueue })
                }
                VIRTIO_MSG_GET_CONFIG => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::GetConfig(unsafe { req.__bindgen_anon_1.get_config })
                }
                VIRTIO_MSG_EVENT_AVAIL => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::EventAvail(unsafe { req.__bindgen_anon_1.event_avail })
                }
                VIRTIO_MSG_EVENT_USED => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::EventUsed(unsafe { req.__bindgen_anon_1.event_used })
                }
                VIRTIO_MSG_EVENT_CONFIG => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::EventConfig(unsafe { req.__bindgen_anon_1.event_config })
                }
                VIRTIO_MSG_RESET_VQUEUE => {
                    // SAFETY: `req` is an array of bytes which is sufficient to initialize all
                    // union variants with valid values.
                    VirtioMsgPayload::ResetVqueue(unsafe { req.__bindgen_anon_1.reset_vqueue })
                }
                VIRTIO_MSG_CONNECT => todo!("support VIRTIO_MSG_CONNECT"),
                VIRTIO_MSG_SET_CONFIG => todo!("support VIRTIO_MSG_SET_CONFIG"),
                VIRTIO_MSG_GET_CONFIG_GEN => VirtioMsgPayload::GetConfigGen,
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

    pub fn ffa_error(self) {
        let resp = VirtioMsgFFA::from_bytes_mut(self.buf);
        let is_bus_msg = u32::from(resp.type_) & VIRTIO_MSG_TYPE_BUS != 0;
        assert!(is_bus_msg);
        resp.id = u8::try_from(VIRTIO_MSG_FFA_ERROR).unwrap();
        resp.__bindgen_anon_1.payload_u8 = [0; 36];
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

    // Set the payload as the response to an activate request
    pub fn activate(self, device_version: u32, features: u64, num_dev: u64) {
        let resp = VirtioMsgFFA::from_bytes_mut(self.buf);
        resp.__bindgen_anon_1.bus_activate_resp =
            bus_activate_resp { device_version, features, num: num_dev }
    }

    // Set the payload as the response to a configure request
    pub fn configure(self, features: u64) {
        let resp = VirtioMsgFFA::from_bytes_mut(self.buf);
        resp.__bindgen_anon_1.bus_configure_resp = bus_configure_resp { features }
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
    pub fn get_device_status(self, status: u32) {
        let resp = VirtioMsg::from_bytes_mut(self.buf);
        resp.__bindgen_anon_1.get_device_status_resp = get_device_status_resp { status }
    }

    // Set the payload as the response to a get_features request
    pub fn get_features(self, index: u32, features: u64) {
        let resp = VirtioMsg::from_bytes_mut(self.buf);
        resp.__bindgen_anon_1.get_features_resp = get_features_resp {
            index,
            // bits 38 and up are reserved so unconditionally zero the top 192 bits
            features: [features, 0, 0, 0],
        }
    }

    pub fn set_features(self, index: u32, features: u64) {
        let resp = VirtioMsg::from_bytes_mut(self.buf);
        resp.__bindgen_anon_1.set_features_resp =
            set_features_resp { index, features: [features, 0, 0, 0] }
    }

    // Set the payload as the response to a get_vqueue request
    pub fn get_vqueue(self, index: u32, vqueue: Option<&VirtQueue>) {
        let resp = VirtioMsg::from_bytes_mut(self.buf);
        // virtio-msg spec 3.2: If the virtqueue was configured, the current information is returned
        // otherwise all fields other than Max Virtqueue Size are 0.
        match vqueue {
            Some(vqueue) => {
                resp.__bindgen_anon_1.get_vqueue_resp = get_vqueue_resp {
                    index,
                    max_size: VSOCK_QUEUE_SIZE,
                    size: vqueue.size,
                    descriptor_addr: vqueue.desc_table as u64,
                    driver_addr: vqueue.avail_ring as u64,
                    device_addr: vqueue.used_ring as u64,
                };
            }
            None => {
                resp.__bindgen_anon_1.get_vqueue_resp = get_vqueue_resp {
                    index,
                    max_size: VSOCK_QUEUE_SIZE,
                    size: 0,
                    descriptor_addr: 0,
                    driver_addr: 0,
                    device_addr: 0,
                };
            }
        }
    }

    // Set the payload as the response to a set_vqueue request
    pub fn set_vqueue(
        self,
        index: u32,
        size: u32,
        descriptor_addr: u64,
        driver_addr: u64,
        device_addr: u64,
    ) {
        let resp = VirtioMsg::from_bytes_mut(self.buf);
        resp.__bindgen_anon_1.set_vqueue_resp =
            set_vqueue_resp { index, unused: 0, size, descriptor_addr, driver_addr, device_addr };
    }

    // Set the payload as the response to a get_config request
    pub fn get_config(self, offset: [u8; 3], size: u8, data: [u64; 4]) {
        let resp = VirtioMsg::from_bytes_mut(self.buf);
        resp.__bindgen_anon_1.get_config_resp = get_config_resp { offset, size, data };
    }

    pub fn get_config_gen(self, generation: u32) {
        let resp = VirtioMsg::from_bytes_mut(self.buf);
        resp.__bindgen_anon_1.get_config_gen_resp = get_config_gen_resp { generation };
    }
}
