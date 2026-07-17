//! Pure reference-count transition algebra shared by runtime atomics and proofs.
//!
//! Storage and synchronization are target-specific.  The legal state machine
//! is not: retain must never cross zero, immortal, committed-dead, or overflow;
//! release reaches terminal death exactly on `1 -> 0`; and the sole revival
//! window starts from either zero ordinary owners or one stable ABI-view hold.

use molt_codegen_abi::IMMORTAL_REFCOUNT;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainError {
    Zero,
    Immortal,
    Deallocating,
    Overflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefCountRelease {
    previous: u32,
}

impl RefCountRelease {
    #[inline(always)]
    pub const fn previous(self) -> u32 {
        self.previous
    }

    #[inline(always)]
    pub const fn next(self) -> u32 {
        self.previous - 1
    }

    #[inline(always)]
    pub const fn reached_zero(self) -> bool {
        self.previous == 1
    }
}

#[inline(always)]
pub const fn retain_next(current: u32, count: u32, deallocating: bool) -> Result<u32, RetainError> {
    if current == 0 {
        return Err(RetainError::Zero);
    }
    if current == IMMORTAL_REFCOUNT {
        return Err(RetainError::Immortal);
    }
    if deallocating {
        return Err(RetainError::Deallocating);
    }
    // Batch-retain callers naturally produce empty batches. On a valid live
    // mortal object, adding zero is an exact no-op; terminal-state validation
    // above still applies, so zero can never resurrect or bless corrupt state.
    if count == 0 {
        return Ok(current);
    }
    match current.checked_add(count) {
        // The all-ones value is a reserved immortal sentinel, not a mortal
        // count.  Reaching it by arithmetic would create an object that can no
        // longer be retained even though it lacks the immortal lifecycle flag.
        Some(next) if next < IMMORTAL_REFCOUNT => Ok(next),
        Some(_) | None => Err(RetainError::Overflow),
    }
}

#[inline(always)]
pub const fn live_upgrade_next(current: u32, deallocating: bool) -> Result<u32, RetainError> {
    retain_next(current, 1, deallocating)
}

#[inline(always)]
pub const fn release_transition(previous: u32) -> Option<RefCountRelease> {
    if previous == 0 || previous == IMMORTAL_REFCOUNT {
        None
    } else {
        Some(RefCountRelease { previous })
    }
}

/// Return the live baseline after adding the finalizer/weakref internal pin.
#[inline(always)]
pub const fn revival_window_baseline(previous: u32, has_stable_view_hold: bool) -> Option<u32> {
    let expected = has_stable_view_hold as u32;
    if previous == expected {
        Some(expected + 1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retain_rejects_every_terminal_or_wrapping_transition() {
        assert_eq!(retain_next(0, 1, false), Err(RetainError::Zero));
        assert_eq!(
            retain_next(IMMORTAL_REFCOUNT, 1, false),
            Err(RetainError::Immortal)
        );
        assert_eq!(retain_next(1, 1, true), Err(RetainError::Deallocating));
        assert_eq!(retain_next(7, 0, false), Ok(7));
        assert_eq!(
            retain_next(IMMORTAL_REFCOUNT - 1, 2, false),
            Err(RetainError::Overflow)
        );
        assert_eq!(
            retain_next(IMMORTAL_REFCOUNT - 1, 1, false),
            Err(RetainError::Overflow)
        );
    }

    #[test]
    fn terminal_release_and_revival_states_are_exact() {
        let terminal = release_transition(1).expect("one owner has a legal release");
        assert!(terminal.reached_zero());
        assert_eq!(terminal.next(), 0);
        assert!(release_transition(0).is_none());
        assert!(release_transition(IMMORTAL_REFCOUNT).is_none());
        assert_eq!(revival_window_baseline(0, false), Some(1));
        assert_eq!(revival_window_baseline(1, true), Some(2));
        assert_eq!(revival_window_baseline(1, false), None);
        assert_eq!(revival_window_baseline(0, true), None);
    }
}
