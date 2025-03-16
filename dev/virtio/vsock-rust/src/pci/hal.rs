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

use core::ops::Deref;
use core::ops::DerefMut;
use core::ptr::NonNull;

use lazy_static::lazy_static;

use hypervisor::mmio_map_region;

use rust_support::mmu::ARCH_MMU_FLAG_PERM_NO_EXECUTE;
use rust_support::mmu::ARCH_MMU_FLAG_UNCACHED_DEVICE;
use rust_support::paddr_t;
use rust_support::sync::Mutex;
use rust_support::vaddr_t;
use rust_support::vmm::vmm_alloc_physical;
use rust_support::vmm::vmm_get_kernel_aspace;
use rust_support::Error as LkError;

use static_assertions::const_assert_eq;

use virtio_drivers::transport::pci::bus::ConfigurationAccess;
use virtio_drivers::transport::pci::bus::DeviceFunction;
use virtio_drivers::transport::pci::bus::PciRoot;
use virtio_drivers::{BufferDirection, Hal, PhysAddr, PAGE_SIZE};

use crate::err::Error;
use crate::pci::arch;

#[derive(Copy, Clone)]
struct BarInfo {
    paddr: paddr_t,
    size: usize,
    vaddr: vaddr_t,
}

const NUM_BARS: usize = 6;
lazy_static! {
    static ref BARS: Mutex<[Option<BarInfo>; NUM_BARS]> = Mutex::new([None; NUM_BARS]);
}

// virtio-drivers requires 4k pages, check that we meet requirement
const_assert_eq!(PAGE_SIZE, rust_support::mmu::PAGE_SIZE as usize);

pub struct TrustyHal;

impl TrustyHal {
    pub fn mmio_alloc(
        pci_root: &mut PciRoot<impl ConfigurationAccess>,
        device_function: DeviceFunction,
    ) -> Result<(), Error> {
        for bar in 0..NUM_BARS {
            let bar_info = pci_root.bar_info(device_function, bar as u8).unwrap();
            if let Some((bar_paddr, bar_size)) = bar_info.memory_address_size() {
                let bar_vaddr = core::ptr::null_mut();
                let bar_size_aligned = (bar_size as usize + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

                // Safety:
                // `aspace` is `vmm_get_kernel_aspace()`.
                // `name` is a `&'static CStr`.
                // `bar_paddr` and `bar_size_aligned` are safe by this function's safety requirements.
                let ret = unsafe {
                    vmm_alloc_physical(
                        vmm_get_kernel_aspace(),
                        c"pci_config_space".as_ptr(),
                        bar_size_aligned,
                        &bar_vaddr,
                        0,
                        bar_paddr as usize,
                        0,
                        ARCH_MMU_FLAG_PERM_NO_EXECUTE | ARCH_MMU_FLAG_UNCACHED_DEVICE,
                    )
                };
                LkError::from_lk(ret)?;

                // Safety:
                // `bar_paddr` and `bar_size_aligned` are safe by this function's safety requirements.
                match unsafe { mmio_map_region(bar_paddr as usize, bar_size_aligned) } {
                    // Ignore not supported which implies that guard is not used.
                    Ok(()) | Err(LkError::ERR_NOT_SUPPORTED) | Err(LkError::ERR_INVALID_ARGS) => {}
                    Err(err) => {
                        log::error!("mmio_map_region returned unexpected error: {:?}", err);
                        return Err(Error::Lk(err));
                    }
                }

                BARS.lock().deref_mut()[bar] = Some(BarInfo {
                    paddr: bar_paddr as usize,
                    size: bar_size_aligned,
                    vaddr: bar_vaddr as usize,
                });
            }
        }
        Ok(())
    }
}

// Safety: TrustyHal is stateless and thus trivially safe to send to another thread
unsafe impl Send for TrustyHal {}

// Safety: See function specific comments
unsafe impl Hal for TrustyHal {
    // Safety:
    // Function either returns a non-null, properly aligned pointer or panics the kernel.
    // The call to `vmm_alloc_contiguous` ensures that the pointed to memory is zeroed.
    fn dma_alloc(pages: usize, direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let size = pages * PAGE_SIZE;
        let (paddr, vaddr) = crate::hal::dma_alloc(pages, direction);
        arch::dma_alloc_share(paddr, size);
        (paddr, vaddr)
    }

    // Safety: `vaddr` was returned by `dma_alloc` and hasn't been deallocated.
    unsafe fn dma_dealloc(paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        let size = pages * PAGE_SIZE;
        arch::dma_dealloc_unshare(paddr, size);

        // Safety:
        // `vaddr` was returned by `dma_alloc` and hasn't been deallocated.
        unsafe { crate::hal::dma_dealloc(paddr, vaddr, pages) }
    }

    // Only used for MMIO addresses within BARs read from the device,
    // for the PCI transport.
    //
    // Safety: `paddr` and `size` are validated against allocations made in
    // `Self::mmio_alloc`; panics on validation failure.
    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        for bar in BARS.lock().deref().iter().flatten() {
            let bar_paddr_end = bar.paddr + bar.size;
            if (bar.paddr..bar_paddr_end).contains(&paddr) {
                // check that the address range up to the given size is within
                // the region expected for MMIO.
                if paddr + size > bar_paddr_end {
                    panic!("invalid arguments passed to mmio_phys_to_virt");
                }
                let offset = paddr - bar.paddr;

                let bar_vaddr_ptr: *mut u8 = bar.vaddr as _;
                // Safety:
                // - `BARS` correctly maps from physical to virtual pages
                // - `offset` is less than or equal to bar.size because
                //   `bar.paddr` <= `paddr`` < `bar_paddr_end`
                let vaddr = unsafe { bar_vaddr_ptr.add(offset) };
                return NonNull::<u8>::new(vaddr).unwrap();
            }
        }

        panic!("error mapping physical memory to virtual for mmio");
    }

    // Safety: delegated to callee
    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        // Safety: delegated to arch::share
        unsafe { arch::share(buffer, direction) }
    }

    // Safety: delegated to callee
    unsafe fn unshare(paddr: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
        // Safety: delegated to arch::unshare
        unsafe {
            arch::unshare(paddr, buffer, direction);
        }
    }
}
