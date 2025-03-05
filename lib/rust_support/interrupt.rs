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

use bitflags::bitflags;

use crate::sys::lk_interrupt_restore;
use crate::sys::lk_interrupt_save;
use crate::sys::lk_ints_disabled;
use crate::sys::spin_lock_save_flags_t;
use crate::sys::spin_lock_saved_state_t;
use crate::sys::SPIN_LOCK_FLAG_INTERRUPTS;
use crate::sys::SPIN_LOCK_FLAG_IRQ;
use crate::sys::SPIN_LOCK_FLAG_IRQ_FIQ;

#[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
use crate::sys::{lk_fiqs_disabled, SPIN_LOCK_FLAG_FIQ};

fn interrupt_save(state: &mut spin_lock_saved_state_t, flags: spin_lock_save_flags_t) {
    // SAFETY: `lk_interrupt_save` can be called in any context,
    // and the `statep` arg is from a `&mut`, so it can be written to.
    unsafe { lk_interrupt_save(state, flags) }
}

/// `state` should be from the corresponding call to [`interrupt_save`],
/// and `flags` should be the same `flags` from that [`interrupt_save`] call.
fn interrupt_restore(state: spin_lock_saved_state_t, flags: spin_lock_save_flags_t) {
    // SAFETY: `lk_interrupt_restore` is safe and can be called in any context.
    unsafe { lk_interrupt_restore(state, flags) }
}

pub fn irqs_disabled() -> bool {
    // SAFETY: `lk_ints_disabled` can be called in any context.
    unsafe { lk_ints_disabled() }
}

#[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
fn fiqs_disabled() -> bool {
    // SAFETY: `lk_fiqs_disabled` can be called in any context.
    unsafe { lk_fiqs_disabled() }
}

/// See [`InterruptFlags::check`] for what combinations of [`InterruptFlags`]
/// are allowed to be disabled compared to what is already disabled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InterruptFlags(u32);

bitflags! {
    impl InterruptFlags: u32 {
        /// Disable IRQs (interrupt requests).
        ///
        /// IRQs (without FIQs) cannot safely (risking deadlocks)
        /// be disabled if only FIQs are currently disabled.
        /// (see [`Self::check`] for more).
        const IRQ = SPIN_LOCK_FLAG_IRQ;

        /// Disable FIQs (fast interrupt requests).
        ///
        /// These are generally lower latency and higher priority than IRQs.
        ///
        /// FIQs (without IRQs) cannot safely (risking deadlocks)
        /// be disabled if only IRQs are currently disabled
        /// (see [`Self::check`] for more).
        ///
        /// Currently, FIQs do not exist on `x86`/`x86_64` `target_arch`es,
        /// while they always exist on `arm`/`aarch64` `target_arch`es.
        /// However, in the latter, they are sometimes merged with IRQs.
        #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
        const FIQ = SPIN_LOCK_FLAG_FIQ;

        /// Disable both IRQs and FIQs.
        ///
        /// This is still defined for convenience on platforms with no FIQs,
        /// where this does not disable FIQs, but FIQs just don't exist at all.
        const IRQ_FIQ = SPIN_LOCK_FLAG_IRQ_FIQ;

        /// Disable interrupts in general in a platform-agnostic way.
        ///
        /// On `arm`/`aarch64` `target_arch`es, this only disables IRQs.
        const INTERRUPTS = SPIN_LOCK_FLAG_INTERRUPTS;
    }
}

impl Default for InterruptFlags {
    fn default() -> Self {
        Self::INTERRUPTS
    }
}

impl InterruptFlags {
    const fn as_raw(&self) -> spin_lock_save_flags_t {
        self.bits()
    }

    /// Returns `self` if `select` is `true`,
    /// or [`Self::empty`] if `select` is `false`.
    pub const fn select(self, select: bool) -> Self {
        if select {
            self
        } else {
            Self::empty()
        }
    }

    /// Checks if these [`InterruptFlags`] can safely (risking potential deadlocks) be disabled
    /// depending on what is currently already disabled.
    ///
    /// More specifically, considering only [IRQ]s and [FIQ]s,
    /// if only [IRQ]s are requested, then only [FIQ]s being disabled already is not allowed,
    /// and if only [FIQ]s are requested, then only [IRQ]s being disabled already is not allowed.
    ///
    /// Panics on error.
    ///
    /// [IRQ]: Self::IRQ
    /// [FIQ]: Self::FIQ
    #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
    #[track_caller]
    pub fn check(&self) {
        let irq = Self::IRQ;
        let fiq = Self::FIQ;

        let this = *self & (irq | fiq);
        let disabled = irq.select(irqs_disabled()) | fiq.select(fiqs_disabled());

        if (this == irq && disabled == fiq) || (this == fiq && disabled == irq) {
            error(this, disabled);
        }

        #[track_caller]
        // This function is cold, so don't inline it to save on code size,
        // which helps the rest of the function be inlined more easily.
        #[cold]
        #[inline(never)]
        fn error(this: InterruptFlags, disabled: InterruptFlags) {
            let [this, disabled] =
                [this, disabled]
                    .map(|flags| if flags == InterruptFlags::IRQ { "IRQ" } else { "FIQ" });
            panic!("can't disable only {this}s for a SpinLock when only {disabled}s are currently disabled");
        }
    }

    #[cfg(not(any(target_arch = "arm", target_arch = "aarch64")))]
    pub fn check(&self) {
        // FIQs don't exist.
    }
}

/// Go through this `trait` instead of just [`InterruptFlags`]
/// so that when the [`InterruptFlags`] are known at compile time,
/// we don't need to store them (see [`IRQInterruptFlags`]).
pub trait GetInterruptFlags: Clone + Copy + Send + Sync {
    /// This should always return the same flags for the same `self`.
    fn get(&self) -> InterruptFlags;
}

impl GetInterruptFlags for InterruptFlags {
    fn get(&self) -> InterruptFlags {
        *self
    }
}

/// A ZST implementation of [`GetInterruptFlags`] that stores the [`InterruptFlags`] at compile time.
/// Most uses should use this (with convenient type aliases in [`interrupt::flags`])
/// instead of [`InterruptFlags`] directly
///
/// [`interrupt::flags`]: crate::interrupt::flags
// TODO use `#![feature(adt_const_params)]` once stabilized,
// with `const FLAGS: InterruptFlags` instead of `u32`.
#[derive(Clone, Copy)]
pub struct ConstInterruptFlags<const FLAGS: u32>;

impl<const FLAGS: u32> GetInterruptFlags for ConstInterruptFlags<FLAGS> {
    fn get(&self) -> InterruptFlags {
        // Truncate instead of retain since the `FLAGS: u32` is unchecked.
        InterruptFlags::from_bits_truncate(FLAGS)
    }
}

pub mod flags {
    use super::*;

    // Type aliases can't be used as constructors,
    // so we define separate `const`s/constructors.
    // This makes initialization much simpler.

    pub type None = ConstInterruptFlags<{ InterruptFlags::empty().bits() }>;
    pub const NONE: None = ConstInterruptFlags;

    pub type Irq = ConstInterruptFlags<{ InterruptFlags::IRQ.bits() }>;
    pub const IRQ: Irq = ConstInterruptFlags;

    #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
    pub type Fiq = ConstInterruptFlags<{ InterruptFlags::FIQ.bits() }>;
    #[cfg(any(target_arch = "arm", target_arch = "aarch64"))]
    pub const FIQ: Fiq = ConstInterruptFlags;

    pub type IrqFiq = ConstInterruptFlags<{ InterruptFlags::IRQ_FIQ.bits() }>;
    pub const IRQ_FIQ: IrqFiq = ConstInterruptFlags;

    pub type Interrupts = ConstInterruptFlags<{ InterruptFlags::INTERRUPTS.bits() }>;
    pub const INTERRUPTS: Interrupts = ConstInterruptFlags;

    pub type All = ConstInterruptFlags<{ InterruptFlags::all().bits() }>;
    pub const ALL: All = ConstInterruptFlags;
}

pub struct InterruptState<F: GetInterruptFlags> {
    state: spin_lock_saved_state_t,
    flags: F,
}

impl<F: GetInterruptFlags> InterruptState<F> {
    /// Disables interrupts and saves the interrupt state.
    pub fn save(flags: F) -> Self {
        let mut state = Default::default();
        let raw_flags = flags.get().as_raw();
        interrupt_save(&mut state, raw_flags);
        Self { state, flags }
    }
}

impl<F: GetInterruptFlags> Drop for InterruptState<F> {
    fn drop(&mut self) {
        let raw_flags = self.flags.get().as_raw();
        interrupt_restore(self.state, raw_flags);
    }
}

impl<F: GetInterruptFlags> InterruptState<F> {
    /// A more explicit restore, equivalent to [`drop`].
    pub fn restore(self) {
        drop(self)
    }
}
