//! Profiling + allocation-budget gate for the Py_buffer export→release hot path.
//!
//! This module is `#[cfg(test)]`-only: it is NEVER compiled into the shipped
//! `cdylib`/`rlib`, so the counting global allocator and the timing loops carry
//! ZERO production cost. It exists to make the buffer-export cost
//! *machine-checkable* per M10: a deterministic allocation-budget assertion that
//! fails closed if any change adds a per-export heap allocation, plus an
//! (ignored) wall-clock profiler that emits the ns/export attestation numbers.
//!
//! ## The hot path under test
//!
//! Every `numpy` array ↔ `memoryview` and every buffer-protocol export in
//! compute traverses `api::buffer`:
//!   * `PyObject_GetBuffer` / `copy_pybuffer_for_memoryview` / `PyBuffer_FillInfo`
//!     → `install_buffer_internal` → `Box::new(BufferInternal)` (heap box) +
//!     `register_buffer_internal` (global `Mutex<HashSet>` insert)
//!   * C reads `view.format` / `view.shape` / `view.strides` (pointers into the
//!     boxed descriptor) for the lifetime of the view
//!   * `PyBuffer_Release` → `unregister_buffer_internal` (mutex remove) +
//!     `Box::from_raw` drop (heap free)
//!
//! `BufferInternal` = 8 B `release_kind` + `MoltBufferView` (1104 B) = **1112 B**
//! boxed *per export*, plus a wholesale 1104 B `memcpy` into the box. `shape` /
//! `strides` are fixed inline `[isize; 64]` arrays, so there is NO per-dim heap
//! allocation — the descriptor is O(1) = 1112 B in space regardless of `ndim`;
//! the only `ndim`-dependent work is the O(ndim) copy loop in
//! `descriptor_from_pybuffer` (bounded ndim ≤ 64).

#![allow(clippy::undocumented_unsafe_blocks)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::hint::black_box;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::abi_types::{Py_buffer, PyBUF_FORMAT, PyBUF_STRIDES};
use crate::api::buffer::{PyBuffer_FillInfo, PyBuffer_Release, copy_pybuffer_for_memoryview};

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
// `obj` is left NULL so `descriptor_from_pybuffer` takes `base = 0` (no bridge
// lock) and `install_buffer_internal` skips `Py_INCREF`; `release_kind` is `Raw`
// so `PyBuffer_Release` skips the runtime `buffer_release` hook. The export /
// release box + registry cycle is therefore exercised with zero dependency on
// registered runtime hooks — a self-contained measurement.

/// Backing storage kept alive for the duration of one export variant.
struct Exporter {
    _data: Vec<u8>,
    format: Vec<c_char>,
    shape: Vec<isize>,
    strides: Vec<isize>,
    info: Py_buffer,
}

fn make_exporter(format_code: &str, itemsize: isize, shape: &[isize], strides: &[isize]) -> Box<Exporter> {
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

/// One full export→(C reads)→release cycle through `copy_pybuffer_for_memoryview`.
/// Returns nothing observable; all reads are `black_box`ed so the optimizer
/// cannot elide the C-visible descriptor accesses.
#[inline(never)]
fn export_release_cycle(info: *const Py_buffer) {
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    let rc = unsafe { copy_pybuffer_for_memoryview(&mut view as *mut Py_buffer, info) };
    debug_assert_eq!(rc, 0, "export must succeed");
    // Simulate C reading the descriptor pointers (format walk + shape/strides).
    unsafe {
        black_box(view.len);
        black_box(view.itemsize);
        if !view.format.is_null() {
            let mut p = view.format;
            while *p != 0 {
                black_box(*p);
                p = p.add(1);
            }
        }
        if !view.shape.is_null() && !view.strides.is_null() {
            for i in 0..view.ndim as isize {
                black_box(*view.shape.offset(i));
                black_box(*view.strides.offset(i));
            }
        }
    }
    unsafe { PyBuffer_Release(&mut view as *mut Py_buffer) };
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
        PyBuffer_Release(&mut view as *mut Py_buffer);
    }
}

// ── Allocation-budget GATE (runs in normal `cargo test`) ────────────────────
//
// Deterministic + machine-independent: asserts EXACTLY one heap allocation per
// export in the common case. This is the perf-regression interlock — if an edit
// adds a second per-export allocation (e.g. a Vec for shape/strides, a String
// for format) this fails; if the box is eliminated (arena/inline) the expected
// count is updated DOWN (and can never silently drift up).

/// Measure steady-state allocations for `iters` cycles of `f`, after a warmup
/// pass that grows the registry HashSet so it does not reallocate mid-measure.
fn measure_allocs(iters: usize, mut f: impl FnMut()) -> (f64, f64) {
    for _ in 0..2048 {
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

const EXPECTED_ALLOCS_PER_EXPORT: f64 = 1.0; // the 1112 B BufferInternal box
const EXPECTED_BYTES_PER_EXPORT: f64 = 1112.0;

#[test]
fn buffer_export_allocation_budget() {
    // 1-D uint8 contiguous — the numpy `np.uint8` / bytes-like common case.
    let ex = make_exporter("B", 1, &[4096], &[1]);
    let info = &ex.info as *const Py_buffer;
    let (ap, bp) = measure_allocs(20_000, || export_release_cycle(info));
    assert!(
        (ap - EXPECTED_ALLOCS_PER_EXPORT).abs() < 0.01,
        "1D uint8 export allocations/export = {ap} (expected {EXPECTED_ALLOCS_PER_EXPORT}); \
         a per-export allocation was added or removed — update the budget deliberately",
    );
    assert!(
        (bp - EXPECTED_BYTES_PER_EXPORT).abs() < 1.0,
        "1D uint8 export bytes/export = {bp} (expected {EXPECTED_BYTES_PER_EXPORT})",
    );

    // 3-D float64 strided — worst-case descriptor (still ONE box; shape/strides
    // are inline, so bytes/export is unchanged from the 1-D case).
    let ex3 = make_exporter("d", 8, &[16, 16, 16], &[2048, 128, 8]);
    let info3 = &ex3.info as *const Py_buffer;
    let (ap3, bp3) = measure_allocs(20_000, || export_release_cycle(info3));
    assert!(
        (ap3 - EXPECTED_ALLOCS_PER_EXPORT).abs() < 0.01,
        "3D f64 export allocations/export = {ap3} (expected {EXPECTED_ALLOCS_PER_EXPORT})",
    );
    assert!(
        (bp3 - EXPECTED_BYTES_PER_EXPORT).abs() < 1.0,
        "3D f64 export bytes/export = {bp3} (expected {EXPECTED_BYTES_PER_EXPORT}) — \
         box size is O(1) in ndim (inline [isize;64] shape/strides)",
    );

    // FillInfo raw 1-D path (distinct public entrypoint, same box budget).
    let mut fbuf = vec![0u8; 4096];
    let fptr = fbuf.as_mut_ptr() as *mut c_void;
    let (apf, _bpf) = measure_allocs(20_000, || fillinfo_cycle(fptr, 4096));
    assert!(
        (apf - EXPECTED_ALLOCS_PER_EXPORT).abs() < 0.01,
        "FillInfo export allocations/export = {apf} (expected {EXPECTED_ALLOCS_PER_EXPORT})",
    );
}

// ── Wall-clock profiler (ignored; run manually for the attestation) ─────────
//
//   cargo test -p molt-lang-cpython-abi --release buffer_export_timing_profile \
//       -- --ignored --nocapture
//
// Emits ns/export + allocs/export for every dtype/shape variant plus control
// baselines that attribute the cost between the raw box and the registry.

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

/// Control A: raw `Box::new` of a 1112 B payload + read + drop — isolates the
/// malloc+free+memcpy cost with NO registry / descriptor logic.
#[inline(never)]
fn control_box_only() {
    let b: Box<[u8; 1112]> = Box::new([0u8; 1112]);
    black_box(b[0]);
    black_box(b[1111]);
    drop(black_box(b));
}

/// Control B: a `Mutex<HashSet<usize>>` insert+remove per iter, mirroring the
/// global `BUFFER_INTERNAL_REGISTRY` — isolates the registry mutex + hash cost
/// paid on every export AND every release.
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

/// Control C: `MoltBufferView::default()` build + read — isolates the ~1104 B
/// zero-init of the inline `[isize;64]` shape/strides arrays.
#[inline(never)]
fn control_default_view() {
    let v = crate::hooks::MoltBufferView::default();
    black_box(v.len);
    black_box(v.shape[0]);
    black_box(v.strides[63]);
    black_box(v.format[0]);
}

#[test]
#[ignore = "wall-clock profiler; run with --ignored --nocapture --release"]
fn buffer_export_timing_profile() {
    const N: usize = 200_000;

    println!("\n=== Py_buffer export→release profile (N={N}/variant) ===");
    println!("BufferInternal box = 1112 B; MoltBufferView = 1104 B (inline [isize;64] shape+strides)\n");

    // Control baselines (attribution of the ~144 ns total).
    let ns_box = time_ns(N, control_box_only);
    println!("control A: Box<[u8;1112]> new+read+drop       {ns_box:8.2} ns  (raw malloc+free+memcpy floor)");
    let ns_reg = time_ns(N, control_registry_only);
    println!("control B: Mutex<HashSet> insert+remove       {ns_reg:8.2} ns  (registry cost / export)");
    let ns_def = time_ns(N, control_default_view);
    println!("control C: MoltBufferView::default() +read    {ns_def:8.2} ns  (1104 B zero-init)");

    // dtype sweep — 1-D contiguous, the numpy scalar-dtype common case.
    let dtypes: &[(&str, isize)] = &[
        ("B", 1), ("b", 1), ("H", 2), ("h", 2), ("I", 4), ("i", 4),
        ("L", 8), ("l", 8), ("Q", 8), ("q", 8), ("f", 4), ("d", 8),
    ];
    println!("\n-- 1-D contiguous, shape=[4096], per numpy dtype --");
    for &(fmt, isz) in dtypes {
        let ex = make_exporter(fmt, isz, &[4096], &[isz]);
        let info = &ex.info as *const Py_buffer;
        let ns = time_ns(N, || export_release_cycle(info));
        let (ap, bp) = measure_allocs(20_000, || export_release_cycle(info));
        println!("  dtype '{fmt}' itemsize={isz:<2}  {ns:8.2} ns/export   {ap:.3} allocs/export  {bp:.0} B/export");
    }

    // ndim sweep — shows the O(ndim) descriptor copy loop is a small term.
    println!("\n-- float64, ndim sweep (strided) --");
    let shapes: &[(&str, Vec<isize>, Vec<isize>)] = &[
        ("1-D", vec![4096], vec![8]),
        ("2-D", vec![64, 64], vec![512, 8]),
        ("3-D", vec![16, 16, 16], vec![2048, 128, 8]),
        ("4-D", vec![8, 8, 8, 8], vec![4096, 512, 64, 8]),
    ];
    for (label, shape, strides) in shapes {
        let ex = make_exporter("d", 8, shape, strides);
        let info = &ex.info as *const Py_buffer;
        let ns = time_ns(N, || export_release_cycle(info));
        println!("  {label} ndim={}  {ns:8.2} ns/export", shape.len());
    }

    // FillInfo public path.
    let mut fbuf = vec![0u8; 4096];
    let fptr = fbuf.as_mut_ptr() as *mut c_void;
    let ns_fi = time_ns(N, || fillinfo_cycle(fptr, 4096));
    println!("\n-- PyBuffer_FillInfo raw 1-D public path --");
    println!("  FillInfo  {ns_fi:8.2} ns/export");

    println!("\n(interpretation: ns/export - control ≈ registry Mutex<HashSet> + descriptor memcpy + apply_molt_view)\n");
}
