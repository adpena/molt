//! Kani bounded-verification harnesses for refcount invariants.
//!
//! Since `MoltRefCount` lives in `molt-runtime` (a large crate with many
//! dependencies), we verify the core refcount logic here using a minimal
//! standalone model that mirrors the `AtomicU32`-backed implementation.
//! This avoids pulling in the entire runtime dependency graph.
//!
//! Run with: `cd runtime/molt-obj-model && cargo kani --tests`

#[cfg(kani)]
mod refcount_proofs {
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Minimal model of MoltRefCount's native (non-wasm) implementation.
    struct RefCount {
        inner: AtomicU32,
    }

    impl RefCount {
        fn new(val: u32) -> Self {
            Self {
                inner: AtomicU32::new(val),
            }
        }

        fn load(&self) -> u32 {
            self.inner.load(Ordering::Relaxed)
        }

        fn fetch_add(&self, val: u32) -> u32 {
            self.inner.fetch_add(val, Ordering::Relaxed)
        }

        fn fetch_sub(&self, val: u32) -> u32 {
            self.inner.fetch_sub(val, Ordering::Relaxed)
        }
    }

    // ---------------------------------------------------------------
    // Increment then decrement returns to original
    // ---------------------------------------------------------------

    /// For any initial refcount, incrementing by 1 then decrementing by 1
    /// restores the original value.
    #[kani::proof]
    #[kani::unwind(1)]
    fn inc_dec_returns_to_original() {
        let init: u32 = kani::any();
        // Avoid overflow: initial value must leave room for +1.
        kani::assume(init < u32::MAX);

        let rc = RefCount::new(init);
        rc.fetch_add(1);
        assert_eq!(rc.load(), init + 1);

        rc.fetch_sub(1);
        assert_eq!(rc.load(), init);
    }

    /// Incrementing by n then decrementing by n restores the original value.
    #[kani::proof]
    #[kani::unwind(1)]
    fn inc_n_dec_n_identity() {
        let init: u32 = kani::any();
        let n: u32 = kani::any();
        // Avoid overflow.
        kani::assume((init as u64) + (n as u64) <= u32::MAX as u64);

        let rc = RefCount::new(init);
        rc.fetch_add(n);
        assert_eq!(rc.load(), init.wrapping_add(n));

        rc.fetch_sub(n);
        assert_eq!(rc.load(), init);
    }

    // ---------------------------------------------------------------
    // Refcount starts at expected value
    // ---------------------------------------------------------------

    /// A freshly created refcount has the initial value.
    #[kani::proof]
    #[kani::unwind(1)]
    fn new_has_initial_value() {
        let init: u32 = kani::any();
        let rc = RefCount::new(init);
        assert_eq!(rc.load(), init);
    }

    // ---------------------------------------------------------------
    // fetch_add / fetch_sub return the *previous* value
    // ---------------------------------------------------------------

    /// fetch_add returns the value before the add.
    #[kani::proof]
    #[kani::unwind(1)]
    fn fetch_add_returns_previous() {
        let init: u32 = kani::any();
        let n: u32 = kani::any();
        kani::assume((init as u64) + (n as u64) <= u32::MAX as u64);

        let rc = RefCount::new(init);
        let prev = rc.fetch_add(n);
        assert_eq!(prev, init);
        assert_eq!(rc.load(), init + n);
    }

    /// fetch_sub returns the value before the sub.
    #[kani::proof]
    #[kani::unwind(1)]
    fn fetch_sub_returns_previous() {
        let init: u32 = kani::any();
        let n: u32 = kani::any();
        kani::assume(init >= n);

        let rc = RefCount::new(init);
        let prev = rc.fetch_sub(n);
        assert_eq!(prev, init);
        assert_eq!(rc.load(), init - n);
    }

    // ---------------------------------------------------------------
    // Monotonicity: inc always increases, dec always decreases
    // ---------------------------------------------------------------

    /// After fetch_add(1), the refcount is strictly greater than before.
    #[kani::proof]
    #[kani::unwind(1)]
    fn inc_is_monotonically_increasing() {
        let init: u32 = kani::any();
        kani::assume(init < u32::MAX);

        let rc = RefCount::new(init);
        rc.fetch_add(1);
        assert!(rc.load() > init);
    }

    /// After fetch_sub(1) on a non-zero refcount, the value is strictly less.
    #[kani::proof]
    #[kani::unwind(1)]
    fn dec_is_monotonically_decreasing() {
        let init: u32 = kani::any();
        kani::assume(init > 0);

        let rc = RefCount::new(init);
        rc.fetch_sub(1);
        assert!(rc.load() < init);
    }
}
