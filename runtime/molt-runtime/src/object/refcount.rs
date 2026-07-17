//! Typed reference-count storage and transitions for [`MoltHeader`].
//!
//! The storage representation is target-specific, but callers never select
//! atomic orderings or perform arithmetic directly. Default GIL/native and
//! single-thread wasm builds use a zero-atomic `Cell`; native free-threaded
//! builds select the atomic release sequence. Terminal zero, GC pins, immortal
//! objects, and finalizer revival remain one authority in every mode.

use super::{
    HEADER_FLAG_DEALLOCATING, HEADER_FLAG_GC_PINNED, HEADER_FLAG_IMMORTAL, IMMORTAL_REFCOUNT,
    MoltHeader,
};
use molt_obj_model::refcount_semantics::{
    RefCountRelease, RetainError, live_upgrade_next, release_transition, retain_next,
    revival_window_baseline,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
use std::sync::atomic::Ordering;

/// Target-specific storage for the reference-count word in `MoltHeader`.
///
/// This type deliberately has no public atomic-shaped API.  All transitions
/// are expressed through `MoltHeader` methods below so new callers cannot
/// invent a weaker ordering or bypass overflow/terminal-state checks.
#[repr(transparent)]
pub(super) struct MoltRefCount {
    #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
    inner: std::sync::atomic::AtomicU32,
    #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
    inner: std::cell::Cell<u32>,
}

impl MoltRefCount {
    #[inline(always)]
    const fn new(value: u32) -> Self {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            Self {
                inner: std::sync::atomic::AtomicU32::new(value),
            }
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            Self {
                inner: std::cell::Cell::new(value),
            }
        }
    }

    #[inline(always)]
    fn load_relaxed(&self) -> u32 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.inner.load(Ordering::Relaxed)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            self.inner.get()
        }
    }

    #[inline(always)]
    fn load_acquire(&self) -> u32 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.inner.load(Ordering::Acquire)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            self.inner.get()
        }
    }

    #[inline(always)]
    fn store_release(&self, value: u32) {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.inner.store(value, Ordering::Release);
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            self.inner.set(value);
        }
    }

    #[inline(always)]
    fn compare_exchange_retain(&self, current: u32, new: u32) -> Result<u32, u32> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            // Retaining an already-owned live object does not publish payload.
            // Relaxed is the same ordering used by Arc's increment path.
            self.inner
                .compare_exchange_weak(current, new, Ordering::Relaxed, Ordering::Relaxed)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let observed = self.inner.get();
            if observed == current {
                self.inner.set(new);
                Ok(observed)
            } else {
                Err(observed)
            }
        }
    }

    #[inline(always)]
    fn compare_exchange_live_upgrade(&self, current: u32, new: u32) -> Result<u32, u32> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            // A successful upgrade must observe initialization published by the
            // owner whose reference kept `current` non-zero.
            self.inner
                .compare_exchange_weak(current, new, Ordering::Acquire, Ordering::Relaxed)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let observed = self.inner.get();
            if observed == current {
                self.inner.set(new);
                Ok(observed)
            } else {
                Err(observed)
            }
        }
    }

    #[inline(always)]
    fn release_one(&self) -> u32 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            // Standard Arc-style release sequence: only the terminal edge pays
            // for the acquire fence before reading and destroying payload.
            let previous = self.inner.fetch_sub(1, Ordering::Release);
            if previous == 1 {
                std::sync::atomic::fence(Ordering::Acquire);
            }
            previous
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let previous = self.inner.get();
            self.inner.set(previous.wrapping_sub(1));
            previous
        }
    }

    #[inline(always)]
    fn add_internal_pin(&self) -> u32 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.inner.fetch_add(1, Ordering::Relaxed)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let previous = self.inner.get();
            self.inner.set(previous.wrapping_add(1));
            previous
        }
    }

    #[inline(always)]
    fn close_internal_pin(&self) -> u32 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.inner.fetch_sub(1, Ordering::AcqRel)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let previous = self.inner.get();
            self.inner.set(previous.wrapping_sub(1));
            previous
        }
    }
}

const _: () = {
    // The generated ABI fixes the header refcount word at four-byte size and
    // alignment. Selecting Cell or AtomicU32 by concurrency mode must never
    // change object layout for native, WASM, extensions, or serialized IR.
    assert!(std::mem::size_of::<MoltRefCount>() == std::mem::size_of::<u32>());
    assert!(std::mem::align_of::<MoltRefCount>() == std::mem::align_of::<u32>());
};

impl MoltHeader {
    /// Start the lifetime of the refcount field in freshly allocated storage.
    /// Calling an atomic method on merely zero-filled bytes is not sufficient
    /// to construct an `AtomicU32` under Rust's object-lifetime rules.
    #[inline(always)]
    pub(crate) unsafe fn initialize_refcount_before_publication(header: *mut Self, value: u32) {
        unsafe {
            std::ptr::write(
                std::ptr::addr_of_mut!((*header).ref_count),
                MoltRefCount::new(value),
            );
        }
    }

    /// Diagnostic/ABI snapshot under the active concurrency contract. This is
    /// atomic in native free-threaded builds and GIL-confined otherwise. The
    /// value is never an ownership token and cannot justify a later dereference.
    #[inline(always)]
    pub fn ref_count_snapshot(&self) -> u32 {
        self.ref_count.load_acquire()
    }

    /// Snapshot used only while the caller already owns the object or holds
    /// the mutation authority.  It avoids an unnecessary acquire on ARM.
    #[inline(always)]
    pub(crate) fn owned_ref_count_snapshot(&self) -> u32 {
        self.ref_count.load_relaxed()
    }

    #[inline(always)]
    pub(crate) fn is_uniquely_owned(&self) -> bool {
        self.ref_count.load_acquire() == 1
    }

    /// Retain an object through an existing owned reference.
    #[inline(always)]
    pub(crate) fn retain_owned(&self, count: usize, label: &str) -> u32 {
        if self.has_flag(HEADER_FLAG_IMMORTAL) {
            return self.ref_count.load_relaxed();
        }
        if count == 0 {
            let current = self.ref_count.load_relaxed();
            return match retain_next(current, 0, self.has_flag(HEADER_FLAG_DEALLOCATING)) {
                Ok(next) => next,
                Err(RetainError::Zero | RetainError::Deallocating | RetainError::Immortal) => {
                    fatal_terminal_retain(label)
                }
                Err(RetainError::Overflow) => unreachable!("adding zero cannot overflow"),
            };
        }
        let Ok(count) = u32::try_from(count) else {
            fatal_refcount_overflow(label, self.ref_count.load_relaxed(), count);
        };
        let mut current = self.ref_count.load_relaxed();
        loop {
            if current == 0 || self.has_flag(HEADER_FLAG_DEALLOCATING) {
                fatal_terminal_retain(label);
            }
            let next = match retain_next(current, count, self.has_flag(HEADER_FLAG_DEALLOCATING)) {
                Ok(next) => next,
                Err(RetainError::Overflow) => {
                    fatal_refcount_overflow(label, current, count as usize)
                }
                Err(RetainError::Zero | RetainError::Deallocating) => fatal_terminal_retain(label),
                // `HEADER_FLAG_IMMORTAL` returned above. Reaching the numeric
                // sentinel without that lifecycle flag is corrupted state,
                // never permission to silently treat a mortal as immortal.
                Err(RetainError::Immortal) => fatal_terminal_retain(label),
            };
            match self.ref_count.compare_exchange_retain(current, next) {
                Ok(_) => return current,
                Err(observed) => current = observed,
            }
        }
    }

    /// Upgrade registry custody to one ordinary owner only while the object is
    /// live.  Unlike `retain_owned`, zero is an expected miss, not a fatal bug.
    #[inline(always)]
    pub(crate) fn try_retain_live(&self) -> bool {
        if self.has_flag(HEADER_FLAG_DEALLOCATING | HEADER_FLAG_IMMORTAL) {
            return false;
        }
        let mut current = self.ref_count.load_acquire();
        loop {
            let Ok(next) = live_upgrade_next(current, self.has_flag(HEADER_FLAG_DEALLOCATING))
            else {
                return false;
            };
            match self.ref_count.compare_exchange_live_upgrade(current, next) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    /// Release one owned reference.  A terminal result carries the acquire
    /// fence required before payload destruction.
    #[inline(always)]
    pub(crate) fn release_owned(&self, label: &str) -> RefCountRelease {
        let previous = self.ref_count.release_one();
        let Some(transition) = release_transition(previous) else {
            eprintln!("molt fatal: invalid refcount release in {label} (previous={previous})");
            std::process::abort();
        };
        transition
    }

    #[inline(always)]
    /// Sole post-publication transition into immortal custody. The caller must
    /// own runtime mutation authority; arbitrary retain/release code may not
    /// store the sentinel or synthesize immortality from arithmetic.
    pub(crate) fn make_immortal(&self) {
        self.ref_count.store_release(IMMORTAL_REFCOUNT);
    }

    #[inline(always)]
    /// Runtime-shutdown-only inverse of `make_immortal`. Teardown must have
    /// stopped workers and hold exclusive root custody before reopening one
    /// mortal owner; this is never a general resurrection path.
    pub(crate) fn make_mortal_for_shutdown(&self) {
        if self.ref_count.load_acquire() == IMMORTAL_REFCOUNT {
            self.ref_count.store_release(1);
        }
    }

    #[inline(always)]
    /// Bridge-only restoration of the one stable ABI-view hold after a
    /// terminal runtime-owner drop. The bridge identity transaction and GIL
    /// provide exclusive authority for this direct store.
    pub(crate) fn restore_stable_view_hold(&self) {
        self.ref_count.store_release(1);
    }

    #[inline(always)]
    /// Bridge-only retirement of the stable ABI-view hold after every
    /// resurrection opportunity and direct C root has closed.
    pub(crate) fn retire_stable_view_hold(&self) {
        self.ref_count.store_release(0);
    }

    /// Add the collector's temporary strong pin and publish its flag as one
    /// typed lifecycle operation.
    #[inline(always)]
    pub(crate) fn pin_for_gc(&self) {
        if self.has_flag(HEADER_FLAG_GC_PINNED) {
            eprintln!("molt fatal: object pinned twice by cycle collector");
            std::process::abort();
        }
        self.retain_owned(1, "cycle collector pin");
        self.fetch_or_flags(HEADER_FLAG_GC_PINNED);
    }

    /// Open the sole Python-visible finalizer/weakref revival window.  The
    /// terminal path must contain either zero ordinary refs or exactly one
    /// stable ABI-view hold before this internal pin is added.
    #[inline(always)]
    pub(crate) fn open_revival_window(&self, has_stable_view_hold: bool) -> u32 {
        let expected = u32::from(has_stable_view_hold);
        let previous = self.ref_count.add_internal_pin();
        let Some(baseline) = revival_window_baseline(previous, has_stable_view_hold) else {
            eprintln!(
                "molt fatal: invalid refcount opening revival window (expected={expected}, actual={previous})"
            );
            std::process::abort();
        };
        baseline
    }

    #[inline(always)]
    pub(crate) fn close_revival_window(&self) -> u32 {
        let previous = self.ref_count.close_internal_pin();
        if previous == 0 {
            eprintln!("molt fatal: refcount underflow closing revival window");
            std::process::abort();
        }
        previous
    }
}

#[cold]
#[inline(never)]
fn fatal_terminal_retain(label: &str) -> ! {
    eprintln!("molt fatal: owned retain attempted after terminal death in {label}");
    std::process::abort()
}

#[cold]
#[inline(never)]
fn fatal_refcount_overflow(label: &str, current: u32, count: usize) -> ! {
    eprintln!("molt fatal: refcount overflow in {label} (count={current}, add={count})");
    std::process::abort()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::MoltAuxWord;
    use molt_codegen_abi::MoltFlags;

    fn header(count: u32, flags: u32) -> MoltHeader {
        MoltHeader {
            type_id: 0,
            ref_count: MoltRefCount::new(count),
            flags: MoltFlags::new(flags),
            size_class: 0,
            aux_kind: 0,
            aux: MoltAuxWord::new(0),
        }
    }

    #[test]
    fn storage_mode_preserves_header_abi() {
        assert_eq!(
            std::mem::size_of::<MoltRefCount>(),
            std::mem::size_of::<u32>()
        );
        assert_eq!(
            std::mem::align_of::<MoltRefCount>(),
            std::mem::align_of::<u32>()
        );

        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            fn assert_send_sync<T: Send + Sync>() {}
            assert_send_sync::<MoltRefCount>();
        }
    }

    #[test]
    fn typed_transitions_cover_owned_live_gc_immortal_and_revival_states() {
        let owned = header(1, 0);
        assert_eq!(owned.retain_owned(0, "empty batch"), 1);
        assert_eq!(owned.retain_owned(3, "test batch"), 1);
        assert_eq!(owned.ref_count_snapshot(), 4);
        let release = owned.release_owned("test release");
        assert_eq!(release.previous(), 4);
        assert!(!release.reached_zero());
        assert_eq!(owned.ref_count_snapshot(), 3);

        let live = header(1, 0);
        assert!(live.try_retain_live());
        assert_eq!(live.ref_count_snapshot(), 2);
        assert!(!header(0, 0).try_retain_live());
        assert!(!header(IMMORTAL_REFCOUNT, HEADER_FLAG_IMMORTAL).try_retain_live());
        assert!(!header(1, HEADER_FLAG_DEALLOCATING).try_retain_live());

        let gc = header(1, 0);
        gc.pin_for_gc();
        assert_eq!(gc.ref_count_snapshot(), 2);
        assert!(gc.has_flag(HEADER_FLAG_GC_PINNED));

        let ordinary_revival = header(0, 0);
        assert_eq!(ordinary_revival.open_revival_window(false), 1);
        assert_eq!(ordinary_revival.close_revival_window(), 1);
        assert_eq!(ordinary_revival.ref_count_snapshot(), 0);

        let view_revival = header(1, 0);
        assert_eq!(view_revival.open_revival_window(true), 2);
        assert_eq!(view_revival.close_revival_window(), 2);
        assert_eq!(view_revival.ref_count_snapshot(), 1);
    }

    #[test]
    #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
    fn concurrent_retain_release_preserves_the_baseline_owner() {
        use std::sync::{Arc, Barrier};

        const THREADS: usize = 4;
        const ITERATIONS: usize = 4_096;
        let refcount = Arc::new(MoltRefCount::new(1));
        let start = Arc::new(Barrier::new(THREADS));
        let mut workers = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            let refcount = Arc::clone(&refcount);
            let start = Arc::clone(&start);
            workers.push(std::thread::spawn(move || {
                start.wait();
                for _ in 0..ITERATIONS {
                    let mut current = refcount.load_relaxed();
                    loop {
                        let next = retain_next(current, 1, false)
                            .expect("baseline owner keeps the storage live");
                        match refcount.compare_exchange_retain(current, next) {
                            Ok(_) => break,
                            Err(observed) => current = observed,
                        }
                    }
                    let previous = refcount.release_one();
                    assert!(previous > 1);
                }
            }));
        }
        for worker in workers {
            worker.join().expect("refcount worker panicked");
        }
        assert_eq!(refcount.load_acquire(), 1);
    }
}
