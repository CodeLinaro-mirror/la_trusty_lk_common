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

#![no_std]

use alloc::vec::Vec;
use core::iter::once;
use core::ops::Range;
use core::ptr::NonNull;
use rust_support::mmu::PAGE_SIZE_SHIFT;
use rust_support::paddr_t;
use rust_support::vmm::VmmPageArray;
use rust_support::Error as LkError;

/// Simple region-based memory allocator.
///
/// It takes a physical memory range and manages allocations from
/// that region, returning the physical and corresponding virtual
/// address for every allocation.
pub struct RegionAllocator {
    region: Range<paddr_t>,
    mapping: VmmPageArray,
    allocs: Vec<Range<paddr_t>>,
}

impl RegionAllocator {
    pub fn try_new(region: Range<paddr_t>) -> Result<Self, LkError> {
        // TODO: do we need the VmmPageArray to use UNCACHED or UNCACHED_DEVICE?
        // Cacheable memory will have better performance, but both sides have
        // to use the same type if this is used for a shared memory region.
        let mapping = VmmPageArray::new_physical(
            c"region-alloc",
            region.start,
            region.end - region.start,
            PAGE_SIZE_SHIFT as u8,
            0,
        )?;

        Ok(Self { region, mapping, allocs: Vec::new() })
    }

    pub fn region(&self) -> Range<paddr_t> {
        self.region.clone()
    }

    pub fn alloc(
        &mut self,
        size: usize,
        align: paddr_t,
        zeroed: bool,
    ) -> Option<(paddr_t, NonNull<u8>)> {
        // TODO: this should be debug_assert!
        // but right now those are always disabled on Trusty
        assert!(self.allocs.is_sorted_by_key(|r| r.start));

        if size == 0 {
            return None;
        }

        // Poor man's allocator: linear time first fit
        //
        // Iterate over the unallocated gaps in our region
        // (the space between consecutive allocations) in sorted order:
        //   0. region.start..allocs[0].start
        //   1. allocs[0].end..allocs[1].start
        //   2. allocs[1].end..allocs[2].start
        // ...
        // N-1. allocs[N-2].end..allocs[N-1].start
        //   N. allocs[N-1].end..region.end
        let gap_start_iter = once(self.region.start).chain(self.allocs.iter().map(|r| r.end));
        let gap_end_iter = self.allocs.iter().map(|r| r.start).chain(once(self.region.end));
        for (idx, (gap_start, gap_end)) in gap_start_iter.zip(gap_end_iter).enumerate() {
            let aligned_start = gap_start.next_multiple_of(align);
            if aligned_start >= gap_end {
                continue;
            }
            if size > gap_end - aligned_start {
                // This gap is too small
                continue;
            }

            // `idx` is exactly the index of the gap we are using for
            // our allocation, and `gap[idx]` precedes `allocs[idx]`
            // so `idx` is exactly the position where we need to insert
            // the new allocation.
            //
            // This insertion will take time proportional to `allocs.len() - idx`
            // because it shifts all remaining elements up by one,
            // so this whole loop is overall linear in the number of
            // existing allocations.
            self.allocs.insert(idx, aligned_start..aligned_start + size);

            let start_offset = aligned_start - self.region.start;
            let vaddr = self.mapping.ptr().cast::<u8>().wrapping_add(start_offset);
            // If vaddr is NULL, something went horribly wrong
            let vaddr = NonNull::new(vaddr).expect("Pointer is unexpectedly NULL");
            // Check that it didn't wrap
            assert!(vaddr.as_ptr().cast() >= self.mapping.ptr());

            if zeroed {
                // Safety: `vaddr` points to a block of at least `size` bytes
                // since the VmmPageArray covers the entire region
                unsafe {
                    core::ptr::write_bytes(vaddr.as_ptr(), 0, size);
                }
            }

            return Some((aligned_start, vaddr));
        }

        // No space left in this allocator
        None
    }

    pub fn dealloc(&mut self, paddr: paddr_t) {
        assert!(self.region.contains(&paddr));
        // TODO: this should be debug_assert!
        assert!(self.allocs.is_sorted_by_key(|r| r.start));
        let idx = self
            .allocs
            .binary_search_by_key(&paddr, |r| r.start)
            .expect("Address not found in allocator");

        self.allocs.remove(idx);
    }
}
