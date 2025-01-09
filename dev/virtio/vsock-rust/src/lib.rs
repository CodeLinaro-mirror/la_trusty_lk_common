#![no_std]
#![allow(non_camel_case_types)]
#![feature(cfg_version)]
// C string byte counts were stabilized in Rust 1.79
#![cfg_attr(not(version("1.79")), feature(cstr_count_bytes))]
// C string literals were stabilized in Rust 1.77
#![cfg_attr(not(version("1.77")), feature(c_str_literals))]

mod err;
#[cfg(target_arch = "aarch64")]
#[path = "kvm_aarch64.rs"]
mod kvm;
#[cfg(target_arch = "x86_64")]
#[path = "kvm_x86_64.rs"]
mod kvm;
mod pci;
mod vsock;

pub use err::Error;
pub use pci::pci_init_mmio;
