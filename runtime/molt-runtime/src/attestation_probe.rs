//! Test-feature-only allocator and hook observer for performance attestations.
//!
//! Production builds use `mimalloc::MiMalloc` directly.  The explicit
//! `l7-attestation-probe` feature wraps the same allocator so untimed observer
//! passes can count allocation traffic without enabling mimalloc's expensive
//! debug statistics or contaminating timed samples with atomics.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static TRACK: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static NUMERIC_HOOK_CALLS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub allocations: u64,
    pub allocated_bytes: u64,
    pub peak_live_bytes: u64,
    pub numeric_hook_calls: u64,
}

pub struct CountingMiMalloc;

impl CountingMiMalloc {
    pub const fn new() -> Self {
        Self
    }
}

#[inline]
fn raise_peak(live: u64) {
    let mut peak = PEAK_LIVE_BYTES.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_LIVE_BYTES.compare_exchange_weak(
            peak,
            live,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => peak = observed,
        }
    }
}

#[inline]
fn record_allocation(size: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    let live = LIVE_BYTES.fetch_add(size as u64, Ordering::Relaxed) + size as u64;
    raise_peak(live);
}

#[inline]
fn record_deallocation(size: usize) {
    let _ = LIVE_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size as u64))
    });
}

unsafe impl GlobalAlloc for CountingMiMalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { mimalloc::MiMalloc.alloc(layout) };
        if !ptr.is_null() && TRACK.load(Ordering::Relaxed) {
            record_allocation(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACK.load(Ordering::Relaxed) {
            record_deallocation(layout.size());
        }
        unsafe { mimalloc::MiMalloc.dealloc(ptr, layout) };
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) };
        if !ptr.is_null() && TRACK.load(Ordering::Relaxed) {
            record_allocation(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { mimalloc::MiMalloc.realloc(ptr, layout, new_size) };
        if !next.is_null() && TRACK.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            if new_size >= layout.size() {
                let delta = new_size - layout.size();
                let live = LIVE_BYTES.fetch_add(delta as u64, Ordering::Relaxed) + delta as u64;
                raise_peak(live);
            } else {
                record_deallocation(layout.size() - new_size);
            }
        }
        next
    }
}

pub fn reset() {
    assert!(
        !TRACK.load(Ordering::Relaxed),
        "reset while probe is active"
    );
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_LIVE_BYTES.store(0, Ordering::Relaxed);
    NUMERIC_HOOK_CALLS.store(0, Ordering::Relaxed);
}

pub fn set_tracking(enabled: bool) {
    TRACK.store(enabled, Ordering::Relaxed);
}

pub fn snapshot() -> Snapshot {
    assert!(
        !TRACK.load(Ordering::Relaxed),
        "snapshot while probe is active"
    );
    Snapshot {
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
        allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        peak_live_bytes: PEAK_LIVE_BYTES.load(Ordering::Relaxed),
        numeric_hook_calls: NUMERIC_HOOK_CALLS.load(Ordering::Relaxed),
    }
}

#[inline]
pub(crate) fn record_numeric_hook() {
    if TRACK.load(Ordering::Relaxed) {
        NUMERIC_HOOK_CALLS.fetch_add(1, Ordering::Relaxed);
    }
}
