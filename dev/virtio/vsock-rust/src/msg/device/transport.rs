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

use crate::msg::device::DeviceState;
use crate::msg::device::VirtioMsgDevice;
use crate::msg::device::VSOCK_QUEUE_SIZE;
use crate::msg::BusAddress;
use crate::FFAClientId;
use alloc::sync::Arc;
use rust_support::spinlock::IRQSpinLock;
use virtio_drivers_and_devices::transport::DeviceTransport;

pub struct FFAMsgTransport {
    state: Arc<IRQSpinLock<DeviceState>>,
    client_id: u16,
}

impl FFAMsgTransport {
    pub fn new(device: &VirtioMsgDevice, client_id: FFAClientId) -> Self {
        let state = device.state.clone();
        Self { state, client_id }
    }
}

impl DeviceTransport for FFAMsgTransport {
    fn get_client_id(&self) -> u16 {
        self.client_id
    }
    fn max_queue_size(&mut self, _queue: u16) -> u32 {
        VSOCK_QUEUE_SIZE
    }

    fn requires_legacy_layout(&self) -> bool {
        true
    }

    // virtio-drivers uses this to get the addresses passed to TrustyDeviceHal::dma_map so they must
    // be bus addresses
    fn queue_get(&mut self, queue: u16) -> [BusAddress; 3] {
        let state = self.state.lock_save();
        let vq = state
            .vqueues
            .get(usize::from(queue))
            .expect("tried to initialize invalid queue")
            .as_ref()
            .expect("tried to initialize unconfigured queue");
        [vq.desc_table, vq.avail_ring, vq.used_ring]
    }

    fn notify(&mut self, _queue: u16) {
        // nop for now
    }
}
