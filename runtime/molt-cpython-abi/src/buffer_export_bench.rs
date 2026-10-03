//! Allocation budget for the independent, allocation-free FillInfo transaction.
#![allow(clippy::undocumented_unsafe_blocks)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::c_void;
use std::hint::black_box;

use crate::abi_types::{Py_buffer, PyBUF_FORMAT, PyBUF_STRIDES};
use crate::api::buffer::{PyBuffer_FillInfo, PyBuffer_Release};
// ── Counting allocator ─────────────────────────────────────────────────────
// Wraps the System allocator and tallies allocation count + bytes. Installed as
// THE global allocator for this crate's test binary only (`#[cfg(test)]`).

// PER-THREAD tallies, not process-global. The budget test runs its export
// cycles AND reads the before/after delta on ONE thread, so a thread-local
// counter measures exactly that thread's allocations — immune to allocations
// from OTHER libtest threads running concurrently in this same binary. The
// former process-global `AtomicUsize` counters tallied every thread, so a
// sibling unit test allocating during the measurement window inflated
// allocations/export past the tight ±0.01 budget and flaked under parallel
// `cargo test` (a pure test-measurement isolation defect, not a real
// per-export allocation). `const` initialization keeps the TLS access
// allocation-free, and `try_with` never panics, so incrementing inside the
// global allocator can neither re-enter the allocator nor abort the process.
thread_local! {
    static ALLOC_COUNT: Cell<usize> = const { Cell::new(0) };
    static ALLOC_BYTES: Cell<usize> = const { Cell::new(0) };
}

#[inline]
fn tally(count_delta: usize, bytes_delta: usize) {
    let _ = ALLOC_COUNT.try_with(|c| c.set(c.get() + count_delta));
    let _ = ALLOC_BYTES.try_with(|b| b.set(b.get() + bytes_delta));
}

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tally(1, layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tally(1, layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        tally(1, new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

// NOT installed under Miri: with a custom global allocator Miri interprets
// the REAL Windows `System` alloc code, whose dealloc of an over-aligned
// allocation reads an alignment header stored BEFORE the payload — outside
// the payload-ranged Unique tag a `Box` carries — which Stacked Borrows
// rejects (trips in the libtest harness's own mpmc-channel teardown, 128-byte
// aligned nodes). Under Miri the budget test still RUNS every cycle (full UB
// coverage of export→read→release); the deterministic allocation counts are
// enforced by every native `cargo test` run.
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

pub(crate) fn allocs() -> usize {
    ALLOC_COUNT.with(Cell::get)
}
fn bytes() -> usize {
    ALLOC_BYTES.with(Cell::get)
}

/// One full FillInfo (raw 1-D) export→read→release cycle (public C entrypoint).
#[inline(never)]
fn fillinfo_cycle(buf: *mut c_void, len: isize) {
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        PyBuffer_FillInfo(
            &mut view as *mut Py_buffer,
            std::ptr::null_mut(),
            buf,
            len,
            1,
            PyBUF_FORMAT | PyBUF_STRIDES,
        )
    };
    debug_assert_eq!(rc, 0);
    unsafe {
        black_box(view.len);
        if !view.format.is_null() {
            black_box(*view.format);
        }
        if !view.shape.is_null() {
            black_box(*view.shape);
        }
        PyBuffer_Release(&mut view as *mut Py_buffer);
    }
}

// ── Allocation-budget GATE (runs in normal `cargo test`) ────────────────────
//
// Deterministic + machine-independent. This is the perf-regression interlock —
// if an edit adds a per-export allocation (a side box, a Vec for shape/strides,
// a String for format, a registry node) this fails; further eliminations edit
// the expected counts DOWN (and they can never silently drift up).

/// Iterations for the allocation gate. The per-cycle allocation count is
/// DETERMINISTIC, so a tiny iteration count under Miri (interpreter, ~10^4x
/// slower) preserves both the budget assertion and Miri's UB coverage of the
/// full construct→read→release cycle.
const GATE_ITERS: usize = if cfg!(miri) { 4 } else { 20_000 };
const GATE_WARMUP: usize = if cfg!(miri) { 2 } else { 2048 };

/// Measure steady-state allocations for `iters` cycles of `f` after a warmup.
fn measure_allocs(iters: usize, mut f: impl FnMut()) -> (f64, f64) {
    for _ in 0..GATE_WARMUP {
        f();
    }
    let a0 = allocs();
    let b0 = bytes();
    for _ in 0..iters {
        f();
    }
    let da = allocs() - a0;
    let db = bytes() - b0;
    (da as f64 / iters as f64, db as f64 / iters as f64)
}

#[test]
fn buffer_fillinfo_allocation_budget() {
    let mut bytes = [0u8; 4096];
    let pointer = bytes.as_mut_ptr().cast();
    let (allocations, bytes) = measure_allocs(GATE_ITERS, || fillinfo_cycle(pointer, 4096));
    if !cfg!(miri) {
        assert_eq!((allocations, bytes), (0.0, 0.0));
    }
}
