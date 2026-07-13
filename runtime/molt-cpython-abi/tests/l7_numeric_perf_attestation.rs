//! Machine-readable L7 numeric performance attestation.
//!
//! This is deliberately an ignored, release-only integration test. Run it via
//! `tools/bench/run_l7_numeric_attestation.py`; the runner executes the compiled
//! test binary directly so build time is excluded and process peak RSS can be
//! sampled independently of the Rust allocator counters below.

#![allow(clippy::undocumented_unsafe_blocks)]

use molt_cpython_abi::abi_types::{
    Py_True, Py_complex, PyLong_Type, PyLongObject, PyLongValue, PyObject,
};
use molt_cpython_abi::api::errors::{PyErr_Clear, PyErr_Occurred};
use molt_cpython_abi::api::numbers::{
    _Py_c_pow, _Py_c_sum, _PyLong_AsByteArray, _PyLong_FromByteArray, _PyLong_NumBits,
    PyFloat_Pack2, PyFloat_Pack4, PyLong_FromLong, PyLong_FromString,
};
use molt_cpython_abi::api::refcount::Py_DECREF;
use molt_cpython_abi::bridge::{GLOBAL_BRIDGE, molt_capi_pyobj_to_handle};
use molt_cpython_abi::hooks::{INT_BYTES_OK, OwnedHandleResult, STUB_HOOKS};
use molt_lang_obj_model::MoltObject;
use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::{CString, c_char};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

unsafe extern "C" {
    fn molt_l7_overlay_numeric_probe() -> std::os::raw::c_int;
    fn molt_l7_overlay_long_probe(value: *mut PyObject) -> std::os::raw::c_long;
    fn molt_l7_prebuilt_direct_refcnt(value: *mut PyObject) -> isize;
    fn molt_l7_prebuilt_direct_incref(value: *mut PyObject) -> isize;
    fn molt_l7_prebuilt_direct_decref(value: *mut PyObject) -> isize;
}

const SAMPLE_COUNT: usize = 9;
const SCHEMA_VERSION: u32 = 1;
const TARGET_SAMPLE_NS: u128 = 20_000_000;
const MAX_TIMED_ITERATIONS: usize = 1 << 28;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static HOOK_CALLS: AtomicU64 = AtomicU64::new(0);
static BYTE_WIDTH: AtomicUsize = AtomicUsize::new(0);
static TRACK_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static TRACK_HOOKS: AtomicBool = AtomicBool::new(false);
static mut BYTE_HEAP_TOKEN: u8 = 0;
static mut COMPLEX_HEAP_TOKEN: u8 = 0;
static COMPLEX_REAL_BITS: AtomicU64 = AtomicU64::new(0);
static COMPLEX_IMAG_BITS: AtomicU64 = AtomicU64::new(0);

struct CountingAllocator;

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
fn add_allocation(size: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    let live = LIVE_BYTES.fetch_add(size as u64, Ordering::Relaxed) + size as u64;
    raise_peak(live);
}

#[inline]
fn remove_live_bytes(size: usize) {
    let _ = LIVE_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size as u64))
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
            add_allocation(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
            remove_live_bytes(layout.size());
        }
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() && TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
            add_allocation(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(ptr, layout, new_size) };
        if !next.is_null() && TRACK_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            if new_size >= layout.size() {
                let delta = new_size - layout.size();
                let live = LIVE_BYTES.fetch_add(delta as u64, Ordering::Relaxed) + delta as u64;
                raise_peak(live);
            } else {
                remove_live_bytes(layout.size() - new_size);
            }
        }
        next
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[inline]
fn count_hook() {
    if TRACK_HOOKS.load(Ordering::Relaxed) {
        HOOK_CALLS.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe extern "C" fn counted_from_digits(
    digits: *const u8,
    len: usize,
    _base: u32,
    _negative: i32,
) -> u64 {
    count_hook();
    if digits.is_null() || len == 0 {
        return 0;
    }
    black_box(unsafe { *digits.add(len - 1) });
    MoltObject::try_from_int(7).expect("inline int").bits()
}

unsafe extern "C" fn counted_from_bytes(
    data: *const u8,
    len: usize,
    _little_endian: i32,
    _signed: i32,
) -> u64 {
    count_hook();
    BYTE_WIDTH.store(len, Ordering::Relaxed);
    if data.is_null() && len != 0 {
        return 0;
    }
    if len != 0 {
        black_box(unsafe { *data.add(len - 1) });
    }
    MoltObject::from_ptr(&raw mut BYTE_HEAP_TOKEN).bits()
}

unsafe extern "C" fn counted_i64_checked(_bits: u64, _out: *mut i64) -> i32 {
    count_hook();
    -1
}

unsafe extern "C" fn counted_u64_checked(_bits: u64, _out: *mut u64) -> i32 {
    count_hook();
    -1
}

unsafe extern "C" fn counted_binary(op: u32, _left: u64, _right: u64) -> OwnedHandleResult {
    count_hook();
    match op {
        op if op == molt_cpython_abi::hooks::NumberBinaryOp::Add as u32 => {
            OwnedHandleResult::ok(MoltObject::try_from_int(42).expect("inline sum").bits())
        }
        op if op == molt_cpython_abi::hooks::NumberBinaryOp::Rshift as u32 => {
            OwnedHandleResult::ok(MoltObject::try_from_int(0).expect("inline zero").bits())
        }
        _ => OwnedHandleResult::error(),
    }
}

unsafe extern "C" fn counted_to_bytes(
    _bits: u64,
    data: *mut u8,
    len: usize,
    _little_endian: i32,
    _signed: i32,
) -> i32 {
    count_hook();
    if data.is_null() && len != 0 {
        return -1;
    }
    if len != 0 {
        unsafe { std::ptr::write_bytes(data, 0x5a, len) };
    }
    INT_BYTES_OK
}

unsafe extern "C" fn counted_num_bits(_bits: u64, out: *mut usize) -> i32 {
    count_hook();
    if out.is_null() {
        return -1;
    }
    unsafe { *out = BYTE_WIDTH.load(Ordering::Relaxed).saturating_mul(8) };
    0
}

unsafe extern "C" fn counted_int_sign(bits: u64) -> i32 {
    count_hook();
    let byte_token = MoltObject::from_ptr(&raw mut BYTE_HEAP_TOKEN).bits();
    i32::from(bits == byte_token)
}

unsafe extern "C" fn counted_complex_from_doubles(real: f64, imag: f64) -> OwnedHandleResult {
    count_hook();
    COMPLEX_REAL_BITS.store(real.to_bits(), Ordering::Relaxed);
    COMPLEX_IMAG_BITS.store(imag.to_bits(), Ordering::Relaxed);
    OwnedHandleResult::ok(MoltObject::from_ptr(&raw mut COMPLEX_HEAP_TOKEN).bits())
}

unsafe extern "C" fn counted_complex_parts(bits: u64, real: *mut f64, imag: *mut f64) -> i32 {
    count_hook();
    let token = MoltObject::from_ptr(&raw mut COMPLEX_HEAP_TOKEN).bits();
    if bits != token || real.is_null() || imag.is_null() {
        return -1;
    }
    unsafe {
        *real = f64::from_bits(COMPLEX_REAL_BITS.load(Ordering::Relaxed));
        *imag = f64::from_bits(COMPLEX_IMAG_BITS.load(Ordering::Relaxed));
    }
    0
}

unsafe extern "C" fn counted_classify(bits: u64) -> u8 {
    count_hook();
    let byte_token = MoltObject::from_ptr(&raw mut BYTE_HEAP_TOKEN).bits();
    let complex_token = MoltObject::from_ptr(&raw mut COMPLEX_HEAP_TOKEN).bits();
    if bits == byte_token {
        molt_cpython_abi::abi_types::MoltTypeTag::Int as u8
    } else if bits == complex_token {
        molt_cpython_abi::abi_types::MoltTypeTag::Complex as u8
    } else {
        molt_cpython_abi::abi_types::MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn counted_inc_ref(_bits: u64) {
    count_hook();
}

unsafe extern "C" fn counted_dec_ref(bits: u64) {
    count_hook();
    // The boundary-control hook has no backing runtime allocator. Mirror the
    // real terminal runtime path: once c_ref_zero delegates the last heap hold,
    // finalization is vacuous and runtime_object_destroyed retires the canonical
    // view after that (empty) resurrection window.
    if MoltObject::from_bits(bits).is_ptr() {
        GLOBAL_BRIDGE.runtime_object_destroyed(bits);
    }
}

fn initialize_hooks() {
    molt_cpython_abi_test_support::link();
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = STUB_HOOKS;
    hooks.int_from_digits = counted_from_digits;
    hooks.int_from_bytes = counted_from_bytes;
    hooks.int_as_i64_checked = counted_i64_checked;
    hooks.int_as_u64_checked = counted_u64_checked;
    hooks.int_to_bytes = counted_to_bytes;
    hooks.int_num_bits = counted_num_bits;
    hooks.int_sign = counted_int_sign;
    hooks.complex_from_doubles = counted_complex_from_doubles;
    hooks.complex_parts = counted_complex_parts;
    hooks.number_binary_op = counted_binary;
    hooks.classify_heap = counted_classify;
    hooks.inc_ref = counted_inc_ref;
    hooks.dec_ref = counted_dec_ref;
    unsafe {
        assert!(
            molt_cpython_abi::try_set_runtime_hooks(hooks),
            "attestation test binary must own the RuntimeHooks table"
        );
    }
}

#[derive(Clone, Copy)]
struct Sample {
    ns_per_op: f64,
    allocations_per_op: f64,
    allocated_bytes_per_op: f64,
    peak_live_bytes: u64,
    hook_calls_per_op: f64,
}

struct CaseResult {
    name: String,
    family: &'static str,
    input_json: String,
    timed_iterations: usize,
    observer_iterations: usize,
    samples: [Sample; SAMPLE_COUNT],
}

fn reset_sample_counters() {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_LIVE_BYTES.store(0, Ordering::Relaxed);
    HOOK_CALLS.store(0, Ordering::Relaxed);
}

fn assert_no_pending_exception() {
    assert!(
        unsafe { PyErr_Occurred() }.is_null(),
        "ABI attestation left a pending exception"
    );
}

fn assert_semantic_batch(iterations: usize, operation: &mut impl FnMut(usize) -> u64) {
    let mut witnesses = 0_u64;
    for index in 0..iterations {
        witnesses = black_box(witnesses.wrapping_add(operation(black_box(index))));
    }
    assert_eq!(witnesses, iterations as u64, "ABI semantic witness failed");
    assert_no_pending_exception();
}

fn calibrate_timed_iterations(
    seed_iterations: usize,
    operation: &mut impl FnMut(usize) -> u64,
) -> usize {
    let mut iterations = seed_iterations.max(1).min(MAX_TIMED_ITERATIONS);
    loop {
        let started = Instant::now();
        assert_semantic_batch(iterations, operation);
        let elapsed_ns = started.elapsed().as_nanos().max(1);
        if elapsed_ns >= TARGET_SAMPLE_NS || iterations == MAX_TIMED_ITERATIONS {
            return iterations;
        }
        let growth = TARGET_SAMPLE_NS.div_ceil(elapsed_ns).clamp(2, 64) as usize;
        iterations = iterations.saturating_mul(growth).min(MAX_TIMED_ITERATIONS);
    }
}

fn measure_case(
    name: impl Into<String>,
    family: &'static str,
    input_json: String,
    observer_iterations: usize,
    mut operation: impl FnMut(usize) -> u64,
) -> CaseResult {
    let warmup = observer_iterations.clamp(64, 2048);
    assert_semantic_batch(warmup, &mut operation);
    let timed_iterations = calibrate_timed_iterations(observer_iterations, &mut operation);

    // Prime any lazy path while the allocation observer is active, then reset.
    // This keeps one-time test-harness growth out of the steady-state samples
    // without hiding persistent per-operation traffic.
    reset_sample_counters();
    TRACK_ALLOCATIONS.store(true, Ordering::Relaxed);
    TRACK_HOOKS.store(true, Ordering::Relaxed);
    let mut prime_witness = 0_u64;
    for index in 0..observer_iterations {
        prime_witness = black_box(prime_witness.wrapping_add(operation(black_box(index))));
    }
    TRACK_HOOKS.store(false, Ordering::Relaxed);
    TRACK_ALLOCATIONS.store(false, Ordering::Relaxed);
    assert_eq!(prime_witness, observer_iterations as u64);
    assert_no_pending_exception();
    reset_sample_counters();

    let mut samples = [Sample {
        ns_per_op: 0.0,
        allocations_per_op: 0.0,
        allocated_bytes_per_op: 0.0,
        peak_live_bytes: 0,
        hook_calls_per_op: 0.0,
    }; SAMPLE_COUNT];
    for sample in &mut samples {
        TRACK_ALLOCATIONS.store(false, Ordering::Relaxed);
        TRACK_HOOKS.store(false, Ordering::Relaxed);
        let started = Instant::now();
        assert_semantic_batch(timed_iterations, &mut operation);
        let elapsed = started.elapsed().as_nanos() as f64;
        // Count allocation and hook traffic in a separate untimed pass. The
        // timing sample does not pay atomic increments on each observed event.
        reset_sample_counters();
        TRACK_ALLOCATIONS.store(true, Ordering::Relaxed);
        TRACK_HOOKS.store(true, Ordering::Relaxed);
        let mut observer_witness = 0_u64;
        for index in 0..observer_iterations {
            observer_witness =
                black_box(observer_witness.wrapping_add(operation(black_box(index))));
        }
        TRACK_HOOKS.store(false, Ordering::Relaxed);
        TRACK_ALLOCATIONS.store(false, Ordering::Relaxed);
        assert_eq!(observer_witness, observer_iterations as u64);
        assert_no_pending_exception();
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
        let peak = PEAK_LIVE_BYTES.load(Ordering::Relaxed);
        let hooks = HOOK_CALLS.load(Ordering::Relaxed);
        *sample = Sample {
            ns_per_op: elapsed / timed_iterations as f64,
            allocations_per_op: allocations as f64 / observer_iterations as f64,
            allocated_bytes_per_op: bytes as f64 / observer_iterations as f64,
            peak_live_bytes: peak,
            hook_calls_per_op: hooks as f64 / observer_iterations as f64,
        };
    }
    CaseResult {
        name: name.into(),
        family,
        input_json,
        timed_iterations,
        observer_iterations,
        samples,
    }
}

fn decimal_case(digits: usize) -> CaseResult {
    let source = CString::new("9".repeat(digits)).expect("decimal CString");
    let iterations = if digits >= 4096 {
        256
    } else if digits >= 256 {
        1024
    } else {
        4096
    };
    unsafe {
        let value = PyLong_FromString(source.as_ptr(), std::ptr::null_mut(), 10);
        assert!(
            !value.is_null(),
            "decimal preflight failed for {digits} digits"
        );
        Py_DECREF(value);
    }
    measure_case(
        format!("decimal.{digits}"),
        "abi_boundary_control_decimal",
        format!(
            r#"{{"digits":{digits},"base":10,"digit":"9","measurement":"scanner and counted hook boundary only; no runtime BigInt payload"}}"#
        ),
        iterations,
        |_| unsafe {
            let value = PyLong_FromString(source.as_ptr(), std::ptr::null_mut(), 10);
            black_box(value);
            if value.is_null() {
                0
            } else {
                Py_DECREF(value);
                1
            }
        },
    )
}

fn byte_case(width: usize) -> CaseResult {
    let input = vec![0xa5_u8; width];
    let mut output = vec![0_u8; width];
    let iterations = if width >= 4096 {
        256
    } else if width >= 256 {
        1024
    } else {
        4096
    };
    unsafe {
        let value = _PyLong_FromByteArray(input.as_ptr(), width, 1, 0);
        assert!(
            !value.is_null(),
            "byte preflight construction failed for {width}"
        );
        assert_eq!(
            _PyLong_AsByteArray(value.cast(), output.as_mut_ptr(), width, 1, 0),
            0,
            "byte preflight export failed for {width}"
        );
        assert_ne!(_PyLong_NumBits(value), usize::MAX);
        Py_DECREF(value);
    }
    measure_case(
        format!("bytes.{width}"),
        "abi_boundary_control_bytes",
        format!(
            r#"{{"bytes":{width},"little_endian":true,"signed":false,"operations":["from_bytes","to_bytes","num_bits"],"measurement":"counted hook boundary control only; no runtime BigInt payload"}}"#
        ),
        iterations,
        |_| unsafe {
            let value = _PyLong_FromByteArray(input.as_ptr(), width, 1, 0);
            if value.is_null() {
                return 0;
            }
            let export_status = _PyLong_AsByteArray(value.cast(), output.as_mut_ptr(), width, 1, 0);
            let num_bits = _PyLong_NumBits(value);
            let valid = export_status == 0
                && num_bits == width * 8
                && output[0] == 0x5a_u8
                && output[width / 2] == 0x5a_u8
                && output[width - 1] == 0x5a_u8;
            Py_DECREF(value);
            u64::from(black_box(valid))
        },
    )
}

fn float_case(
    format: &'static str,
    class: &'static str,
    value: f64,
    expect_error: bool,
) -> CaseResult {
    let iterations = if expect_error { 4096 } else { 32_768 };
    unsafe {
        let mut output = [0 as c_char; 4];
        let status = if format == "f16" {
            PyFloat_Pack2(value, output.as_mut_ptr(), 1)
        } else {
            PyFloat_Pack4(value, output.as_mut_ptr(), 1)
        };
        assert_eq!(status < 0, expect_error, "float pack preflight failed");
        if expect_error {
            PyErr_Clear();
        }
    }
    measure_case(
        format!("{format}.{class}"),
        "float_pack",
        format!(
            r#"{{"format":"{format}","class":"{class}","value_bits":"{:016x}","expect_error":{expect_error}}}"#,
            value.to_bits()
        ),
        iterations,
        |_| unsafe {
            let mut output = [0 as c_char; 4];
            let status = if format == "f16" {
                PyFloat_Pack2(black_box(value), output.as_mut_ptr(), 1)
            } else {
                PyFloat_Pack4(black_box(value), output.as_mut_ptr(), 1)
            };
            black_box(status);
            black_box(output);
            if expect_error {
                PyErr_Clear();
            }
            u64::from((status < 0) == expect_error)
        },
    )
}

fn prebuilt_direct_refcount_lifetime_witness(bits: u64) {
    let borrowed = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
    assert!(!borrowed.is_null());
    assert_eq!(unsafe { molt_l7_prebuilt_direct_refcnt(borrowed) }, 1);
    assert_eq!(unsafe { molt_l7_prebuilt_direct_incref(borrowed) }, 2);
    assert_eq!(
        GLOBAL_BRIDGE
            .pyobj_to_handle(borrowed)
            .map(|identity| identity.as_handle()),
        Some(bits)
    );
    assert_eq!(
        GLOBAL_BRIDGE.runtime_owner_dropped_to_view_hold(bits),
        Some(false),
        "direct C reference must retain the canonical view after owner drop"
    );
    let retained = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
    assert_eq!(
        retained, borrowed,
        "borrowed view identity changed after owner drop"
    );
    assert_eq!(unsafe { molt_l7_prebuilt_direct_refcnt(retained) }, 1);
    assert_eq!(unsafe { molt_l7_prebuilt_direct_decref(retained) }, 0);
    assert!(
        GLOBAL_BRIDGE.pyobj_to_handle(retained).is_none(),
        "_Py_Dealloc did not retire the zero-refcount managed identity"
    );
}

fn bridge_cases() -> Vec<CaseResult> {
    let scalar = unsafe { PyLong_FromLong(42) };
    assert!(!scalar.is_null());
    let singleton = &raw mut Py_True as *mut _ as *mut PyObject;
    let backing: Vec<Box<u64>> = (0..4096).map(|index| Box::new(index as u64)).collect();
    let heap_bits: Vec<u64> = backing
        .iter()
        .map(|value| MoltObject::from_ptr((&**value as *const u64).cast_mut().cast::<u8>()).bits())
        .collect();
    let managed = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(heap_bits[0]) };

    /* A compiled prebuilt-style consumer retains the exact borrowed view after
     * the runtime owner drains.  This deliberately uses the generic protocol
     * view: only objects with a proven full carrier may be stamped as builtin
     * list/numeric layouts. */
    let lifetime_bits = *heap_bits.last().expect("lifetime witness handle");
    prebuilt_direct_refcount_lifetime_witness(lifetime_bits);

    assert_eq!(unsafe { molt_l7_overlay_long_probe(scalar) }, 42);
    assert_eq!(unsafe { molt_capi_pyobj_to_handle(managed) }, heap_bits[0]);
    let singleton_bits = MoltObject::from_bool(true).bits();
    assert_eq!(
        unsafe { molt_capi_pyobj_to_handle(singleton) },
        singleton_bits
    );
    let cold_preflight = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(heap_bits[1]) };
    assert!(!cold_preflight.is_null());
    assert!(GLOBAL_BRIDGE.release_pyobj(cold_preflight));

    let scalar_decode = measure_case(
        "bridge.canonical_scalar_decode",
        "bridge_c_header",
        r#"{"representation":"canonical_PyLongObject","operation":"compiled_overlay_PyLong_AsLong","legacy_raw_pointer":false}"#.to_owned(),
        65_536,
        |_| unsafe {
            u64::from(black_box(molt_l7_overlay_long_probe(scalar)) == 42)
        },
    );
    let managed_lookup = measure_case(
        "bridge.managed_proxy_lookup",
        "bridge",
        r#"{"representation":"managed_non_scalar","operation":"pyobj_to_handle","expected_proxy_churn":0}"#.to_owned(),
        65_536,
        |_| unsafe {
            u64::from(black_box(molt_capi_pyobj_to_handle(managed)) == heap_bits[0])
        },
    );
    let singleton_lookup = measure_case(
        "bridge.singleton_lookup",
        "bridge",
        r#"{"representation":"canonical_true_singleton","operation":"pyobj_to_handle","expected_proxy_churn":0}"#.to_owned(),
        65_536,
        |_| unsafe {
            u64::from(black_box(molt_capi_pyobj_to_handle(singleton)) == singleton_bits)
        },
    );
    let cold_proxy = measure_case(
        "bridge.cold_proxy_cycle",
        "bridge",
        r#"{"representation":"unique_heap_handle","operation":"handle_to_pyobj+release","working_set":4096}"#.to_owned(),
        4096,
        |index| {
            let bits = heap_bits[(index + 1) % heap_bits.len()];
            let proxy = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
            black_box(proxy);
            u64::from(!proxy.is_null() && black_box(GLOBAL_BRIDGE.release_pyobj(proxy)))
        },
    );

    let mut foreign = Box::new(PyLongObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyLong_Type,
        },
        long_value: PyLongValue {
            lv_tag: 8,
            ob_digit: [42],
        },
    });
    let foreign_ptr = (&raw mut *foreign).cast::<PyObject>();
    assert_eq!(unsafe { molt_l7_overlay_long_probe(foreign_ptr) }, 42);
    let c_header_foreign = measure_case(
        "bridge.c_header_foreign_decode",
        "bridge_c_header",
        r#"{"representation":"foreign_PyLongObject","operation":"compiled_overlay_PyLong_AsLong","scope":"native_test_probe"}"#.to_owned(),
        65_536,
        |_| unsafe {
            u64::from(black_box(molt_l7_overlay_long_probe(foreign_ptr)) == 42)
        },
    );
    assert_eq!(unsafe { molt_l7_overlay_numeric_probe() }, 0);
    let c_header_chain = measure_case(
        "bridge.c_header_canonical_scalar_chain",
        "bridge_c_header",
        r#"{"representation":"canonical_scalar_objects","operation":"constructor+Py_TYPE+exact+PyNumber_Add+decref","legacy_raw_pointer":false}"#.to_owned(),
        4096,
        |_| unsafe {
            u64::from(black_box(molt_l7_overlay_numeric_probe()) == 0)
        },
    );
    unsafe { Py_DECREF(managed) };
    unsafe { Py_DECREF(scalar) };
    drop(foreign);
    drop(heap_bits);
    drop(backing);
    vec![
        scalar_decode,
        managed_lookup,
        singleton_lookup,
        cold_proxy,
        c_header_foreign,
        c_header_chain,
    ]
}

fn complex_cases() -> Vec<CaseResult> {
    let left = Py_complex {
        real: 1.25,
        imag: -2.5,
    };
    let right = Py_complex {
        real: -0.75,
        imag: 4.0,
    };
    let expected_sum = unsafe { _Py_c_sum(left, right) };
    let expected_power = unsafe { _Py_c_pow(left, right) };
    let simple = measure_case(
        "complex.sum",
        "complex",
        r#"{"operation":"_Py_c_sum","left":[1.25,-2.5],"right":[-0.75,4.0]}"#.to_owned(),
        65_536,
        |_| unsafe {
            let result = black_box(_Py_c_sum(black_box(left), black_box(right)));
            u64::from(
                result.real.to_bits() == expected_sum.real.to_bits()
                    && result.imag.to_bits() == expected_sum.imag.to_bits(),
            )
        },
    );
    let power = measure_case(
        "complex.pow",
        "complex",
        r#"{"operation":"_Py_c_pow","base":[1.25,-2.5],"exponent":[-0.75,4.0]}"#.to_owned(),
        16_384,
        |_| unsafe {
            let result = black_box(_Py_c_pow(black_box(left), black_box(right)));
            u64::from(
                result.real.to_bits() == expected_power.real.to_bits()
                    && result.imag.to_bits() == expected_power.imag.to_bits(),
            )
        },
    );
    vec![simple, power]
}

fn required_env(name: &str) -> String {
    let value = std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be provided by the attestation runner"));
    assert!(!value.is_empty(), "{name} must not be empty");
    value
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn cv(values: &[f64]) -> f64 {
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    if mean == 0.0 {
        return 0.0;
    }
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    variance.sqrt() / mean
}

fn metric_summary(samples: &[Sample], field: impl Fn(&Sample) -> f64) -> String {
    let values: Vec<f64> = samples.iter().map(field).collect();
    format!(
        r#"{{"median":{:.6},"cv":{:.6}}}"#,
        median(values.clone()),
        cv(&values)
    )
}

fn case_json(case: &CaseResult) -> String {
    let ns = metric_summary(&case.samples, |sample| sample.ns_per_op);
    let allocations = metric_summary(&case.samples, |sample| sample.allocations_per_op);
    let bytes = metric_summary(&case.samples, |sample| sample.allocated_bytes_per_op);
    let peak = metric_summary(&case.samples, |sample| sample.peak_live_bytes as f64);
    let hooks = metric_summary(&case.samples, |sample| sample.hook_calls_per_op);
    let samples = case
        .samples
        .iter()
        .map(|sample| {
            format!(
                r#"{{"ns_per_op":{:.6},"allocations_per_op":{:.6},"allocated_bytes_per_op":{:.6},"peak_live_bytes":{},"hook_calls_per_op":{:.6}}}"#,
                sample.ns_per_op,
                sample.allocations_per_op,
                sample.allocated_bytes_per_op,
                sample.peak_live_bytes,
                sample.hook_calls_per_op,
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"name":{},"family":{},"input":{},"iterations_per_sample":{},"observer_iterations_per_sample":{},"calibration_target_ns":{},"timing_scope":"loop_inclusive; allocation and hook observers are untimed","sample_count":{},"summary":{{"ns_per_op":{},"allocations_per_op":{},"allocated_bytes_per_op":{},"peak_live_bytes":{},"hook_calls_per_op":{}}},"samples":[{}]}}"#,
        json_string(&case.name),
        json_string(case.family),
        case.input_json,
        case.timed_iterations,
        case.observer_iterations,
        TARGET_SAMPLE_NS,
        SAMPLE_COUNT,
        ns,
        allocations,
        bytes,
        peak,
        hooks,
        samples,
    )
}

fn attestation_json(cases: &[CaseResult]) -> String {
    let git_commit = required_env("MOLT_L7_GIT_COMMIT");
    let git_dirty = required_env("MOLT_L7_GIT_DIRTY") == "true";
    let rustc = required_env("MOLT_L7_RUSTC");
    let build_fingerprint = required_env("MOLT_L7_BUILD_FINGERPRINT");
    let run_nonce = required_env("MOLT_L7_RUN_NONCE");
    let cases = cases.iter().map(case_json).collect::<Vec<_>>().join(",");
    format!(
        r#"{{"schema_version":{},"kind":"l7_numeric_performance_attestation","profile":"release","allocator_scope":"rust_global_allocator","sample_count":{},"host":{{"os":{},"arch":{},"logical_cpus":{}}},"source":{{"git_commit":{},"git_dirty":{},"rustc":{},"build_fingerprint":{},"run_nonce":{}}},"scope":{{"native":true,"wasm32":false,"assembly":false,"code_size":false,"component_rss_only":true}},"coverage":{{"runtime_hook_payload":"ABI boundary control only; real BigInt work is in l7_numeric_runtime_perf_attestation","c_header_probe":"compiled test-gated overlay consumer for canonical scalar type identity, arithmetic chaining, managed non-scalars, and foreign numeric objects","legacy_raw_pointer_lane":false,"process_peak_rss":"component harness only; added by tools/bench/run_l7_numeric_attestation.py","absolute_gates":"canonical scalar/managed/foreign/singleton reads, successful float packs, and complex primitives must remain allocation-free"}},"cases":[{}]}}"#,
        SCHEMA_VERSION,
        SAMPLE_COUNT,
        json_string(std::env::consts::OS),
        json_string(std::env::consts::ARCH),
        std::thread::available_parallelism().map_or(1, usize::from),
        json_string(&git_commit),
        git_dirty,
        json_string(&rustc),
        json_string(&build_fingerprint),
        json_string(&run_nonce),
        cases,
    )
}

fn enforce_allocation_free_cases(cases: &[CaseResult]) {
    const NAMES: &[&str] = &[
        "bridge.canonical_scalar_decode",
        "bridge.managed_proxy_lookup",
        "bridge.singleton_lookup",
        "bridge.c_header_foreign_decode",
        "f16.normal",
        "f16.subnormal",
        "f16.tie",
        "f32.normal",
        "f32.subnormal",
        "f32.tie",
        "complex.sum",
        "complex.pow",
    ];
    for name in NAMES {
        let case = cases
            .iter()
            .find(|case| case.name == *name)
            .unwrap_or_else(|| panic!("missing allocation gate case {name}"));
        for sample in &case.samples {
            assert_eq!(
                sample.allocations_per_op, 0.0,
                "{name} added a steady-state allocation"
            );
            assert_eq!(
                sample.allocated_bytes_per_op, 0.0,
                "{name} added steady-state allocated bytes"
            );
            assert_eq!(
                sample.peak_live_bytes, 0,
                "{name} added steady-state peak live bytes"
            );
        }
    }
}

fn enforce_legacy_raw_lane_absent() {
    let bridge = include_str!("../src/bridge.rs");
    let probe = include_str!("../../molt-cpython-abi-test-support/l7_overlay_probe.c");
    let raw_variant = ["Raw", "Molt"].concat();
    let raw_probe_prefix = ["molt_l7_overlay_", "raw_"].concat();
    assert!(
        !bridge.contains(&raw_variant),
        "legacy raw-handle PyObject variant remains in bridge.rs"
    );
    assert!(
        !probe.contains(&raw_probe_prefix),
        "legacy raw-pointer overlay probe symbol remains"
    );
}

#[test]
fn compiled_prebuilt_direct_refcount_retains_identity_until_zero() {
    initialize_hooks();
    let backing = Box::new(0u64);
    let bits = MoltObject::from_ptr((&raw const *backing).cast_mut().cast::<u8>()).bits();
    prebuilt_direct_refcount_lifetime_witness(bits);
    drop(backing);
}

#[test]
#[ignore = "release profiler; use tools/bench/run_l7_numeric_attestation.py"]
fn l7_numeric_performance_attestation() {
    assert!(
        !cfg!(debug_assertions),
        "L7 numeric attestation is release-only"
    );
    enforce_legacy_raw_lane_absent();
    initialize_hooks();

    let mut cases = Vec::new();
    for digits in [25, 37, 256, 4096, 4300] {
        cases.push(decimal_case(digits));
    }
    for width in [8, 17, 256, 4096] {
        cases.push(byte_case(width));
    }
    cases.extend(bridge_cases());
    for (format, class, value, error) in [
        ("f16", "normal", 1.5, false),
        ("f16", "subnormal", 2f64.powi(-24), false),
        ("f16", "tie", 1.0 + 2f64.powi(-11), false),
        ("f16", "error", 65_520.0, true),
        ("f32", "normal", 1.5, false),
        ("f32", "subnormal", 2f64.powi(-149), false),
        ("f32", "tie", 1.0 + 2f64.powi(-24), false),
        ("f32", "error", f64::MAX, true),
    ] {
        cases.push(float_case(format, class, value, error));
    }
    cases.extend(complex_cases());

    enforce_allocation_free_cases(&cases);
    println!("L7_NUMERIC_ATTESTATION={}", attestation_json(&cases));
}
