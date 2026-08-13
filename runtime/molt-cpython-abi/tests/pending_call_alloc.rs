mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::c_void;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use molt_cpython_abi::api::pending_calls::{
    PENDING_CALL_CAPACITY, PendingCallFn, Py_AddPendingCall, has_pending_calls,
    make_pending_calls_at_runtime_safepoint, register_main_thread,
};

struct CountingAllocator;

thread_local! {
    static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
    static ALLOCATION_BYTES: Cell<usize> = const { Cell::new(0) };
}

fn observe_allocation(bytes: usize) {
    let tracking = TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or(false);
    if tracking {
        let _ = ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
        let _ = ALLOCATION_BYTES.try_with(|total| total.set(total.get() + bytes));
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe_allocation(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        observe_allocation(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        observe_allocation(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

static CALLBACKS_RUN: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn never_run(_arg: *mut c_void) -> c_int {
    CALLBACKS_RUN.fetch_add(1, Ordering::Relaxed);
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn pending_call_calibrated_empty_authority() -> c_int {
    0
}

#[inline(never)]
fn calibrated_call_baseline(input: c_int) -> c_int {
    std::hint::black_box(input);
    pending_call_calibrated_empty_authority()
}

#[inline(never)]
fn measured_production_empty_poll(input: c_int) -> c_int {
    std::hint::black_box(input);
    make_pending_calls_at_runtime_safepoint()
}

#[test]
fn empty_poll_enqueue_and_full_paths_are_allocation_free() {
    support::prepare_abi_test_thread(support::stub_runtime_hooks());
    const ROUNDS: usize = 31;
    const ITERATIONS: usize = 500_000;

    assert_eq!(
        unsafe { Py_AddPendingCall(Some(never_run), std::ptr::null_mut()) },
        -1,
        "pre-initialization pending-call state is closed"
    );
    assert!(!has_pending_calls());
    assert_eq!(CALLBACKS_RUN.load(Ordering::Relaxed), 0);
    assert!(register_main_thread(std::thread::current().id()));

    let measure = |call: fn(c_int) -> c_int| {
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            let input = std::hint::black_box(0);
            std::hint::black_box(call(input));
        }
        start.elapsed().as_nanos() as f64 / ITERATIONS as f64
    };
    for _ in 0..20_000 {
        std::hint::black_box(calibrated_call_baseline(std::hint::black_box(0)));
        std::hint::black_box(measured_production_empty_poll(std::hint::black_box(0)));
    }
    let mut baseline_ns = Vec::with_capacity(ROUNDS);
    let mut empty_poll_ns = Vec::with_capacity(ROUNDS);
    let mut incremental_ns = Vec::with_capacity(ROUNDS);
    for round in 0..ROUNDS {
        let (baseline, poll) = if round % 2 == 0 {
            (
                measure(calibrated_call_baseline),
                measure(measured_production_empty_poll),
            )
        } else {
            let poll = measure(measured_production_empty_poll);
            let baseline = measure(calibrated_call_baseline);
            (baseline, poll)
        };
        assert_eq!(make_pending_calls_at_runtime_safepoint(), 0);
        baseline_ns.push(baseline);
        empty_poll_ns.push(poll);
        incremental_ns.push(poll - baseline);
    }
    baseline_ns.sort_by(f64::total_cmp);
    empty_poll_ns.sort_by(f64::total_cmp);
    incremental_ns.sort_by(f64::total_cmp);
    let quantiles = |samples: &[f64]| {
        (
            samples[ROUNDS / 4],
            samples[ROUNDS / 2],
            samples[(ROUNDS * 9) / 10],
        )
    };
    let (baseline_p25, baseline_p50, baseline_p90) = quantiles(&baseline_ns);
    let (poll_p25, poll_p50, poll_p90) = quantiles(&empty_poll_ns);
    let (incremental_p25, incremental_p50, incremental_p90) = quantiles(&incremental_ns);
    eprintln!(
        "empty pending-call readiness poll profile={} rounds={ROUNDS} iterations={ITERATIONS} baseline_ns[p25={baseline_p25:.3},p50={baseline_p50:.3},p90={baseline_p90:.3}] poll_ns[p25={poll_p25:.3},p50={poll_p50:.3},p90={poll_p90:.3}] signed_delta_ns[p25={incremental_p25:.3},p50={incremental_p50:.3},p90={incremental_p90:.3}]",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let (absolute_limit_ns, incremental_limit_ns) = if cfg!(debug_assertions) {
        (250.0, 100.0)
    } else {
        (75.0, 50.0)
    };
    assert!(
        poll_p90 < absolute_limit_ns,
        "empty pending-call readiness p90 {poll_p90:.3} ns exceeded {absolute_limit_ns:.3} ns"
    );
    assert!(
        incremental_p90 < incremental_limit_ns,
        "empty pending-call signed-delta p90 {incremental_p90:.3} ns exceeded {incremental_limit_ns:.3} ns"
    );

    ALLOCATION_COUNT.with(|count| count.set(0));
    ALLOCATION_BYTES.with(|bytes| bytes.set(0));
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(true));
    for _ in 0..100_000 {
        assert_eq!(make_pending_calls_at_runtime_safepoint(), 0);
    }
    for _ in 0..PENDING_CALL_CAPACITY {
        assert_eq!(
            unsafe { Py_AddPendingCall(Some(never_run as PendingCallFn), std::ptr::null_mut(),) },
            0
        );
    }
    assert_eq!(
        unsafe { Py_AddPendingCall(Some(never_run as PendingCallFn), std::ptr::null_mut(),) },
        -1
    );
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(false));
    eprintln!(
        "empty polls={} enqueues={} full_rejections=1 allocations={} allocated_bytes={}",
        100_000,
        PENDING_CALL_CAPACITY,
        ALLOCATION_COUNT.with(Cell::get),
        ALLOCATION_BYTES.with(Cell::get)
    );
    assert_eq!(ALLOCATION_COUNT.with(Cell::get), 0);
    assert_eq!(ALLOCATION_BYTES.with(Cell::get), 0);
    assert_eq!(CALLBACKS_RUN.load(Ordering::Relaxed), 0);
}
