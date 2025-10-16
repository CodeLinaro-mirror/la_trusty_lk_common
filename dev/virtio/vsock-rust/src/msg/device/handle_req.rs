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
use crate::msg::device::VirtioMsgDevice;
use crate::msg::device::VirtioMsgPayload;
use crate::msg::device::VirtioMsgReq;
use crate::msg::device::VirtioMsgResp;
use crate::msg::device::TRANSPORT;
use crate::msg::BusAddress;
use crate::msg::VirtioMsg;
use crate::msg::MAX_NUM_SHM;
use crate::FFAClientId;
// glob import since we only allowlist virtio_msg.h, virtio_msg_ffa.h and virtio_config.h in bindgen
use crate::sys::*;
use crate::sys_dev2::{
    VIRTIO_MSG_FFA_BUS_VERSION_1_0, VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_RX_SUPP,
    VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_TX_SUPP, VIRTIO_MSG_REVISION_1,
};
use arm_ffa::ARM_FFA_MSG_EXTENDED_ARGS_COUNT;
use log::debug;
use log::error;
use log::trace;
use log::warn;
use rust_support::status_t;
use rust_support::Error as LkError;
use virtio_drivers_and_devices::transport::DeviceType;

impl VirtioMsgDevice {
    /// Handles a virtio-msg request in a u64 array and writes the response back to the same array.
    fn handle_req(
        &self,
        buf_ref: &mut [u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT],
    ) -> Result<(), LkError> {
        let dev_id = VirtioMsg::from_bytes(buf_ref).dev_id;
        // Pass ownership of the `buf_ref` argument to a `VirtioMsgReq` to reinterpret the array as
        // a virtio-msg request
        let req = VirtioMsgReq::parse(buf_ref)?;
        // The dev_id field is not used for bus messages (i.e. FFA-specific requests) so we should
        // not do this check in that case. Trusty currently only emulates one virtio-msg device for
        // each VM so this ID should always be 0.
        if !req.is_bus_msg() && dev_id != 0 {
            error!("invalid dev_id field {dev_id:x?} in virtio-msg request");
            return Err(LkError::ERR_INVALID_ARGS);
        }

        // Define features specific to the virtio-msg transport protocol. Currently indirect messages
        // are not supported and Trusty supports the max number of shared memory regions (255). All
        // other bits are reserved (MBZ).
        const VIRTIO_MSG_FEATURES: u64 =
            (VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_SUPP | VIRTIO_MSG_FFA_FEATURE_NUM_SHM) as u64;

        const VIRTIO_VSOCK_F_STREAM: u64 = 0;
        const VIRTIO_F_VERSION_1: u64 = 32;
        const VIRTIO_F_ACCESS_PLATFORM: u64 = 33;

        const SUPPORTED_VIRTIO_FEATURES: u64 = (1 << VIRTIO_VSOCK_F_STREAM)
            | (1 << VIRTIO_F_VERSION_1)
            | (1 << VIRTIO_F_ACCESS_PLATFORM);

        // Create a copy of the request payload. The enum variant determines what kind of request it is
        let req_payload = req.get_msg_payload();
        // Passes ownership of the `buf_ref` argument to a `VirtioMsgResp` to reinterpret the array
        // as a virtio-msg response which will be filled in depending on how we handle the request
        let resp = VirtioMsgResp::new(req);
        match req_payload {
            VirtioMsgPayload::BusFFAVersion(req) => {
                const DEVICE_FFA_BUS_VERSION: u32 = VIRTIO_MSG_FFA_BUS_VERSION_1_0;
                let req_driver_version = req.driver_version;
                let resp_device_version = match req_driver_version {
                    0..DEVICE_FFA_BUS_VERSION => {
                        debug!(
                            "requested virtio-msg version not supported {:?}",
                            req_driver_version
                        );
                        None
                    }
                    DEVICE_FFA_BUS_VERSION => {
                        // Return the exact version in the request in case the device supports a
                        // range of versions in the future.
                        Some(req_driver_version)
                    }
                    _higher_versions => Some(DEVICE_FFA_BUS_VERSION),
                };

                const DEVICE_VIRTIO_MSG_REVISION: u32 = VIRTIO_MSG_REVISION_1;
                let req_driver_revision = req.vmsg_revision;
                let resp_device_revision = match req_driver_revision {
                    0..DEVICE_VIRTIO_MSG_REVISION => {
                        debug!(
                            "requested virtio-msg revision not supported {:?}",
                            req_driver_revision
                        );
                        None
                    }
                    DEVICE_VIRTIO_MSG_REVISION => {
                        // Return the exact revision in the request in case the device supports a
                        // range of revisions in the future.
                        Some(req_driver_revision)
                    }
                    _higher_revisions => Some(DEVICE_VIRTIO_MSG_REVISION),
                };

                let req_features = req.features;
                if req_features & VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_TX_SUPP == 0 {
                    debug!(
                        "requested virtio-msg features don't include direct TX {:x?}",
                        req_features
                    );
                };
                let resp_features = VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_RX_SUPP;
                resp.write_bus_ffa_version(
                    resp_device_version.unwrap_or(0),
                    resp_device_revision.unwrap_or(0),
                    resp_features,
                );
            }
            VirtioMsgPayload::BusGetDevices(req) => {
                let next_offset = 0;
                let device_bitmap: u8 = if req.offset == 0 {
                    // Only one vsock device per VM is currently supported.
                    0b1
                } else {
                    let offset = req.offset;
                    warn!(
                        "unexpected virtio-msg bus_get_devices request with offset: {:?}",
                        offset
                    );
                    0b0
                };
                resp.write_bus_get_devices(next_offset, device_bitmap);
            }
            VirtioMsgPayload::GetDeviceInfo => {
                debug!("received virtio-msg get_device_info request");
                let dev_ty = DeviceType::Socket;
                let vendor_id = 0;
                resp.write_device_info(dev_ty, vendor_id);
            }
            VirtioMsgPayload::SetDeviceStatus(req) => {
                debug!("received virtio-msg set_device_status request {req:x?}");

                // virtio spec 3.1.1: Device initialization
                // 1.  Reset the device.
                // 2.  Set the ACKNOWLEDGE status bit
                // 3.  Set the DRIVER status bit
                // 4.  Read device feature bits (no status bit change)
                // 5.  Set the FEATURES_OK status bit
                // 6.  Re-read device status to ensure the FEATURES_OK bit is still set
                // 7.  Perform device-specific setup (no status bit change)
                // 8.  Set the DRIVER_OK status bit
                //
                // If any of these steps go irrecoverably wrong, the driver SHOULD set the FAILED
                // status bit to indicate that it has given up on the device (it can reset the
                // device later to restart if desired). The driver MUST NOT continue initialization in that case.

                let mut state = self.state.lock_unsaved();

                // Status can change to RESET from any state so we don't need to check the old status
                // TODO(b/433491122): This also needs to free the device's resources.
                if req.status == 0 {
                    debug!("virtio-msg driver triggered device reset");
                    state.status = 0;
                    state.vqueues = [None, None, None];
                    return Ok(());
                }

                // Status can change to FAILED from any state so we don't need to check the old status
                if req.status & VIRTIO_CONFIG_S_FAILED != 0 {
                    warn!("virtio-msg driver gave up on the device");
                    state.status = 0;
                    return Ok(());
                }

                // Check whether the status transition is valid. The ordering here is important
                // since transitioning to a new state doesn't clear the old status bits. We first
                // check the FAILED bit then check the bits in the reverse of the order we expect
                // them to be set in.
                let valid_transition: bool;
                if state.status & VIRTIO_CONFIG_S_FAILED != 0 {
                    // The only valid transition from the FAILED status is RESET which is handled above
                    error!("virtio-msg device requires reset");
                    valid_transition = false;
                } else if state.status & VIRTIO_CONFIG_S_DRIVER_OK != 0 {
                    // The only valid transitions from DRIVER_OK are FAILED or RESET which are handled above
                    valid_transition = false;
                } else if state.status & VIRTIO_CONFIG_S_FEATURES_OK != 0 {
                    // The only valid transitions from FEATURES_OK are DRIVER_OK, FAILED or RESET
                    valid_transition = req.status & VIRTIO_CONFIG_S_DRIVER_OK != 0;
                    if valid_transition {
                        TRANSPORT.device_init.signal();
                        sm::intc_raise_doorbell_irq();
                        debug!("virtio-msg driver ok");
                    }
                } else if state.status & VIRTIO_CONFIG_S_DRIVER != 0 {
                    // The only valid transitions from DRIVER are FEATURES_OK, FAILED or RESET
                    valid_transition = req.status & VIRTIO_CONFIG_S_FEATURES_OK != 0;
                    if valid_transition {
                        debug!("virtio-msg driver finished feature negotiation");
                    }
                } else if state.status & VIRTIO_CONFIG_S_ACKNOWLEDGE != 0 {
                    // The only valid transitions from ACKNOWLEDGE are DRIVER, FAILED or RESET
                    valid_transition = req.status & VIRTIO_CONFIG_S_DRIVER != 0;
                    if valid_transition {
                        debug!("virtio-msg driver knows how to drive the device");
                    }
                } else {
                    // The only valid transitions from the RESET state are ACKNOWLEDGE, FAILED or RESET
                    valid_transition = req.status & VIRTIO_CONFIG_S_ACKNOWLEDGE != 0;
                    if valid_transition {
                        debug!("virtio-msg driver recognized device as valid");
                    }
                };
                if !valid_transition {
                    // Make a copy of the new status in order to silence compiler errors
                    // about warn! taking a reference to a field in a packed structure
                    let new_status = req.status;
                    warn!(
                        "virtio-msg driver attempted invalid transition to status {new_status:x?}"
                    );
                    // virtio-msg protocol doesn't accept a response here
                    return Err(LkError::ERR_INVALID_ARGS);
                }
                // Only store the new status if the transition was valid
                state.status = req.status;
            }
            VirtioMsgPayload::GetDeviceStatus => {
                debug!("received virtio-msg get_device_status request");
                let state = self.state.lock_unsaved();
                resp.write_get_device_status(state.status);
            }
            VirtioMsgPayload::GetFeatures(req) => {
                debug!("received virtio-msg get_features request {req:x?}");
                resp.get_features(req.index, SUPPORTED_VIRTIO_FEATURES);
            }
            VirtioMsgPayload::SetFeatures(req) => {
                debug!("received virtio-msg set_features request {req:x?}");
                let requested_features = req.features[0];
                if requested_features & SUPPORTED_VIRTIO_FEATURES != SUPPORTED_VIRTIO_FEATURES {
                    warn!("driver does not support required features: {requested_features:x?}");
                }
                // TODO: don't force F_ACCESS_PLATFORM once virtio-drivers accepts it
                let negotiated_features =
                    (requested_features & SUPPORTED_VIRTIO_FEATURES) | VIRTIO_F_ACCESS_PLATFORM;
                resp.set_features(0, negotiated_features);
            }
            VirtioMsgPayload::GetVqueue(req) => {
                debug!("received virtio-msg get_vqueue request {req:x?}");
                let state = self.state.lock_unsaved();
                let vqueue = match state.vqueues.get(req.index as usize) {
                    Some(vq) => vq.as_ref(),
                    None => {
                        return Err(LkError::ERR_INVALID_ARGS);
                    }
                };
                resp.write_get_vqueue(req.index, vqueue);
            }
            VirtioMsgPayload::AreaShare(req) => {
                debug!("received virtio-msg-ffa area_share request {req:x?}");
                let success = 'block: {
                    let idx = usize::from(req.area_id);
                    // This device implementation only supports MAX_NUM_SHM shared memory regions
                    if idx > MAX_NUM_SHM {
                        break 'block false;
                    }
                    // If the device already received an area share request for this area ID return
                    // an error. state.share_requests has MAX_NUM_SHM elements so this indexing
                    // can't panic.
                    let mut state = self.state.lock_unsaved();
                    if state.share_requests[idx].is_some() {
                        break 'block false;
                    }
                    state.share_requests[idx] = Some(req.mem_handle);
                    true
                };
                resp.write_bus_area_share(req.area_id, success);
            }
            VirtioMsgPayload::SetVqueue(req) => {
                debug!("received virtio-msg set_vqueue request {req:x?}");
                let mut state = self.state.lock_unsaved();
                let vqueue = match state.vqueues.get_mut(req.index as usize) {
                    Some(vq) => vq,
                    None => {
                        return Err(LkError::ERR_INVALID_ARGS);
                    }
                };
                *vqueue = Some(VirtQueue {
                    size: req.size,
                    desc_table: req.descriptor_addr as BusAddress,
                    avail_ring: req.driver_addr as BusAddress,
                    used_ring: req.device_addr as BusAddress,
                });
                // leave request buffer the same
                resp.set_vqueue(
                    req.index,
                    req.size,
                    req.descriptor_addr,
                    req.driver_addr,
                    req.device_addr,
                )
            }
            VirtioMsgPayload::AreaUnshare(req) => {
                debug!("received virtio-msg-ffa area_unshare request {req:x?}");
                let idx = req.area_id as u8;
                if idx as u32 != req.area_id {
                    return Err(LkError::ERR_INVALID_ARGS);
                }
                // Take ownership of the ExtMemObj from memory_map in the device if it exists
                let mapped_mem = self.state.lock_unsaved().memory_map[usize::from(idx)].take();

                // If there was an ExtMemObj for the entry move ownership to
                // unshare_requests since it cannot be dropped (i.e. unmapped
                // from the kernel address space) in this thread.
                if let Some(ext_mem_obj) = mapped_mem {
                    let unshare_req_entry =
                        &mut self.unshare_requests.lock_unsaved()[usize::from(idx)];
                    if unshare_req_entry.is_some() {
                        return Err(LkError::ERR_BAD_STATE);
                    }
                    *unshare_req_entry = Some(ext_mem_obj);
                }
                // Notify the thread which handles the unmapping of the new request.
                self.wake_memory_unmap.signal();
                sm::intc_raise_doorbell_irq();
            }
            VirtioMsgPayload::GetConfig(req) => {
                debug!("received virtio-msg request {req:x?}");
                // The config space for vsock devices must always be in little endian
                let mut config = (self.guest_cid as u64).to_le().unbounded_shr(req.offset);
                if req.size < 8 {
                    let mask = (1u64 << (req.size * 8)) - 1;
                    config &= mask;
                }
                resp.write_get_config(0 /* generation */, req.offset, req.size, config);
            }
            VirtioMsgPayload::ResetVqueue(req) => {
                warn!("ignoring unsupported reset vqueue request {req:x?}");
            }
            VirtioMsgPayload::EventAvail(req) => {
                // TODO: Implement this to avoid the need to poll the virtqueues
                warn!("ignoring unsupported event avail request {req:x?}");
            }
            VirtioMsgPayload::EventUsed(req) => {
                warn!("ignoring unsupported event used request {req:x?}");
            }
            VirtioMsgPayload::EventConfig(req) => {
                warn!("ignoring unsupported event config request {req:x?}");
            }
            VirtioMsgPayload::UnknownBusReq(req_id) => {
                error!("ignoring virtio-msg bus request with unknown id {req_id:x?}");
            }
            VirtioMsgPayload::UnknownDeviceReq(req_id) => {
                error!("ignoring virtio-msg device request with unknown id {req_id:x?}");
            }
        }
        Ok(())
    }
}

// SAFETY: The buf argument must point to a 14-element u64 array without any aliasing references for
// the lifetime of the function.
pub unsafe extern "C" fn handle_req_callback(
    client_id: FFAClientId,
    buf_ptr: *mut u64,
) -> status_t {
    trace!("Received virtio-msg request with VM ID {client_id:?}");

    let device = TRANSPORT.get_device(client_id);

    // SAFETY: Safety requirements delegated to the callee.
    let buf_ref =
        unsafe { buf_ptr.cast::<[u64; ARM_FFA_MSG_EXTENDED_ARGS_COUNT]>().as_mut().unwrap() };
    match device.handle_req(buf_ref) {
        Ok(_) => 0,
        Err(e) => status_t::from(e),
    }
}
