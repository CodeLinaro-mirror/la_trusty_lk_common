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

use log::error;
use num_integer::Integer;
use spin::Once;

use rust_support::Error as LkError;

use hypervisor_backends::get_mmio_guard;
use hypervisor_backends::Error as HypError;
use hypervisor_backends::KvmError;

/// The mmio granule size used by the hypervisor
static MMIO_GRANULE: Once<Result<usize, LkError>> = Once::new();

fn get_mmio_granule() -> Result<usize, LkError> {
    *MMIO_GRANULE.call_once(|| {
        let hypervisor = get_mmio_guard()
            .ok_or(LkError::ERR_NOT_SUPPORTED)
            .inspect_err(|_| error!("failed to get hypervisor"))?;

        let granule = hypervisor
            .granule()
            .inspect_err(|e| error!("failed to get granule: {e:?}"))
            .map_err(|_| LkError::ERR_NOT_SUPPORTED)?;

        if !granule.is_power_of_two() {
            error!("invalid memory protection granule");
            return Err(LkError::ERR_INVALID_ARGS);
        }

        Ok(granule)
    })
}

/// # Safety
///  - paddr must be a valid physical address
///  - paddr + size must be a valid physical address
///  - the caller must be aware that after the call the [paddr .. paddr + size] memory
///    is available for reading by the host.
pub unsafe fn mmio_map_region(paddr: usize, size: usize) -> Result<(), LkError> {
    let Some(hypervisor) = get_mmio_guard() else {
        return Ok(());
    };
    let hypervisor_page_size = get_mmio_granule()?;

    if !paddr.is_multiple_of(&hypervisor_page_size) {
        error!("paddr not aligned");
        return Err(LkError::ERR_INVALID_ARGS);
    }

    if !size.is_multiple_of(&hypervisor_page_size) {
        error!("size ({size}) not aligned to page size ({hypervisor_page_size})");
        return Err(LkError::ERR_INVALID_ARGS);
    }

    for page in (paddr..paddr + size).step_by(hypervisor_page_size) {
        hypervisor.map(page).map_err(|err| {
            error!("failed to mmio guard map page 0x{page:x}: {err}");

            // unmap any previously shared mmio pages on error
            // if sharing fail on the first page, the half-open range below is empty
            for prev in (paddr..page).step_by(hypervisor_page_size) {
                // keep going even if we fail
                let _ = hypervisor.unmap(prev);
            }

            match err {
                HypError::KvmError(KvmError::NotSupported, _) => LkError::ERR_NOT_SUPPORTED,
                HypError::KvmError(KvmError::InvalidParameter, _) => LkError::ERR_INVALID_ARGS,
                HypError::KvmError(_, _) => LkError::ERR_GENERIC,
                _ => panic!("MMIO Guard unmap returned unexpected error: {err:?}"),
            }
        })?;
    }

    Ok(())
}
