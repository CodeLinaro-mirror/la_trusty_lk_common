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

use crate::msg::driver::requests::{VirtioMsgReq, VirtioMsgResp};
use crate::msg::VIRTIO_MSG_FFA_UUID;
use crate::sys::{
    bus_activate_resp as BusActivateResp, bus_configure_resp as BusConfigureResp,
    get_device_info_resp as GetDeviceInfoResp, VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_SUPP,
    VIRTIO_MSG_FFA_FEATURE_NUM_SHM, VIRTIO_MSG_FFA_VERSION_1_0,
};
use arm_ffa::{msg_send_direct_req2, partition_info_get_count, partition_info_get_desc};
use core::ffi::c_uint;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};
use log::{debug, error, info, warn};
use rust_support::init::lk_init_level;
use rust_support::{Error as LkError, LK_INIT_HOOK};
use static_assertions::const_assert_eq;
use virtio_drivers_and_devices::transport::DeviceType;

mod requests;

type Result<T> = core::result::Result<T, LkError>;

// RECEIVER_ID is an FFA ID so it's only 16 bits, but we use the upper 16 bits as a sentinel to
// ensure it's been validated.
static RECEIVER_ID: AtomicU32 = AtomicU32::new(u32::MAX);

fn get_receiver_id() -> u16 {
    let receiver_id = RECEIVER_ID.load(Ordering::Relaxed);
    // u32::MAX is the initial sentinel value, but any valid value will fit in a u16
    receiver_id.try_into().expect("RECEIVER_ID has not been initialized")
}

fn init_receiver_id() -> Result<u16> {
    let num_desc = partition_info_get_count(VIRTIO_MSG_FFA_UUID).inspect_err(|e| {
        error!("arm_ffa_partition_info_get_count failed with {e:?}");
    })?;
    if num_desc != 1 {
        error!(
                "arm_ffa_partition_info_get_count: expected 1 partition descriptor, received {num_desc:?}"
            );
        return Err(LkError::ERR_GENERIC);
    }

    let mut desc = [MaybeUninit::uninit()];
    let mut desc_iter =
        partition_info_get_desc(VIRTIO_MSG_FFA_UUID, &mut desc).inspect_err(|e| {
            error!("arm_ffa_partition_info_get_desc failed with {e:?}");
        })?;
    let receiver_id = match desc_iter.next() {
        Some(init_desc) => init_desc.partition_id,
        None => {
            error!("arm_ffa_partition_info_get_desc did not return any descriptors");
            return Err(LkError::ERR_GENERIC);
        }
    };
    RECEIVER_ID.store(u32::from(receiver_id), Ordering::Relaxed);
    Ok(receiver_id)
}

fn send_virtio_msg_request(req: VirtioMsgReq) -> Result<VirtioMsgResp> {
    let receiver_id = get_receiver_id();
    let resp =
        msg_send_direct_req2(VIRTIO_MSG_FFA_UUID, receiver_id, req.get_buf()).inspect_err(|e| {
            warn!("msg_send_direct_req2 failed {e:?}");
        })?;
    VirtioMsgResp::new(resp.params)
}

fn activate_device(driver_version: u32) -> Result<BusActivateResp> {
    let req = VirtioMsgReq::activate(driver_version);
    let resp = send_virtio_msg_request(req)?;
    resp.into_activate()
}

fn configure_device(num_shm: u8) -> Result<BusConfigureResp> {
    // This VirtioMsgReq constructor only takes the number of shared memory regions as an argument
    // and hard-codes direct message as supported
    let req = VirtioMsgReq::configure(num_shm);
    let resp = send_virtio_msg_request(req)?;
    resp.into_configure()
}

fn get_device_info(dev_id: u16) -> Result<GetDeviceInfoResp> {
    let req = VirtioMsgReq::get_device_info(dev_id);
    let resp = send_virtio_msg_request(req)?;
    resp.into_get_device_info()
}

// Validates that the `features` bitmask supports direct messages and at least the requested number
// of shared memory regions.
fn validate_features(features: u64, req_num_shm: u8) -> Result<()> {
    if features & VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_SUPP as u64 == 0 {
        // The driver requires support for direct messages
        return Err(LkError::ERR_NOT_VALID);
    }
    // This constant is a 16-bit bitmask so ensure the u32 generated for the macro by bindgen is
    // correct.
    const_assert_eq!(VIRTIO_MSG_FFA_FEATURE_NUM_SHM >> 16, 0);
    // Also ensure that the lower 8 bits are zero
    const_assert_eq!(VIRTIO_MSG_FFA_FEATURE_NUM_SHM & 0x1FF, 0x100);
    // `as u8` cannot truncate since the constant is a 16-bit bitmask and we shifted down by 8
    let features_num_shm = ((features & VIRTIO_MSG_FFA_FEATURE_NUM_SHM as u64) >> 8) as u8;
    if features_num_shm < req_num_shm {
        // Number of shared memory regions in the feature bits didn't match the expected num_shm
        return Err(LkError::ERR_NOT_VALID);
    }
    Ok(())
}

fn driver_init() -> Result<()> {
    // Call FFA_PARTITION_INFO_GET to get the FFA ID for the partition with the virtio-msg device
    init_receiver_id()?;

    // Send an activate request to the virtio-msg device over FFA
    let activate_resp = activate_device(VIRTIO_MSG_FFA_VERSION_1_0).inspect_err(|e| {
        error!("virtio-msg activate request failed with {e}");
    })?;
    debug!("received {activate_resp:x?} as response to virtio-msg activate request");

    // Make sure the virtio-msg protocol version is what we expect. There is currently only one
    // version
    let dev_version = activate_resp.device_version;
    if dev_version != VIRTIO_MSG_FFA_VERSION_1_0 {
        error!("found unexpected device version {dev_version:x?}");
        return Err(LkError::ERR_NOT_VALID);
    }

    // Validate that the device supports direct messages and at least one shared memory region.
    let features = activate_resp.features;
    validate_features(features, 1).inspect_err(|e| {
        error!("failed to validate features bitmask {features:x?} {e}");
    })?;

    // Send a configure device request with one shared memory region to negotiate features
    let configure_resp = configure_device(1).inspect_err(|e| {
        error!("failed to configure vsock device {e}");
    })?;
    debug!("device configuration returned {configure_resp:x?}");

    // Ensure the features accepted by the device are valid and match what the driver requested.
    validate_features(configure_resp.features, 1).inspect_err(|e| {
        error!("failed to validate features bitmask {features:x?} {e}");
    })?;

    let num_devices = activate_resp.num as u16;

    // Go through all the devices on the virtio-msg bus and initialize the vsock devices
    for dev_id in 0..num_devices {
        debug!("getting info for device #{dev_id:?}");
        let dev_info_resp = get_device_info(dev_id)?;
        debug!("get_device_info returned {dev_info_resp:?}");

        let dev_ty = dev_info_resp.device_id;
        if dev_ty != DeviceType::Socket as u32 {
            // Non vsock virtio-msg devices are not currently expected but should just be ignored
            info!("ignoring unexpected non-vsock virtio-msg device with type {dev_ty:?}");
            continue;
        }
    }

    Ok(())
}

extern "C" fn virtio_msg_driver_init_func(_: c_uint) {
    debug!("initializing virtio-msg vsock driver...");
    match driver_init() {
        Ok(_) => {}
        Err(LkError::ERR_NOT_SUPPORTED) => {
            // FFA is not supported so log that the vsock driver is not enabled and continue booting
            info!("disabling virtio-msg vsock driver (FFA not supported")
        }
        Err(_) => {
            // Any error other than ERR_NOT_SUPPORTED is unexpected
            panic!("failed to initialize virtio-msg vsock driver")
        }
    }
}

LK_INIT_HOOK!(
    virtio_msg_driver_init,
    virtio_msg_driver_init_func,
    lk_init_level::LK_INIT_LEVEL_PLATFORM
);
