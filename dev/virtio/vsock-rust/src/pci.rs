/*
 * Copyright (c) 2024 Google Inc. All rights reserved
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

#![deny(unsafe_op_in_unsafe_fn)]
use core::ffi::c_int;
use core::ptr;

use alloc::sync::Arc;

use log::{debug, error};

use virtio_drivers::device::socket::VirtIOSocket;
use virtio_drivers::device::socket::VsockConnectionManager;
use virtio_drivers::transport::pci::bus::Cam;
use virtio_drivers::transport::pci::bus::Command;
use virtio_drivers::transport::pci::bus::ConfigurationAccess;
use virtio_drivers::transport::pci::bus::MmioCam;
use virtio_drivers::transport::pci::bus::PciRoot;
use virtio_drivers::transport::pci::virtio_device_type;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::SomeTransport;
#[cfg(target_arch = "x86_64")]
use {
    hypervisor_backends::get_mem_sharer,
    virtio_drivers::transport::x86_64::{HypCam, HypPciTransport},
};

use virtio_drivers::transport::DeviceType;

use hypervisor::mmio_map_region;

use rust_support::mmu::ARCH_MMU_FLAG_PERM_NO_EXECUTE;
use rust_support::mmu::ARCH_MMU_FLAG_UNCACHED_DEVICE;
use rust_support::paddr_t;
use rust_support::thread::Builder;
use rust_support::thread::Priority;
use rust_support::vmm::vmm_alloc_physical;
use rust_support::vmm::vmm_get_kernel_aspace;
use rust_support::Error as LkError;

use crate::err::Error;
use crate::vsock::VsockDevice;
use hal::TrustyHal;

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod arch;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
#[path = "pci/unimplemented.rs"]
mod arch;
mod hal;

impl TrustyHal {
    fn init_vsock<T: virtio_drivers::transport::Transport + 'static + Send>(
        driver: virtio_drivers::device::socket::VirtIOSocket<TrustyHal, T, 4096>,
    ) -> Result<(), Error> {
        let manager = VsockConnectionManager::new_with_capacity(driver, 4096);
        let device_for_rx = Arc::new(VsockDevice::new(manager));
        let device_for_tx = device_for_rx.clone();

        // In some builds, stack overflows can occur on both threads when using 4k stacks
        let stack_size = 8192usize;
        Builder::new()
            .name(c"virtio_vsock_rx")
            .priority(Priority::HIGH)
            .stack_size(stack_size)
            .spawn(move || {
                let ret = crate::vsock::vsock_rx_loop(device_for_rx);
                error!("vsock_rx_loop returned {:?}", ret);
                ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
            })
            .map_err(|e| LkError::from_lk(e).unwrap_err())?;

        Builder::new()
            .name(c"virtio_vsock_tx")
            .priority(Priority::HIGH)
            .stack_size(stack_size)
            .spawn(move || {
                let ret = crate::vsock::vsock_tx_loop(device_for_tx);
                error!("vsock_tx_loop returned {:?}", ret);
                ret.err().unwrap_or(LkError::NO_ERROR.into()).into_c()
            })
            .map_err(|e| LkError::from_lk(e).unwrap_err())?;

        Ok(())
    }

    fn init_all_vsocks(
        mut pci_root: PciRoot<impl ConfigurationAccess>,
        pci_size: usize,
        use_hyp_transport: bool,
    ) -> Result<(), Error> {
        for bus in u8::MIN..=u8::MAX {
            // each bus can use up to one megabyte of address space, make sure we stay in range
            if bus as usize * 0x100000 >= pci_size {
                break;
            }
            for (device_function, info) in pci_root.enumerate_bus(bus) {
                if virtio_device_type(&info) != Some(DeviceType::Socket) {
                    continue;
                };

                // Map the BARs of the device into virtual memory. Since the mappings must
                // outlive the `PciTransport` constructed in `init_vsock` we no make no
                // attempt to deallocate them.
                Self::mmio_alloc(&mut pci_root, device_function)?;

                // Enable the device to use its BARs.
                pci_root.set_command(
                    device_function,
                    Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER,
                );

                // In contrast to arm64, when Trusty runs as a protected VM on x86_64, emulated
                // MMIO accesses require special handling. On x86_64, the host needs to read and
                // decode guest instructions that caused the MMIO access trap to determine the
                // address and access type. However, due to pKVM nature, the host cannot read
                // protected VM memory. To address this, pKVM supports hypercalls for IOREAD and
                // IOWRITE, which the guest can use.
                //
                // The virtio-drivers' HypPciTransport is based on mentioned hypercalls and
                // therefore is used for x86 protected Trusty, while PciTransport is used
                // otherwise.
                let transport = if use_hyp_transport {
                    #[cfg(target_arch = "x86_64")]
                    {
                        SomeTransport::HypPci(HypPciTransport::new::<_>(
                            &mut pci_root,
                            device_function,
                        )?)
                    }

                    #[cfg(not(target_arch = "x86_64"))]
                    panic!("HypPciTransport is x86_64 specific");
                } else {
                    SomeTransport::Pci(PciTransport::new::<Self, _>(
                        &mut pci_root,
                        device_function,
                    )?)
                };

                let driver = VirtIOSocket::new(transport)?;
                Self::init_vsock(driver)?;
            }
        }
        Ok(())
    }
}

/// # Safety
///
/// `pci_paddr` must be a valid physical address with `'static` lifetime to the base of the MMIO region,
/// which must have a size of `pci_size`.
unsafe fn map_pci_root_and_init_vsock(
    pci_paddr: paddr_t,
    pci_size: usize,
    cfg_size: usize,
) -> Result<(), Error> {
    // The ECAM is defined in Section 7.2.2 of the PCI Express Base Specification, Revision 2.0.
    // The ECAM size must be a power of two with the exponent between 1 and 8.
    let cam = match cfg_size / /* device functions */ 8 {
        256 => Cam::MmioCam,
        4096 => Cam::Ecam,
        _ => return Err(LkError::ERR_BAD_LEN.into()),
    };

    if !pci_size.is_power_of_two() || pci_size > cam.size() as usize {
        return Err(LkError::ERR_BAD_LEN.into());
    }
    // The ECAM base must be 2^(n + 20)-bit aligned.
    if cam == Cam::Ecam && pci_paddr & (pci_size - 1) != 0 {
        return Err(LkError::ERR_INVALID_ARGS.into());
    }

    // Map the PCI configuration space.
    let pci_vaddr = ptr::null_mut();
    // Safety:
    // `aspace` is `vmm_get_kernel_aspace()`.
    // `name` is a `&'static CStr`.
    // `pci_paddr` and `pci_size` are safe by this function's safety requirements.
    let e = unsafe {
        vmm_alloc_physical(
            vmm_get_kernel_aspace(),
            c"pci_config_space".as_ptr(),
            pci_size,
            &pci_vaddr,
            0,
            pci_paddr,
            0,
            ARCH_MMU_FLAG_PERM_NO_EXECUTE | ARCH_MMU_FLAG_UNCACHED_DEVICE,
        )
    };
    LkError::from_lk(e)?;

    // Safety:
    // `pci_paddr` and `pci_size` are safe by this function's safety requirements.
    match unsafe { mmio_map_region(pci_paddr, pci_size) } {
        // Ignore not supported which implies that guard is not used.
        Ok(()) | Err(LkError::ERR_NOT_SUPPORTED) => {}
        Err(err) => return Err(Error::Lk(err)),
    }

    #[cfg(not(target_arch = "x86_64"))]
    let use_hyp_transport = false;

    // x86 when running in protected mode requires hyp transport
    #[cfg(target_arch = "x86_64")]
    let use_hyp_transport = get_mem_sharer().is_some();

    if use_hyp_transport {
        #[cfg(target_arch = "x86_64")]
        {
            let pci_root = PciRoot::new(HypCam::new(pci_paddr, cam));
            TrustyHal::init_all_vsocks(pci_root, pci_size, use_hyp_transport)?;
        }
    } else {
        // Safety:
        // `pci_paddr` is a valid physical address to the base of the MMIO region.
        // `pci_vaddr` is the mapped virtual address of that.
        // `pci_paddr` has `'static` lifetime, and `pci_vaddr` is never unmapped,
        // so it, too, has `'static` lifetime.
        // We also check that the `cam` size is valid.
        let pci_root = PciRoot::new(unsafe { MmioCam::new(pci_vaddr.cast(), cam) });
        TrustyHal::init_all_vsocks(pci_root, pci_size, use_hyp_transport)?;
    }
    Ok(())
}

/// # Safety
///
/// See [`map_pci_root_and_init_vsock`].
#[no_mangle]
pub unsafe extern "C" fn pci_init_mmio(
    pci_paddr: paddr_t,
    pci_size: usize,
    cfg_size: usize,
) -> c_int {
    debug!("initializing vsock: pci_paddr 0x{pci_paddr:x}, pci_size 0x{pci_size:x}");
    || -> Result<(), Error> {
        // Safety: Delegated to `map_pci_root_and_init_vsock`.
        unsafe { map_pci_root_and_init_vsock(pci_paddr, pci_size, cfg_size) }?;
        Ok(())
    }()
    .err()
    .unwrap_or(LkError::NO_ERROR.into())
    .into_c()
}
