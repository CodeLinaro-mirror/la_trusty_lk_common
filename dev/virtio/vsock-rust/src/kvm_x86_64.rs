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

use core::result;

use lazy_static::lazy_static;

const KVM_HC_PKVM_OP: u32 = 20;
const PKVM_GHC_SHARE_MEM: u32 = KVM_HC_PKVM_OP + 1;
const PKVM_GHC_UNSHARE_MEM: u32 = KVM_HC_PKVM_OP + 2;

const KVM_ENOSYS: i64 = -1000;
const KVM_EINVAL: i64 = -22;

/// This CPUID returns the signature and can be used to determine if VM is running under pKVM, KVM
/// or not.
pub const KVM_CPUID_SIGNATURE: u32 = 0x40000000;

lazy_static! {
    static ref IS_PKVM: bool = is_pkvm();
}

use core::fmt;
use core::fmt::Display;
use core::fmt::Formatter;

/// Error from a KVM HVC call.
/// TODO: Switch to hypervisor_backends::KvmError once libhypervisor_backends will support x86_64
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum KvmError {
    /// The call is not supported by the implementation.
    NotSupported,
    /// One of the call parameters has a non-supported value.
    InvalidParameter,
    /// There was an unexpected return value.
    Unknown(i64),
}

impl Display for KvmError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::NotSupported => write!(f, "KVM call not supported"),
            Self::InvalidParameter => write!(f, "KVM call received non-supported value"),
            Self::Unknown(e) => write!(f, "Unknown return value from KVM {} ({0:#x})", e),
        }
    }
}

impl From<i64> for KvmError {
    fn from(value: i64) -> Self {
        match value {
            KVM_ENOSYS => KvmError::NotSupported,
            KVM_EINVAL => KvmError::InvalidParameter,
            _ => KvmError::Unknown(value),
        }
    }
}

/// Result type with kvm error.
pub type Result<T> = result::Result<T, KvmError>;

/// This function is responsible for triggering PKVM_GHC_SHARE_MEM hypercall. In case KVM/pKVM
/// hypervisor doesn't support it in a given version KVM_ENOSYS will be returned.
pub(crate) fn share_pages(paddr: usize, size: usize) -> Result<()> {
    // No-op for non-pKVM
    if !(*IS_PKVM) {
        return Ok(());
    }

    let ret: u32;
    // SAFETY:
    // Any undeclared register aren't clobbered except rbx but rbx value is restored at the end of
    // the asm block.
    unsafe {
        core::arch::asm!(
                "push rbx",
                "mov rbx, r8",
                "vmcall",
                "pop rbx",
                inout("rax") PKVM_GHC_SHARE_MEM => ret,
                in("r8") paddr,
                in("rcx") size);
    };

    if ret != 0 {
        return Err(KvmError::from((ret as i32) as i64));
    }

    Ok(())
}

/// This function is responsible for triggering PKVM_GHC_UNSHARE_MEM hypercall. In case KVM/pKVM
/// hypervisor doesn't support it in a given version KVM_ENOSYS will be returned.
pub(crate) fn unshare_pages(paddr: usize, size: usize) -> Result<()> {
    // No-op for non-pKVM
    if !(*IS_PKVM) {
        return Ok(());
    }

    let ret: u32;
    // SAFETY:
    // Any undeclared register aren't clobbered except rbx but rbx value is restored at the end of
    // the asm block.
    unsafe {
        core::arch::asm!(
                "push rbx",
                "mov rbx, r8",
                "vmcall",
                "pop rbx",
                inout("rax") PKVM_GHC_UNSHARE_MEM => ret,
                in("r8") paddr, in("rcx") size);
    };

    if ret != 0 {
        return Err(KvmError::from((ret as i32) as i64));
    }

    Ok(())
}

///TODO: Switch to hypervisor backends lib once x86_64 support will be ready
pub(crate) fn is_pkvm() -> bool {
    let s_kvm: u32;
    // SAFETY:
    // Any undeclared register aren't clobbered except rbx but rbx value is restored at the end of
    // the asm block.
    unsafe {
        // The argument for cpuid is passed via rax and in case of KVM_CPUID_SIGNATURE returned via
        // rbx, rcx and rdx. Ideally using named arguments in inline asm for rbx would be much
        // more straightforward but when rbx is directly used LLVM complains that:
        //      error: cannot use register `bx`: rbx is used internally by LLVM
        //      and cannot be used as an operand for inline asm
        //
        // Therefore use r8 instead and push rbx to the stack before making cpuid call, store
        // rbx content to r8 and use it as inline asm output, finally pop the rbx to restore
        // original value.
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov r8, rbx",
            "pop rbx",
            in("eax") KVM_CPUID_SIGNATURE, out("r8") s_kvm, out("rcx") _, out ("rdx") _);
    };

    // Check if running on pKVM or not
    s_kvm.to_le_bytes() == "PKVM".as_bytes()
}
