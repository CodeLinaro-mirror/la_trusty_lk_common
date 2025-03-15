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

use core::cell::UnsafeCell;
use core::ffi::c_int;
use core::fmt;
use core::fmt::Debug;
use core::fmt::Formatter;
use core::ops::Deref;
use core::ops::DerefMut;

use crate::interrupt;
use crate::interrupt::ConstInterruptFlags;
use crate::interrupt::GetInterruptFlags;
use crate::interrupt::InterruptState;
use crate::sys::lk_spin_lock;
use crate::sys::lk_spin_trylock;
use crate::sys::lk_spin_unlock;
use crate::sys::spin_lock_t;
use crate::sys::SPIN_LOCK_INITIAL_VALUE;

/// Interrupts must be disabled ([`InterruptState::save`]) before calling this,
/// or else this might deadlock.
/// Furthermore, if [`InterruptFlags::check`] fails (panics), then deadlocks might happen.
///
/// # Safety
///
/// `lock` must have been initialized with [`SPIN_LOCK_INITIAL_VALUE`].
///
/// This function is thread-safe and can be called concurrently from multiple threads.
/// Only one thread is guaranteed to successfully lock it at a time.
unsafe fn spin_lock(lock: *mut spin_lock_t) {
    // SAFETY: See above.
    unsafe { lk_spin_lock(lock) }
}

/// Returns 0 on a successful lock.
///
/// # Safety
///
/// Same as [`spin_lock`] (including deadlock safety).
unsafe fn spin_trylock(lock: *mut spin_lock_t) -> c_int {
    // SAFETY: See above.
    unsafe { lk_spin_trylock(lock) }
}

/// # Safety
///
/// `lock` must already be locked by either [`spin_lock`] or [`spin_trylock`].
unsafe fn spin_unlock(lock: *mut spin_lock_t) {
    // SAFETY: See above.
    unsafe { lk_spin_unlock(lock) }
}

/// A spin lock that does not encapsulate the data it protects.
struct LoneSpinLock {
    inner: UnsafeCell<spin_lock_t>,
}

impl LoneSpinLock {
    pub const fn new() -> Self {
        let inner = UnsafeCell::new(SPIN_LOCK_INITIAL_VALUE as _);
        Self { inner }
    }

    fn get_raw(&self) -> *mut spin_lock_t {
        self.inner.get()
    }
}

impl Default for LoneSpinLock {
    fn default() -> Self {
        Self::new()
    }
}

/// A spin lock wrapping the C [`spin_lock_t`].
/// Its API is modeled after [`std::sync::Mutex`].
///
/// # Specifying [`InterruptFlags`]
///
/// To safely use a [`SpinLock`] without risking deadlocks,
/// [`interrupts`] must be disabled first.
/// Therefore, [`SpinLock`] requires [`InterruptFlags`] to be specified,
/// which it uses to disable interrupts in [`SpinLock::lock_save`] and [`SpinLock::try_lock_save`].
/// This can be bypassed with [`SpinLock::lock_unsaved`] and [`SpinLock::try_lock_unsaved`]
/// if interrupts have already been disabled elsewhere with [`InterruptState::save`].
///
/// To ensure maximal flexibility, these [`InterruptFlags`]
/// can be specified at runtime or at compile time
/// (this is what `F: `[`GetInterruptFlags`] is for).
/// To specify it at runtime, [`InterruptFlags`] can be used directly,
/// but in most cases, the flags are known at compile time already,
/// so [`ConstInterruptFlags`] can be used instead.
/// Since [`ConstInterruptFlags`] uses a const generic,
/// it is much simpler to use the aliases in [`interrupt::flags`] instead,
/// which correspond to the variants of [`InterruptFlags`].
///
/// If this is confusing, the best default is to [`IRQSpinLock`] instead.
/// This type alias is [`SpinLock`] with [`interrupt::flags::Interrupts`],
/// which is the default [`InterruptFlags`].
/// Using this corresponds to `spin_lock_irqsave` and `spin_lock_irqrestore` in C.
///
/// ## Examples
///
/// ### With Runtime [`InterruptFlags`]
///
/// ```
/// let lock = SpinLock::new("value", InterruptFlags::IRQ_FIQ);
/// ```
///
/// ### With Compile Time [`ConstInterruptFlags`]
///
/// ```
/// let lock = SpinLock::new("value", interrupt::flags::IRQ);
/// ```
///
/// ### With [`IRQSpinLock`]
///
/// ```
/// let lock = IRQSpinLock::new_irq("value");
/// ```
///
/// ## [`InterruptFlags`] Runtime Checks
///
/// Certain combinations of [`InterruptFlags`] being disabled and already disabled
/// can lead to deadlocks, so this is checked against at runtime by [`InterruptFlags::check`].
/// See [`InterruptFlags::check`] for what combinations of [`InterruptFlags`] these are.
/// These will be checked before locking, panicking if the check fails.
///
/// [`InterruptFlags`]: interrupt::InterruptFlags
/// [`InterruptFlags::check`]: interrupt::InterruptFlags::check
#[derive(Default)]
pub struct SpinLock<T: ?Sized, F: GetInterruptFlags> {
    lock: LoneSpinLock,
    interrupt_flags: F,
    value: UnsafeCell<T>,
}

impl<T, F: GetInterruptFlags> SpinLock<T, F> {
    /// See [`SpinLock`] for more information on `interrupt_flags`
    /// and the `F: `[`GetInterruptFlags`] generic.
    pub const fn new(value: T, interrupt_flags: F) -> Self {
        Self { lock: LoneSpinLock::new(), interrupt_flags, value: UnsafeCell::new(value) }
    }

    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: ?Sized, F: GetInterruptFlags> SpinLock<T, F> {
    fn get_raw(&self) -> *mut spin_lock_t {
        self.lock.get_raw()
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

pub struct SpinLockGuard<'a, T: ?Sized, F: GetInterruptFlags> {
    lock: &'a SpinLock<T, F>,
}

impl<T: ?Sized, F: GetInterruptFlags> SpinLock<T, F> {
    /// Interrupts must be disabled ([`InterruptState::save`]) before calling this,
    /// or else this might deadlock.
    ///
    /// [`Self::lock_save`] should be preferred.
    ///
    /// This is the analogue to `spin_lock` in C.
    #[track_caller] // For `InterruptFlags::check`.
    pub fn lock_unsaved(&self) -> SpinLockGuard<'_, T, F> {
        self.interrupt_flags.get().check();
        // SAFETY: `LoneSpinLock::new` initialized `self.lock` with `SPIN_LOCK_INITIAL_VALUE`.
        unsafe { spin_lock(self.lock.get_raw()) };
        SpinLockGuard { lock: self }
    }

    /// Interrupts must be disabled ([`InterruptState::save`]) before calling this,
    /// or else this might deadlock.
    ///
    /// [`Self::try_lock_save`] should be preferred.
    ///
    /// This is the analogue to `spin_trylock` in C.
    #[track_caller] // For `InterruptFlags::check`.
    pub fn try_lock_unsaved(&self) -> Option<SpinLockGuard<'_, T, F>> {
        self.interrupt_flags.get().check();
        // SAFETY: `LoneSpinLock::new` initialized `self.lock` with `SPIN_LOCK_INITIAL_VALUE`.
        let status = unsafe { spin_trylock(self.lock.get_raw()) };
        // `spin_trylock` returns 0 on success, but doesn't specify what other non-zero values mean.
        if status != 0 {
            return None;
        }
        Some(SpinLockGuard { lock: self })
    }
}

impl<T: ?Sized, F: GetInterruptFlags> Drop for SpinLockGuard<'_, T, F> {
    fn drop(&mut self) {
        // SAFETY: `SpinLockGuard` is only created with a locked spin lock
        // with either `spin_lock` or `spin_trylock`.
        unsafe { spin_unlock(self.lock.get_raw()) };
    }
}

impl<T: ?Sized, F: GetInterruptFlags> SpinLockGuard<'_, T, F> {
    /// A more explicit unlock, equivalent to [`drop`].
    ///
    /// This is the analogue to `spin_unlock` in C.
    pub fn unlock(self) {
        drop(self);
    }
}

impl<T: ?Sized, F: GetInterruptFlags> Deref for SpinLockGuard<'_, T, F> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Only one thread could have `spin_lock` or `spin_trylock`ed,
        // so we have a unique reference to `self.lock.value`.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T: ?Sized, F: GetInterruptFlags> DerefMut for SpinLockGuard<'_, T, F> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Only one thread could have `spin_lock` or `spin_trylock`ed,
        // so we have a unique reference to `self.lock.value`.
        unsafe { &mut *self.lock.value.get() }
    }
}

/// SAFETY: This is a spin lock wrapping [`spin_lock_t`].
/// [`GetInterruptFlags`] is also `Send + Sync`.
unsafe impl<T: ?Sized + Send, F: GetInterruptFlags> Send for SpinLock<T, F> {}

/// SAFETY: This is a spin lock wrapping [`spin_lock_t`].
/// [`GetInterruptFlags`] is also `Send + Sync`.
unsafe impl<T: ?Sized + Send, F: GetInterruptFlags> Sync for SpinLock<T, F> {}

// Note: We don't impl `UnwindSafe` and `RefUnwindSafe`
// because we don't poison our `SpinLock` upon panicking
// like `std::sync::Mutex` does.

pub struct SpinLockGuardState<'a, T: ?Sized, F: GetInterruptFlags> {
    // The order is important here.
    // The guard needs to be dropped (unlocked) first before the state (is restored).
    guard: SpinLockGuard<'a, T, F>,

    // The `#[allow(dead_code)]` is because this is only used for dropping.
    #[allow(dead_code)]
    state: InterruptState<F>,
}

impl<T: ?Sized, F: GetInterruptFlags> SpinLock<T, F> {
    /// This is the analogue to `spin_lock_save` in C.
    #[track_caller] // For `InterruptFlags::check`.
    pub fn lock_save(&self) -> SpinLockGuardState<'_, T, F> {
        let state = InterruptState::save(self.interrupt_flags);
        let guard = self.lock_unsaved();
        SpinLockGuardState { guard, state }
    }

    /// This has no direct C analogue, but it is a combination of
    /// `spin_trylock` and `spin_lock_save`.
    #[track_caller] // For `InterruptFlags::check`.
    pub fn try_lock_save(&self) -> Option<SpinLockGuardState<'_, T, F>> {
        let state = InterruptState::save(self.interrupt_flags);
        let guard = self.try_lock_unsaved()?;
        Some(SpinLockGuardState { guard, state })
    }
}

impl<T: ?Sized, F: GetInterruptFlags> SpinLockGuardState<'_, T, F> {
    /// A more explicit unlock, equivalent to [`drop`].
    ///
    /// This is the analogue of `spin_unlock_restore` in C.
    pub fn unlock_restore(self) {
        drop(self);
    }
}

impl<T: ?Sized, F: GetInterruptFlags> Deref for SpinLockGuardState<'_, T, F> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.guard.deref()
    }
}

impl<T: ?Sized, F: GetInterruptFlags> DerefMut for SpinLockGuardState<'_, T, F> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.deref_mut()
    }
}

/// Copied from [`std::sync::Mutex`],
/// with minor changes as [`SpinLock`] has no poisoning.
impl<T: ?Sized + Debug, F: GetInterruptFlags> Debug for SpinLock<T, F> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let mut d = f.debug_struct("SpinLock");
        match self.try_lock_save() {
            Some(guard) => {
                d.field("data", &&*guard);
            }
            None => {
                d.field("data", &format_args!("<locked>"));
            }
        }
        d.finish_non_exhaustive()
    }
}

/// [`Self::lock_save`] and [`Self::unlock_restore`] on this type
/// are the analogues of `spin_lock_irqsave` and `spin_lock_irqrestore` in C.
///
/// This type defaults to [`interrupt::flags::Interrupts`]
/// (the ZST version of [`InterruptFlags::INTERRUPTS`])
/// for the `F: `[`GetInterruptFlags`] generic of [`SpinLock`].
/// Generally, this means it will disable IRQs but not FIQs
/// (if they even exist on the platform).
///
/// If you're unsure which [`GetInterruptFlags`] generic to use,
/// this is a good default.
///
/// [`InterruptFlags::INTERRUPTS`]: interrupt::InterruptFlags::INTERRUPTS
pub type IRQSpinLock<T> = SpinLock<T, interrupt::flags::Interrupts>;

impl<T> IRQSpinLock<T> {
    pub const fn new_irq(value: T) -> Self {
        Self::new(value, ConstInterruptFlags)
    }
}
