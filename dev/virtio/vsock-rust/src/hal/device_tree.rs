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

use alloc::vec::Vec;
use core::ops::Range;
use dtb_service::{get_dtb, DtbServiceError};
use libfdt::{FdtError, FdtNode};

fn parse_fdt_dma_pool_reg(dma_pool: &FdtNode) -> libfdt::Result<Range<u64>> {
    let mut reg_iter = dma_pool.reg()?.ok_or(FdtError::NotFound)?;
    let reg = reg_iter.next().ok_or(FdtError::NotFound)?;
    let reg_size = reg.size.ok_or(FdtError::NotFound)?;
    let reg_end = reg.addr.checked_add(reg_size).ok_or(FdtError::BadValue)?;
    Ok(reg.addr..reg_end)
}

// The compiler complains about dead code if the fields are only
// used for debugging; silence those warnings for now.
#[allow(dead_code)]
#[derive(Debug)]
enum DmaPoolError {
    GetDtb(DtbServiceError),
    FindCompatible(FdtError),
    ParseReg(FdtError),
}

/// Parse and return all restricted-dma-pool nodes from the FDT.
///
/// This calls [`dtb:service::get_dtb`] to get the device tree.
/// On generic-arm64, that is only available from an init level strictly higher than
/// [`LK_INIT_LEVEL_VM`], so this function will fail if called earlier.
fn get_fdt_dma_pools() -> Result<Vec<Range<u64>>, DmaPoolError> {
    let fdt = get_dtb().map_err(DmaPoolError::GetDtb)?;
    let dma_pools =
        fdt.compatible_nodes(c"restricted-dma-pool").map_err(DmaPoolError::FindCompatible)?;

    let mut result = Vec::new();
    for dma_pool in dma_pools {
        let pool = parse_fdt_dma_pool_reg(&dma_pool).map_err(DmaPoolError::ParseReg)?;
        result.push(pool);
    }
    Ok(result)
}
