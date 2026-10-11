//! Test-feature-only allocator and hook observer for performance attestations.
//!
//! Production builds use `mimalloc::MiMalloc` directly.  The explicit
//! `l7-attestation-probe` feature wraps the same allocator so untimed observer
//! passes can count allocation traffic without enabling mimalloc's expensive
//! debug statistics or contaminating timed samples with atomics.

use molt_cpython_abi::l7_attestation::c_allocations;
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static TRACK: AtomicBool = AtomicBool::new(false);
static DENY_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static DENIED_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static NUMERIC_HOOK_CALLS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub allocations: u64,
    /// Attempts at the ABI platform C malloc/calloc/realloc seam.
    pub c_allocation_attempts: u64,
    pub allocated_bytes: u64,
    pub peak_live_bytes: u64,
    pub numeric_hook_calls: u64,
}

// Fixed recording storage and its lock are initialized before tracing.
// Recording callbacks only mutate that storage; they never allocate or format.
const REQUEST_LIMIT: usize = 128;
static TRACE_REQUESTS: AtomicBool = AtomicBool::new(false);
static REQUESTS: std::sync::Mutex<RequestState> = std::sync::Mutex::new(RequestState::EMPTY);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationOperation {
    Allocate,
    Zeroed,
    Reallocate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationOutcome {
    Pending,
    Granted,
    Denied,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocationRequest {
    pub ordinal: usize,
    pub operation: AllocationOperation,
    pub size: usize,
    pub alignment: usize,
    pub outcome: AllocationOutcome,
}

impl AllocationRequest {
    const EMPTY: Self = Self {
        ordinal: 0,
        operation: AllocationOperation::Allocate,
        size: 0,
        alignment: 0,
        outcome: AllocationOutcome::Pending,
    };

    fn matches(&self, other: &Self) -> bool {
        self.ordinal == other.ordinal
            && self.operation == other.operation
            && self.size == other.size
            && self.alignment == other.alignment
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AllocationTrace {
    requests: [AllocationRequest; REQUEST_LIMIT],
    len: usize,
    pub overflowed: bool,
    pub prefix_mismatch: bool,
}

impl AllocationTrace {
    const EMPTY: Self = Self {
        requests: [AllocationRequest::EMPTY; REQUEST_LIMIT],
        len: 0,
        overflowed: false,
        prefix_mismatch: false,
    };

    pub fn requests(&self) -> &[AllocationRequest] {
        &self.requests[..self.len]
    }
}

struct RequestState {
    actual: AllocationTrace,
    expected: [AllocationRequest; REQUEST_LIMIT],
    expected_len: usize,
}

impl RequestState {
    const EMPTY: Self = Self {
        actual: AllocationTrace::EMPTY,
        expected: [AllocationRequest::EMPTY; REQUEST_LIMIT],
        expected_len: 0,
    };
}

/// Records real Rust allocator requests; optionally refuses only the final
/// request of an exactly matching previously observed successful prefix. A
/// changed prefix disables refusal, and makes the proof visibly fail instead
/// of denying an unrelated infallible allocation. Own the process exclusively.
pub struct AllocationTraceGuard;

pub fn trace_allocations(deny_prefix: Option<&[AllocationRequest]>) -> AllocationTraceGuard {
    assert!(
        !TRACK.load(Ordering::Relaxed),
        "allocation probe already active"
    );
    assert!(
        !TRACE_REQUESTS.load(Ordering::Relaxed),
        "request trace already active"
    );
    assert!(
        !DENY_ALLOCATIONS.load(Ordering::Relaxed),
        "allocation denial already active"
    );
    let prefix = deny_prefix.unwrap_or(&[]);
    assert!(
        prefix.len() <= REQUEST_LIMIT,
        "request prefix exceeds bounded storage"
    );
    assert!(
        prefix
            .iter()
            .all(|request| request.outcome == AllocationOutcome::Granted)
    );
    reset();
    DENIED_ALLOCATIONS.store(0, Ordering::Relaxed);
    {
        let mut state = REQUESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = RequestState::EMPTY;
        state.expected[..prefix.len()].copy_from_slice(prefix);
        state.expected_len = prefix.len();
    }
    TRACE_REQUESTS.store(true, Ordering::Relaxed);
    set_tracking(true);
    AllocationTraceGuard
}

impl AllocationTraceGuard {
    pub fn finish(self) -> AllocationTrace {
        drop(self);
        REQUESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .actual
    }
}

impl Drop for AllocationTraceGuard {
    fn drop(&mut self) {
        set_tracking(false);
        TRACE_REQUESTS.store(false, Ordering::Relaxed);
    }
}

fn request_allocation(
    operation: AllocationOperation,
    size: usize,
    alignment: usize,
) -> (Option<usize>, bool) {
    let mut denied = TRACK.load(Ordering::Relaxed) && DENY_ALLOCATIONS.load(Ordering::Relaxed);
    let mut recorded = None;
    if TRACE_REQUESTS.load(Ordering::Relaxed) {
        let mut state = REQUESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = state.actual.len;
        if index == REQUEST_LIMIT {
            state.actual.overflowed = true;
        } else {
            let mut request = AllocationRequest {
                ordinal: index + 1,
                operation,
                size,
                alignment,
                outcome: AllocationOutcome::Pending,
            };
            if index < state.expected_len && !state.actual.prefix_mismatch {
                if !request.matches(&state.expected[index]) {
                    state.actual.prefix_mismatch = true;
                } else if index + 1 == state.expected_len {
                    denied = true;
                }
            }
            if denied {
                request.outcome = AllocationOutcome::Denied;
            }
            state.actual.requests[index] = request;
            state.actual.len += 1;
            recorded = Some(index);
        }
    }
    if denied {
        DENIED_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
    (recorded, denied)
}

fn complete_request(index: Option<usize>, succeeded: bool) {
    if let Some(index) = index {
        let mut state = REQUESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.actual.requests[index].outcome = if succeeded {
            AllocationOutcome::Granted
        } else {
            AllocationOutcome::Failed
        };
    }
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
    let _ = LIVE_BYTES.try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size as u64))
    });
}

unsafe impl GlobalAlloc for CountingMiMalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let track = TRACK.load(Ordering::Relaxed);
        let (request, denied) = if track {
            request_allocation(AllocationOperation::Allocate, layout.size(), layout.align())
        } else {
            (None, false)
        };
        if denied {
            return std::ptr::null_mut();
        }
        let ptr = unsafe { mimalloc::MiMalloc.alloc(layout) };
        complete_request(request, !ptr.is_null());
        if !ptr.is_null() && track {
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
        let track = TRACK.load(Ordering::Relaxed);
        let (request, denied) = if track {
            request_allocation(AllocationOperation::Zeroed, layout.size(), layout.align())
        } else {
            (None, false)
        };
        if denied {
            return std::ptr::null_mut();
        }
        let ptr = unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) };
        complete_request(request, !ptr.is_null());
        if !ptr.is_null() && track {
            record_allocation(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let track = TRACK.load(Ordering::Relaxed);
        let (request, denied) = if track {
            request_allocation(AllocationOperation::Reallocate, new_size, layout.align())
        } else {
            (None, false)
        };
        if denied {
            return std::ptr::null_mut();
        }
        let next = unsafe { mimalloc::MiMalloc.realloc(ptr, layout, new_size) };
        complete_request(request, !next.is_null());
        if !next.is_null() && track {
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
    c_allocations::reset();
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_LIVE_BYTES.store(0, Ordering::Relaxed);
    NUMERIC_HOOK_CALLS.store(0, Ordering::Relaxed);
}

pub fn set_tracking(enabled: bool) {
    c_allocations::set_tracking(enabled);
    TRACK.store(enabled, Ordering::Relaxed);
}

pub fn snapshot() -> Snapshot {
    assert!(
        !TRACK.load(Ordering::Relaxed),
        "snapshot while probe is active"
    );
    Snapshot {
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
        c_allocation_attempts: c_allocations::snapshot().attempts,
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

/// Scoped allocation denial for prepared, untimed native controls. Uses the
/// existing feature-only Rust allocator and ABI platform C allocator observer;
/// normal guests never compile this authority.
/// As with tracking, the harness must own the process and run a single test.
pub struct AllocationDenial;

pub fn deny_allocations() -> AllocationDenial {
    assert!(
        !TRACK.load(Ordering::Relaxed),
        "allocation probe already active"
    );
    assert!(
        !DENY_ALLOCATIONS.load(Ordering::Relaxed),
        "allocation denial already active"
    );
    c_allocations::reset();
    c_allocations::set_denial(true);
    DENIED_ALLOCATIONS.store(0, Ordering::Relaxed);
    DENY_ALLOCATIONS.store(true, Ordering::Relaxed);
    set_tracking(true);
    AllocationDenial
}

impl Drop for AllocationDenial {
    fn drop(&mut self) {
        set_tracking(false);
        DENY_ALLOCATIONS.store(false, Ordering::Relaxed);
        c_allocations::set_denial(false);
    }
}

pub fn denied_allocations() -> u64 {
    assert!(
        !TRACK.load(Ordering::Relaxed),
        "denial result while probe is active"
    );
    DENIED_ALLOCATIONS.load(Ordering::Relaxed)
}

/// Denied attempts at the ABI platform C allocator; extension-owned libc
/// allocation and the variadic shim's private scratch allocation are excluded.
pub fn denied_c_allocations() -> u64 {
    c_allocations::snapshot().denied
}
