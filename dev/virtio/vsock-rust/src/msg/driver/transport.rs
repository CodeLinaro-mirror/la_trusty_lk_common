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

use crate::msg::bus_address;
use crate::msg::driver::{
    get_device_info, send_virtio_msg_request, VirtioMsgReq, INITIAL_AREA_ID, MAIN_HEAP,
};
use crate::sys_dev2::VIRTIO_CONFIG_S_NEEDS_RESET;
use crate::VsockVirtioFeatures;
use core::mem::size_of;
use log::warn;
use virtio_drivers_and_devices::transport::{DeviceStatus, DeviceType, InterruptStatus, Transport};
use virtio_drivers_and_devices::{Error as VirtioError, PhysAddr};
use zerocopy::{FromBytes, Immutable, IntoBytes};

type Result<T> = core::result::Result<T, VirtioError>;

// The transport layer for virtio-msg devices.
pub struct FFAMsgTransport {
    // The device id specified in the header for each virtio-msg request
    dev_id: u16,
}

impl FFAMsgTransport {
    pub fn new(dev_id: u16) -> Self {
        Self { dev_id }
    }
}

// TODO: Move this to virtio-drivers once the virtio-msg transport becomes standardized and the
// vsock-specific parts of this are generalized to other device types.
// TODO: Most calls to `expect` in these methods should bubble up an Error rather than panic once
// the trait supports this https://github.com/immunant/virtio-drivers-and-devices/issues/18
impl Transport for FFAMsgTransport {
    fn device_type(&self) -> DeviceType {
        let dev_info_resp = get_device_info(self.dev_id).expect("get_device_info request failed");
        DeviceType::try_from(dev_info_resp.device_id)
            .expect("found virtio-msg device with unexpected type")
    }

    fn read_device_features(&mut self) -> u64 {
        // Feature blocks are groups of 32 bits
        let num_blocks = size_of::<VsockVirtioFeatures>() / size_of::<u32>();
        let req = VirtioMsgReq::new_get_device_features(
            self.dev_id,
            0, /* index */
            num_blocks.try_into().unwrap(),
        );
        let resp = send_virtio_msg_request(req).expect("get_features request failed");
        let feature_data =
            resp.read_get_device_features().expect("get_features returned invalid response").1;
        u64::from_le_bytes(feature_data[0..8].try_into().unwrap())
    }

    fn write_driver_features(&mut self, driver_features: u64) {
        let req = VirtioMsgReq::set_features(self.dev_id, 0, [driver_features, 0, 0, 0]);
        send_virtio_msg_request(req).expect("set_features virtio-msg request failed");
    }

    fn max_queue_size(&mut self, queue: u16) -> u32 {
        let req = VirtioMsgReq::new_get_vqueue(self.dev_id, queue);
        let resp = send_virtio_msg_request(req).expect("get_vqueue request failed");
        resp.read_get_vqueue().expect("get_vqueue returned invalid response").max_size
    }

    fn notify(&mut self, _queue: u16) {
        // nop for now since Trusty virtio-msg device ignores event_avail requests anyway
    }

    fn get_status(&self) -> DeviceStatus {
        let req = VirtioMsgReq::new_get_device_status(self.dev_id);
        let resp = send_virtio_msg_request(req).expect("get_device_status request failed");
        let status = resp
            .read_get_device_status()
            .expect("get_device_status returned invalid response")
            .status;
        DeviceStatus::from_bits_retain(status)
    }

    fn set_status(&mut self, status: DeviceStatus) {
        let req = VirtioMsgReq::new_set_device_status(self.dev_id, status);
        let resp = send_virtio_msg_request(req).expect("set_device_status request failed");
        let status = resp
            .read_set_device_status()
            .expect("set_device_status returned invalid response")
            .status;
        if status & VIRTIO_CONFIG_S_NEEDS_RESET != 0 {
            // TODO: Bubble up an error to allow cleanly resetting the device by dropping the
            // VirtIOSocket and FFAMsgTransport
            panic!("virtio-msg device needs reset");
        }
    }

    fn set_guest_page_size(&mut self, _guest_page_size: u32) {
        // virtio-msg over FF-A always uses 4K pages
    }

    fn requires_legacy_layout(&self) -> bool {
        // Only support the legacy virtqueue layout for simplicity
        true
    }

    fn queue_set(
        &mut self,
        queue: u16,
        size: u32,
        desc_phys: PhysAddr,
        driver_area_phys: PhysAddr,
        device_area_phys: PhysAddr,
    ) {
        let main_heap = MAIN_HEAP.lock();
        let base_paddr = main_heap.paddr;
        let desc_bus = bus_address(INITIAL_AREA_ID, (desc_phys - base_paddr) as u64);
        let driver_area_bus = bus_address(INITIAL_AREA_ID, (driver_area_phys - base_paddr) as u64);
        let device_area_bus = bus_address(INITIAL_AREA_ID, (device_area_phys - base_paddr) as u64);

        // Send the set_vqueue virtio-msg request
        let req = VirtioMsgReq::new_set_vqueue(
            self.dev_id,
            queue,
            size,
            desc_bus as u64,
            driver_area_bus as u64,
            device_area_bus as u64,
        );
        send_virtio_msg_request(req).expect("set_vqueue request failed");
    }

    fn queue_unset(&mut self, _queue: u16) {
        todo!("unsetting vqueues is not supported yet")
    }

    fn queue_used(&mut self, queue: u16) -> bool {
        let req = VirtioMsgReq::new_get_vqueue(self.dev_id, queue);
        let resp = send_virtio_msg_request(req).expect("get_vqueue request failed");
        let get_vqueue = resp.read_get_vqueue().expect("get_vqueue returned invalid response");
        // virtio-msg spec: If the vqueue hasn't been configured all fields are zero
        let all_fields_zero = get_vqueue.size == 0
            && get_vqueue.descriptor_addr == 0
            && get_vqueue.driver_addr == 0
            && get_vqueue.device_addr == 0;
        !all_fields_zero
    }

    fn ack_interrupt(&mut self) -> InterruptStatus {
        unreachable!("ack_interrupt should not be called for vsock devices")
    }

    fn read_config_generation(&self) -> u32 {
        let req = VirtioMsgReq::new_get_config(self.dev_id, 0 /* offset */, 0 /* size */);
        let resp = send_virtio_msg_request(req).expect("get_config request failed");
        let (config_resp_header, _config_data) =
            resp.read_get_config().expect("get_config returned invalid response");
        config_resp_header.generation
    }

    fn read_config_space<T: FromBytes>(&self, offset: usize) -> Result<T> {
        let req = VirtioMsgReq::new_get_config(
            self.dev_id,
            offset as u32,
            size_of::<T>().try_into().unwrap(),
        );
        let resp = send_virtio_msg_request(req).expect("get_config request failed");
        let (_config_resp_header, config_data) =
            resp.read_get_config().expect("get_config returned invalid response");
        // vsock config space is only 8 bytes so the call in virtio-drivers-and-devices should never
        // cause this to panic.
        let (config_data, remainder) = T::read_from_prefix(config_data).unwrap();
        if remainder.iter().any(|&b| b != 0) {
            warn!("virtio-msg device returned more config data than expected");
            return Err(VirtioError::InvalidParam);
        }
        Ok(config_data)
    }

    fn write_config_space<T: IntoBytes + Immutable>(
        &mut self,
        _offset: usize,
        _value: T,
    ) -> Result<()> {
        unreachable!("vsock device should not write to the config space")
    }
}
