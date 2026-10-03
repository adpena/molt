//! Publication of one runtime-owned cached handle.
//!
//! Initialization transfers an owned handle to the slot. Reads borrow that
//! owner; Python-callable exports must retain a separate result reference.
//! The caller holds runtime custody that prevents concurrent slot retirement.

use std::sync::atomic::{AtomicU64, Ordering};

pub fn get_or_init(
    slot: &AtomicU64,
    initialize: impl FnOnce() -> u64,
    release: impl FnOnce(u64),
) -> u64 {
    let existing = slot.load(Ordering::Acquire);
    if existing != 0 {
        return existing;
    }
    let candidate = initialize();
    if candidate == 0 {
        return 0;
    }
    match slot.compare_exchange(0, candidate, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => candidate,
        Err(existing) => {
            release(candidate);
            existing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn publication_transfers_one_owner_and_hits_only_borrow() {
        let slot = AtomicU64::new(0);
        assert_eq!(
            get_or_init(&slot, || 17, |_| panic!("published owner released")),
            17
        );
        assert_eq!(
            get_or_init(
                &slot,
                || panic!("cache hit initialized"),
                |_| panic!("borrow released")
            ),
            17
        );
        assert_eq!(slot.swap(0, Ordering::AcqRel), 17);
    }

    #[test]
    fn reentrant_winner_is_preserved_and_losing_owner_is_released_once() {
        let slot = AtomicU64::new(0);
        let released = Cell::new(0);
        let selected = get_or_init(
            &slot,
            || {
                assert_eq!(get_or_init(&slot, || 23, |_| panic!("winner released")), 23);
                17
            },
            |bits| {
                assert_eq!(slot.load(Ordering::Acquire), 23);
                assert_eq!(released.replace(bits), 0);
            },
        );
        assert_eq!(selected, 23);
        assert_eq!(released.get(), 17);
    }

    #[test]
    fn failed_initialization_leaves_the_slot_available() {
        let slot = AtomicU64::new(0);
        assert_eq!(
            get_or_init(&slot, || 0, |_| panic!("no owner to release")),
            0
        );
        assert_eq!(slot.load(Ordering::Acquire), 0);
        assert_eq!(
            get_or_init(&slot, || 31, |_| panic!("published owner released")),
            31
        );
    }
}
