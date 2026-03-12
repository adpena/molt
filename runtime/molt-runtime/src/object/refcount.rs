//! Refcount storage for `MoltHeader`.
//!
//! On native targets (x86_64, aarch64) refcounts use `AtomicU32` because
//! the GIL does not prevent concurrent access from cpython-abi hooks and
//! signal handlers.
//!
//! On `wasm32` the runtime is guaranteed single-threaded, so we use
//! `Cell<u32>` to avoid atomic-fence overhead on every inc_ref / dec_ref.

use std::sync::atomic::Ordering;

/// Wrapper around the refcount field inside `MoltHeader`.
///
/// On native: backed by `AtomicU32`.
/// On wasm32: backed by `Cell<u32>`.
#[repr(transparent)]
pub struct MoltRefCount {
    #[cfg(not(target_arch = "wasm32"))]
    inner: std::sync::atomic::AtomicU32,
    #[cfg(target_arch = "wasm32")]
    inner: std::cell::Cell<u32>,
}

impl MoltRefCount {
    /// Create a new refcount with initial value `val`.
    #[inline(always)]
    pub const fn new(val: u32) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                inner: std::sync::atomic::AtomicU32::new(val),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self {
                inner: std::cell::Cell::new(val),
            }
        }
    }

    /// Store `val` into the refcount.
    #[inline(always)]
    pub fn store(&self, val: u32, _order: Ordering) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.store(val, _order);
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.set(val);
        }
    }

    /// Load the current refcount.
    #[inline(always)]
    pub fn load(&self, _order: Ordering) -> u32 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.load(_order)
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.inner.get()
        }
    }

    /// Add `val` and return the *previous* value.
    #[inline(always)]
    pub fn fetch_add(&self, val: u32, _order: Ordering) -> u32 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.fetch_add(val, _order)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let prev = self.inner.get();
            self.inner.set(prev.wrapping_add(val));
            prev
        }
    }

    /// Subtract `val` and return the *previous* value.
    #[inline(always)]
    pub fn fetch_sub(&self, val: u32, _order: Ordering) -> u32 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.fetch_sub(val, _order)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let prev = self.inner.get();
            self.inner.set(prev.wrapping_sub(val));
            prev
        }
    }

    /// Acquire fence after a refcount drop reaches zero.
    /// On native targets, issues an acquire fence.
    /// On wasm32, this is a no-op.
    #[inline(always)]
    pub fn acquire_fence() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::sync::atomic::fence(Ordering::Acquire);
        }
    }
}
