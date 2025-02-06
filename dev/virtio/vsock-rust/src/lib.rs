#![no_std]
#![allow(non_camel_case_types)]
#![feature(cfg_version)]
// C string byte counts were stabilized in Rust 1.79
#![cfg_attr(not(version("1.79")), feature(cstr_count_bytes))]
// C string literals were stabilized in Rust 1.77
#![cfg_attr(not(version("1.77")), feature(c_str_literals))]
// unsigned_is_multiple_of feature was added in Rust 1.82
#![cfg_attr(version("1.82"), feature(unsigned_is_multiple_of))]

mod err;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod kvm;
mod pci;
mod vsock;

pub use err::Error;
pub use pci::pci_init_mmio;
