use core::ptr::NonNull;
use virtio_drivers::BufferDirection;
use virtio_drivers::PhysAddr;

pub(crate) fn dma_alloc_share(_paddr: usize, _size: usize) {
    unimplemented!();
}

pub(crate) fn dma_dealloc_unshare(_paddr: PhysAddr, _size: usize) {
    unimplemented!();
}

// Safety: unimplemented
pub(crate) unsafe fn share(_buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
    unimplemented!();
}

// Safety: unimplemented
pub(crate) unsafe fn unshare(
    _paddr: PhysAddr,
    _buffer: NonNull<[u8]>,
    _direction: BufferDirection,
) {
    unimplemented!();
}
