//! Kani proofs for the canonical reference-count transition algebra.
//!
//! These harnesses call the same pure functions consumed by `MoltHeader` over
//! the mode-selected `molt-codegen-abi::MoltRefCount` storage authority. They
//! therefore cannot drift into a toy fetch-add model that permits zero
//! resurrection, immortal mutation, wrapping, or an ambiguous finalizer
//! revival baseline.
//!
//! Run with: `cd runtime/molt-obj-model && cargo kani --tests`

#[cfg(kani)]
mod refcount_proofs {
    use molt_codegen_abi::IMMORTAL_REFCOUNT;
    use molt_codegen_abi::{
        RetainError, live_upgrade_next, release_transition, retain_next, revival_window_baseline,
    };

    #[kani::proof]
    fn successful_retain_is_exact_nonwrapping_addition() {
        let current: u32 = kani::any();
        let count: u32 = kani::any();
        let deallocating: bool = kani::any();
        if let Ok(next) = retain_next(current, count, deallocating) {
            assert!(current > 0);
            assert!(current < IMMORTAL_REFCOUNT);
            assert!(!deallocating);
            assert_eq!(next, current + count);
            if count == 0 {
                assert_eq!(next, current);
            } else {
                assert!(next > current);
            }
            assert!(next < IMMORTAL_REFCOUNT);
        }
    }

    #[kani::proof]
    fn retain_rejects_zero_immortal_deallocating_and_overflow() {
        let current: u32 = kani::any();
        let count: u32 = kani::any();
        let deallocating: bool = kani::any();
        let result = retain_next(current, count, deallocating);
        if current == 0 {
            assert_eq!(result, Err(RetainError::Zero));
        } else if current == IMMORTAL_REFCOUNT {
            assert_eq!(result, Err(RetainError::Immortal));
        } else if deallocating {
            assert_eq!(result, Err(RetainError::Deallocating));
        } else if !matches!(
            current.checked_add(count),
            Some(next) if next < IMMORTAL_REFCOUNT
        ) {
            assert_eq!(result, Err(RetainError::Overflow));
        } else {
            assert!(result.is_ok());
        }
    }

    #[kani::proof]
    fn live_upgrade_is_exactly_checked_single_retain() {
        let current: u32 = kani::any();
        let deallocating: bool = kani::any();
        assert_eq!(
            live_upgrade_next(current, deallocating),
            retain_next(current, 1, deallocating)
        );
    }

    #[kani::proof]
    fn release_reaches_zero_exactly_from_one() {
        let previous: u32 = kani::any();
        match release_transition(previous) {
            None => assert!(previous == 0 || previous == IMMORTAL_REFCOUNT),
            Some(transition) => {
                assert!(previous > 0);
                assert!(previous < IMMORTAL_REFCOUNT);
                assert_eq!(transition.previous(), previous);
                assert_eq!(transition.next(), previous - 1);
                assert_eq!(transition.reached_zero(), previous == 1);
            }
        }
    }

    #[kani::proof]
    fn retain_then_release_restores_every_legal_state() {
        let current: u32 = kani::any();
        let count: u32 = kani::any();
        kani::assume(count == 1);
        if let Ok(retained) = retain_next(current, count, false) {
            let released = release_transition(retained).expect("retained state is non-zero");
            assert_eq!(released.next(), current);
        }
    }

    #[kani::proof]
    fn revival_window_accepts_only_the_two_owned_baselines() {
        let previous: u32 = kani::any();
        let has_stable_view_hold: bool = kani::any();
        let result = revival_window_baseline(previous, has_stable_view_hold);
        if has_stable_view_hold {
            assert_eq!(result, if previous == 1 { Some(2) } else { None });
        } else {
            assert_eq!(result, if previous == 0 { Some(1) } else { None });
        }
    }
}
