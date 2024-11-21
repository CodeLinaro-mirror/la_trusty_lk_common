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

use log::error;

use num_integer::Integer;

use hypervisor_backends::get_mem_sharer;
use hypervisor_backends::Error;
use hypervisor_backends::KvmError;

use spin::Once;

/// Result type with kvm error.
pub type KvmResult<T> = Result<T, KvmError>;

/// The granule size used by the hypervisor
static GRANULE: Once<KvmResult<usize>> = Once::new();

fn get_granule() -> KvmResult<usize> {
    *GRANULE.call_once(|| {
        let hypervisor = get_mem_sharer()
            .ok_or(KvmError::NotSupported)
            .inspect_err(|_| error!("failed to get hypervisor"))?;
        let granule = hypervisor
            .granule()
            .inspect_err(|e| error!("failed to get granule: {e:?}"))
            .map_err(|_| KvmError::NotSupported)?;
        if !granule.is_power_of_two() {
            error!("invalid memory protection granule");
            return Err(KvmError::InvalidParameter);
        }
        Ok(granule)
    })
}

pub(crate) fn share_pages(paddr: usize, size: usize) -> KvmResult<()> {
    let hypervisor = get_mem_sharer()
        .ok_or(KvmError::NotSupported)
        .inspect_err(|_| error!("failed to get hypervisor"))?;
    let hypervisor_page_size = get_granule()?;

    if !paddr.is_multiple_of(&hypervisor_page_size) {
        error!("paddr not aligned");
        return Err(KvmError::InvalidParameter);
    }

    if !size.is_multiple_of(&hypervisor_page_size) {
        error!("size ({size}) not aligned to page size ({hypervisor_page_size})");
        return Err(KvmError::InvalidParameter);
    }

    for page in (paddr..paddr + size).step_by(hypervisor_page_size) {
        hypervisor.share(page as u64).map_err(|err| {
            error!("failed to share page 0x{page:x}: {err}");

            // unmap any previously shared pages on error
            // if sharing fail on the first page, the half-open range below is empty
            for prev in (paddr..page).step_by(hypervisor_page_size) {
                // keep going even if we fail
                let _ = hypervisor.unshare(prev as u64);
            }

            match err {
                Error::KvmError(e, _) => e,
                _ => panic!("unexpected share error: {err:?}"),
            }
        })?;
    }

    Ok(())
}

pub(crate) fn unshare_pages(paddr: usize, size: usize) -> KvmResult<()> {
    let hypervisor = get_mem_sharer()
        .ok_or(KvmError::NotSupported)
        .inspect_err(|_| error!("failed to get hypervisor"))?;

    let hypervisor_page_size = get_granule()?;

    if !hypervisor_page_size.is_power_of_two() {
        error!("invalid memory protection granule");
        return Err(KvmError::InvalidParameter);
    }

    if !paddr.is_multiple_of(&hypervisor_page_size) {
        error!("paddr not aligned");
        return Err(KvmError::InvalidParameter);
    }

    if !size.is_multiple_of(&hypervisor_page_size) {
        error!("size ({size}) not aligned to page size ({hypervisor_page_size})");
        return Err(KvmError::InvalidParameter);
    }

    for page in (paddr..paddr + size).step_by(hypervisor_page_size) {
        hypervisor.unshare(page as u64).map_err(|err| {
            error!("failed to unshare page 0x{page:x}: {err:?}");

            match err {
                Error::KvmError(e, _) => e,
                _ => panic!("unexpected unshare error: {err:?}"),
            }
        })?;
    }

    Ok(())
}
