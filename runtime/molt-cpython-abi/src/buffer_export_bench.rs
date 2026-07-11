//! Profiling + allocation-budget gate for the Py_buffer export→release hot path.
//!
//! This module is `#[cfg(test)]`-only: it is NEVER compiled into the shipped
//! `cdylib`/`rlib`, so the counting global allocator and the timing loops carry
//! ZERO production cost. It exists to make the buffer-export cost
//! *machine-checkable* per M10: a deterministic allocation-budget assertion that
//! fails closed if any change adds a per-export heap allocation, plus an
//! (ignored) wall-clock profiler that emits the ns/export attestation numbers.
//!
//! ## The hot path under test (post BUFFER-DISTILL-55)
//!
//! The former model boxed a 1112 B `BufferInternal` (8 B release_kind + a
//! wholesale `MoltBufferView` copy) per export and tracked the pointer in a
//! global `Mutex<HashSet>` registry consulted again at release. Both are GONE:
//!
//!   * `PyBuffer_FillInfo` is CPython-exact and **allocation-free** (static
//!     `"B"` format, self-referential `shape = &view.len` /
//!     `strides = &view.itemsize`, `internal = NULL`).
//!   * `PyMemoryView_FromBuffer` / `PyMemoryView_FromMemory` make exactly
//!     **one** allocation — the `PyMemoryViewObject` itself, whose embedded
//!     `ob_shape`/`ob_strides`/`ob_format` storage carries the descriptor
//!     (CPython's `ob_array` model). Nothing to free at `PyBuffer_Release`;
//!     the descriptor dies with the object.
//!   * `PyObject_GetBuffer` (molt-native exporter; exercised in
//!     `tests/test_modules.rs`, needs runtime hooks) makes one right-sized
//!     `ExportInternal` allocation: 32 B header + 16 B/dim — gated by
//!     `api::buffer::export_internal_tests::export_internal_is_right_sized`.
//!   * `PyBuffer_Release` dispatches on the exporter object (CPython's model):
//!     no registry lookup, no global mutex.

#![allow(clippy::undocumented_unsafe_blocks)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::hint::black_box;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::abi_types::{Py_buffer, PyBUF_FORMAT, PyBUF_READ, PyBUF_STRIDES, PyMemoryViewObject};
use crate::api::buffer::{PyBuffer_FillInfo, PyBuffer_Release};
use crate::api::memory::{
    PyMemoryView_FromBuffer, PyMemoryView_FromMemory, PyMemoryView_GET_BUFFER,
    molt_memoryview_dealloc,
};

// ── Counting allocator ─────────────────────────────────────────────────────
// Wraps the System allocator and tallies allocation count + bytes. Installed as
// THE global allocator for this crate's test binary only (`#[cfg(test)]`).

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size, Ordering::Relaxed);
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

fn allocs() -> usize {
    ALLOC_COUNT.load(Ordering::Relaxed)
}
fn bytes() -> usize {
    ALLOC_BYTES.load(Ordering::Relaxed)
}

// ── Synthetic Py_buffer construction (no runtime hooks required) ────────────
//
// `obj` is left NULL so no bridge lock, no refcount, and no runtime hook is
// touched anywhere in the cycle: `descriptor_from_pybuffer` is a pure field
// read, release of an obj-NULL view is a struct reset, and the memoryview is
// torn down with `molt_memoryview_dealloc` directly (what Py_DECREF dispatches
// to). The export / release cycle is therefore measured self-contained.

/// Backing storage kept alive for the duration of one export variant.
struct Exporter {
    _data: Vec<u8>,
    format: Vec<c_char>,
    shape: Vec<isize>,
    strides: Vec<isize>,
    info: Py_buffer,
}

fn make_exporter(
    format_code: &str,
    itemsize: isize,
    shape: &[isize],
    strides: &[isize],
) -> Box<Exporter> {
    assert_eq!(shape.len(), strides.len());
    let ndim = shape.len();
    let nelem: isize = shape.iter().copied().product::<isize>().max(0);
    let len = nelem * itemsize;
    let data = vec![0u8; len.max(1) as usize];
    let mut format: Vec<c_char> = format_code.bytes().map(|b| b as c_char).collect();
    format.push(0); // NUL terminator
    let mut ex = Box::new(Exporter {
        _data: data,
        format,
        shape: shape.to_vec(),
        strides: strides.to_vec(),
        info: unsafe { std::mem::zeroed() },
    });
    ex.info = Py_buffer {
        buf: ex._data.as_ptr() as *mut c_void,
        obj: std::ptr::null_mut(),
        len,
        itemsize,
        readonly: 1,
        ndim: ndim as i32,
        format: ex.format.as_mut_ptr(),
        shape: ex.shape.as_mut_ptr(),
        strides: ex.strides.as_mut_ptr(),
        suboffsets: std::ptr::null_mut(),
        internal: std::ptr::null_mut(),
    };
    ex
}

/// One full `PyMemoryView_FromBuffer` construct→(C reads)→destroy cycle — the
/// real public memoryview-copy entrypoint. All reads are `black_box`ed so the
/// optimizer cannot elide the C-visible descriptor accesses.
#[inline(never)]
fn memoryview_from_buffer_cycle(info: *mut Py_buffer) {
    let mv = unsafe { PyMemoryView_FromBuffer(info) };
    debug_assert!(!mv.is_null(), "export must succeed");
    unsafe {
        let view = PyMemoryView_GET_BUFFER(mv);
        black_box((*view).len);
        black_box((*view).itemsize);
        if !(*view).format.is_null() {
            let mut p = (*view).format;
            while *p != 0 {
                black_box(*p);
                p = p.add(1);
            }
        }
        if !(*view).shape.is_null() && !(*view).strides.is_null() {
            for i in 0..(*view).ndim as isize {
                black_box(*(*view).shape.offset(i));
                black_box(*(*view).strides.offset(i));
            }
        }
        molt_memoryview_dealloc(mv);
    }
}

/// One full `PyMemoryView_FromMemory` construct→read→destroy cycle.
#[inline(never)]
fn memoryview_from_memory_cycle(buf: *mut c_char, len: isize) {
    let mv = unsafe { PyMemoryView_FromMemory(buf, len, PyBUF_READ) };
    debug_assert!(!mv.is_null());
    unsafe {
        let view = PyMemoryView_GET_BUFFER(mv);
        black_box((*view).len);
        if !(*view).shape.is_null() {
            black_box(*(*view).shape);
        }
        molt_memoryview_dealloc(mv);
    }
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

/// A memoryview construction is exactly ONE allocation: the
/// `PyMemoryViewObject` itself (descriptor storage embedded — CPython's
/// `ob_array` model). The former extra 1112 B `BufferInternal` side box and
/// the registry `HashSet` node are gone.
const EXPECTED_ALLOCS_PER_MEMORYVIEW: f64 = 1.0;
/// `PyBuffer_FillInfo` is CPython-exact: allocation-free.
const EXPECTED_ALLOCS_PER_FILLINFO: f64 = 0.0;

#[test]
fn buffer_export_allocation_budget() {
    let mv_bytes = std::mem::size_of::<PyMemoryViewObject>() as f64;

    // Run every cycle first (under Miri this is the UB coverage of the full
    // construct→read→release paths), collecting the steady-state counts.
    //
    // 1-D uint8 contiguous — the numpy `np.uint8` / bytes-like common case.
    let mut ex = make_exporter("B", 1, &[4096], &[1]);
    let info = &mut ex.info as *mut Py_buffer;
    let (ap, bp) = measure_allocs(GATE_ITERS, || memoryview_from_buffer_cycle(info));
    // 3-D float64 strided — worst-case descriptor (still only the object;
    // shape/strides live in the embedded ob_array storage).
    let mut ex3 = make_exporter("d", 8, &[16, 16, 16], &[2048, 128, 8]);
    let info3 = &mut ex3.info as *mut Py_buffer;
    let (ap3, bp3) = measure_allocs(GATE_ITERS, || memoryview_from_buffer_cycle(info3));
    // FromMemory raw 1-D path.
    let mut mbuf = vec![0u8; 4096];
    let mptr = mbuf.as_mut_ptr() as *mut c_char;
    let (apm, bpm) = measure_allocs(GATE_ITERS, || memoryview_from_memory_cycle(mptr, 4096));
    // FillInfo raw 1-D path (public C entrypoint).
    let mut fbuf = vec![0u8; 4096];
    let fptr = fbuf.as_mut_ptr() as *mut c_void;
    let (apf, bpf) = measure_allocs(GATE_ITERS, || fillinfo_cycle(fptr, 4096));

    if cfg!(miri) {
        // The counting allocator is not installed under Miri (see `GLOBAL`),
        // so every measurement above reads 0. The cycles themselves DID run
        // under the interpreter; the exact, deterministic count assertions
        // below are enforced by every native `cargo test` run.
        return;
    }

    assert!(
        (ap - EXPECTED_ALLOCS_PER_MEMORYVIEW).abs() < 0.01,
        "1D uint8 FromBuffer allocations/export = {ap} (expected {EXPECTED_ALLOCS_PER_MEMORYVIEW}); \
         a per-export allocation was added or removed — update the budget deliberately",
    );
    assert!(
        (bp - mv_bytes).abs() < 1.0,
        "1D uint8 FromBuffer bytes/export = {bp} (expected {mv_bytes} = one PyMemoryViewObject)",
    );
    assert!(
        (ap3 - EXPECTED_ALLOCS_PER_MEMORYVIEW).abs() < 0.01,
        "3D f64 FromBuffer allocations/export = {ap3} (expected {EXPECTED_ALLOCS_PER_MEMORYVIEW})",
    );
    assert!(
        (bp3 - mv_bytes).abs() < 1.0,
        "3D f64 FromBuffer bytes/export = {bp3} (expected {mv_bytes}) — \
         descriptor storage is embedded in the object, O(1) in ndim",
    );
    assert!(
        (apm - EXPECTED_ALLOCS_PER_MEMORYVIEW).abs() < 0.01,
        "FromMemory allocations/export = {apm} (expected {EXPECTED_ALLOCS_PER_MEMORYVIEW})",
    );
    assert!(
        (bpm - mv_bytes).abs() < 1.0,
        "FromMemory bytes/export = {bpm} (expected {mv_bytes})",
    );
    assert!(
        (apf - EXPECTED_ALLOCS_PER_FILLINFO).abs() < 0.01,
        "FillInfo allocations/export = {apf} (expected {EXPECTED_ALLOCS_PER_FILLINFO}) — \
         FillInfo is CPython-exact and must stay allocation-free",
    );
    assert!(bpf.abs() < 1.0, "FillInfo bytes/export = {bpf} (expected 0)");
}

// ── Wall-clock profiler (ignored; run manually for the attestation) ─────────
//
//   cargo test -p molt-lang-cpython-abi --release buffer_export_timing_profile \
//       -- --ignored --nocapture
//
// Emits ns/export + allocs/export for every dtype/shape variant plus control
// baselines. Controls A/B are kept for continuity with the pre-distill profile
// (they were the box + registry costs this change deleted).

fn time_ns(iters: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..4096 {
        f();
    }
    let t0 = std::time::Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed().as_nanos() as f64 / iters as f64
}

/// Control A (historical): raw `Box::new` of a 1112 B payload + read + drop —
/// the malloc+free+memcpy floor of the DELETED `BufferInternal` box.
#[inline(never)]
fn control_box_only() {
    let b: Box<[u8; 1112]> = Box::new([0u8; 1112]);
    black_box(b[0]);
    black_box(b[1111]);
    drop(black_box(b));
}

/// Control B (historical): a `Mutex<HashSet<usize>>` insert+remove per iter,
/// mirroring the DELETED `BUFFER_INTERNAL_REGISTRY` cost per export+release.
#[inline(never)]
fn control_registry_only() {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static REG: std::sync::LazyLock<Mutex<HashSet<usize>>> =
        std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));
    let key = black_box(0x1234_5678_usize);
    REG.lock().unwrap().insert(key); // export side
    let hit = REG.lock().unwrap().remove(&key); // release side
    black_box(hit);
}

/// Control C: `PyMemoryViewObject`-sized Box new+read+drop — the floor of the
/// ONE remaining allocation in a memoryview construction.
#[inline(never)]
fn control_mv_box_only() {
    let b: Box<PyMemoryViewObject> = Box::new(unsafe { std::mem::zeroed() });
    black_box(b.view.len);
    drop(black_box(b));
}

#[test]
#[ignore = "wall-clock profiler; run with --ignored --nocapture --release"]
fn buffer_export_timing_profile() {
    const N: usize = 200_000;

    println!("\n=== Py_buffer export→release profile (N={N}/variant) ===");
    println!(
        "post-distill: FillInfo = 0 allocs; memoryview = 1 alloc of {} B (object, storage embedded); \
         GetBuffer internal = 32 B + 16 B/dim",
        std::mem::size_of::<PyMemoryViewObject>()
    );

    // Control baselines.
    let ns_box = time_ns(N, control_box_only);
    println!(
        "control A: Box<[u8;1112]> new+read+drop       {ns_box:8.2} ns  (DELETED box floor, historical)"
    );
    let ns_reg = time_ns(N, control_registry_only);
    println!(
        "control B: Mutex<HashSet> insert+remove       {ns_reg:8.2} ns  (DELETED registry cost, historical)"
    );
    let ns_mv = time_ns(N, control_mv_box_only);
    println!(
        "control C: Box<PyMemoryViewObject> new+drop   {ns_mv:8.2} ns  (the one remaining allocation)"
    );

    // dtype sweep — 1-D contiguous, the numpy scalar-dtype common case.
    let dtypes: &[(&str, isize)] = &[
        ("B", 1),
        ("b", 1),
        ("H", 2),
        ("h", 2),
        ("I", 4),
        ("i", 4),
        ("L", 8),
        ("l", 8),
        ("Q", 8),
        ("q", 8),
        ("f", 4),
        ("d", 8),
    ];
    println!("\n-- PyMemoryView_FromBuffer, 1-D contiguous, shape=[4096], per numpy dtype --");
    for &(fmt, isz) in dtypes {
        let mut ex = make_exporter(fmt, isz, &[4096], &[isz]);
        let info = &mut ex.info as *mut Py_buffer;
        let ns = time_ns(N, || memoryview_from_buffer_cycle(info));
        let (ap, bp) = measure_allocs(GATE_ITERS, || memoryview_from_buffer_cycle(info));
        println!(
            "  dtype '{fmt}' itemsize={isz:<2}  {ns:8.2} ns/export   {ap:.3} allocs/export  {bp:.0} B/export"
        );
    }

    // ndim sweep — shows the O(ndim) descriptor copy loop is a small term.
    println!("\n-- PyMemoryView_FromBuffer, float64, ndim sweep (strided) --");
    let shapes: &[(&str, Vec<isize>, Vec<isize>)] = &[
        ("1-D", vec![4096], vec![8]),
        ("2-D", vec![64, 64], vec![512, 8]),
        ("3-D", vec![16, 16, 16], vec![2048, 128, 8]),
        ("4-D", vec![8, 8, 8, 8], vec![4096, 512, 64, 8]),
    ];
    for (label, shape, strides) in shapes {
        let mut ex = make_exporter("d", 8, shape, strides);
        let info = &mut ex.info as *mut Py_buffer;
        let ns = time_ns(N, || memoryview_from_buffer_cycle(info));
        println!("  {label} ndim={}  {ns:8.2} ns/export", shape.len());
    }

    // FromMemory public path.
    let mut mbuf = vec![0u8; 4096];
    let mptr = mbuf.as_mut_ptr() as *mut c_char;
    let ns_fm = time_ns(N, || memoryview_from_memory_cycle(mptr, 4096));
    println!("\n-- PyMemoryView_FromMemory raw 1-D public path --");
    println!("  FromMemory  {ns_fm:8.2} ns/export");

    // FillInfo public path.
    let mut fbuf = vec![0u8; 4096];
    let fptr = fbuf.as_mut_ptr() as *mut c_void;
    let ns_fi = time_ns(N, || fillinfo_cycle(fptr, 4096));
    println!("\n-- PyBuffer_FillInfo raw 1-D public path --");
    println!("  FillInfo  {ns_fi:8.2} ns/export  (allocation-free, registry-free)");

    println!(
        "\n(interpretation: memoryview ns/export ≈ control C + descriptor normalize/copy; \
         FillInfo ns/export is pure field writes)\n"
    );
}
