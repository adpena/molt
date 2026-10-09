//! Runtime-backed companion to the L7 numeric ABI performance attestation.
//!
//! Unlike the ABI boundary-control executable, these cases register the real
//! `molt-runtime` hooks. Decimal construction therefore executes
//! `BigInt::from_radix_be`, and byte export / `_PyLong_NumBits` execute the real
//! heap-BigInt paths. Allocation and hook statistics come from a test-feature
//! wrapper around the production mimalloc allocator. The wrapper is disabled
//! during timed loops and enabled only for separate observer passes.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::undocumented_unsafe_blocks)]

use molt_cpython_abi::api::errors::{PyErr_ExceptionMatches, PyErr_Occurred};
use molt_cpython_abi::api::numbers::{
    _PyLong_AsByteArray, _PyLong_FromByteArray, _PyLong_NumBits, PyFloat_AsDouble,
    PyFloat_FromDouble, PyLong_AsLong, PyLong_FromLong, PyLong_FromString,
};
use molt_cpython_abi::api::refcount::Py_DECREF;
use molt_cpython_abi::l7_attestation::{
    CALIBRATION_TARGET_NS, MINIMUM_SAMPLE_NS, SAMPLE_COUNT, calibrate_timed_iterations,
    enforce_current_thread_affinity, normalized_affinity_mask, summarize_samples,
};
use molt_runtime::attestation_probe;
use num_bigint::BigUint;
use serde_json::{Value, json};
use std::ffi::CString;
use std::hint::black_box;
use std::time::Instant;

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/l7_numeric_runtime_perf_attestation"
));

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    fn molt_exception_clear() -> u64;
}

#[derive(Clone, Copy)]
struct Sample {
    ns_per_op: f64,
    allocations_per_op: f64,
    allocated_bytes_per_op: f64,
    peak_live_bytes: u64,
    numeric_hook_calls_per_op: f64,
}

fn initialize_runtime() {
    unsafe {
        molt_runtime_init();
        molt_exception_clear();
    }
    molt_runtime::cpython_abi_hooks::register_cpython_hooks();
}

fn assert_no_pending_exception() {
    assert!(
        unsafe { PyErr_Occurred() }.is_null(),
        "numeric attestation left a pending exception"
    );
}

fn assert_semantic_batch(iterations: usize, operation: &mut impl FnMut() -> u64) {
    let mut witnesses = 0_u64;
    for _ in 0..iterations {
        witnesses = black_box(witnesses.wrapping_add(operation()));
    }
    assert_eq!(
        witnesses, iterations as u64,
        "numeric attestation operation failed its semantic witness"
    );
    assert_no_pending_exception();
}

fn calibrate_case_iterations(seed_iterations: usize, operation: &mut impl FnMut() -> u64) -> usize {
    calibrate_timed_iterations(seed_iterations, |iterations| {
        let started = Instant::now();
        assert_semantic_batch(iterations, operation);
        started.elapsed().as_nanos()
    })
}

#[derive(Clone, Copy)]
enum CaseFamily {
    BigInt,
    ScalarBridge,
    SharedHashStorage,
}
impl CaseFamily {
    fn name(self) -> &'static str {
        match self {
            Self::BigInt => "runtime_bigint",
            Self::ScalarBridge => "runtime_scalar_bridge",
            Self::SharedHashStorage => "runtime_shared_hash_storage",
        }
    }
}

fn measure(
    name: &str,
    family: CaseFamily,
    input: Value,
    observer_iterations: usize,
    mut operation: impl FnMut() -> u64,
) -> Value {
    assert_semantic_batch(observer_iterations.clamp(64, 1024), &mut operation);
    let iterations = calibrate_case_iterations(observer_iterations, &mut operation);

    attestation_probe::reset();
    attestation_probe::set_tracking(true);
    let mut prime_witness = 0_u64;
    for _ in 0..observer_iterations {
        prime_witness = black_box(prime_witness.wrapping_add(operation()));
    }
    attestation_probe::set_tracking(false);
    assert_eq!(prime_witness, observer_iterations as u64);
    assert_no_pending_exception();
    attestation_probe::reset();

    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let started = Instant::now();
        assert_semantic_batch(iterations, &mut operation);
        let elapsed = started.elapsed().as_nanos() as f64;

        attestation_probe::reset();
        attestation_probe::set_tracking(true);
        let mut observer_witness = 0_u64;
        for _ in 0..observer_iterations {
            observer_witness = black_box(observer_witness.wrapping_add(operation()));
        }
        attestation_probe::set_tracking(false);
        assert_eq!(observer_witness, observer_iterations as u64);
        assert_no_pending_exception();
        let observed = attestation_probe::snapshot();
        samples.push(Sample {
            ns_per_op: elapsed / iterations as f64,
            allocations_per_op: observed.allocations as f64 / observer_iterations as f64,
            allocated_bytes_per_op: observed.allocated_bytes as f64 / observer_iterations as f64,
            peak_live_bytes: observed.peak_live_bytes,
            numeric_hook_calls_per_op: observed.numeric_hook_calls as f64
                / observer_iterations as f64,
        });
    }
    json!({
        "name": name,
        "family": family.name(),
        "input": input,
        "iterations_per_sample": iterations,
        "observer_iterations_per_sample": observer_iterations,
        "calibration_target_ns": CALIBRATION_TARGET_NS,
        "minimum_sample_ns": MINIMUM_SAMPLE_NS,
        "timing_scope": "loop_inclusive; allocation and hook observers are untimed",
        "sample_count": SAMPLE_COUNT,
        "summary": {
            "ns_per_op": summary(&samples, |sample| sample.ns_per_op),
            "allocations_per_op": summary(&samples, |sample| sample.allocations_per_op),
            "allocated_bytes_per_op": summary(&samples, |sample| sample.allocated_bytes_per_op),
            "peak_live_bytes": summary(&samples, |sample| sample.peak_live_bytes as f64),
            "numeric_hook_calls_per_op": summary(
                &samples,
                |sample| sample.numeric_hook_calls_per_op,
            ),
        },
        "samples": samples.iter().map(|sample| json!({
            "ns_per_op": sample.ns_per_op,
            "allocations_per_op": sample.allocations_per_op,
            "allocated_bytes_per_op": sample.allocated_bytes_per_op,
            "peak_live_bytes": sample.peak_live_bytes,
            "numeric_hook_calls_per_op": sample.numeric_hook_calls_per_op,
        })).collect::<Vec<_>>(),
    })
}

fn summary(samples: &[Sample], field: impl Fn(&Sample) -> f64) -> Value {
    let values: Vec<f64> = samples.iter().map(field).collect();
    let summary = summarize_samples(&values);
    json!({
        "median": summary.median,
        "cv": summary.cv,
        "robust_cv": summary.robust_cv,
    })
}

fn decimal_literal(digits: usize) -> (CString, &'static str) {
    if digits != 4096 {
        return (
            CString::new("9".repeat(digits)).expect("decimal CString"),
            "dense_nines",
        );
    }
    let mut exponent = 13_600usize;
    loop {
        let text = (BigUint::from(1u8) << exponent).to_str_radix(10);
        match text.len().cmp(&digits) {
            std::cmp::Ordering::Equal => {
                return (
                    CString::new(text).expect("power-of-two CString"),
                    "power_of_two",
                );
            }
            std::cmp::Ordering::Less => exponent += 1,
            std::cmp::Ordering::Greater => exponent -= 1,
        }
    }
}

fn decimal_case(digits: usize) -> Value {
    let (source, value_class) = decimal_literal(digits);
    let expected_bits = BigUint::parse_bytes(source.as_bytes(), 10)
        .expect("decimal preflight BigUint")
        .bits() as usize;
    let iterations = if digits >= 4096 {
        128
    } else if digits >= 256 {
        512
    } else {
        2048
    };
    unsafe {
        let value = PyLong_FromString(source.as_ptr(), std::ptr::null_mut(), 10);
        assert!(
            !value.is_null(),
            "runtime decimal preflight failed for {digits}"
        );
        assert_eq!(
            _PyLong_NumBits(value),
            expected_bits,
            "runtime decimal value changed for {digits} digits"
        );
        Py_DECREF(value);
    }
    measure(
        &format!("runtime.decimal.{digits}"),
        CaseFamily::BigInt,
        json!({
            "digits": digits,
            "base": 10,
            "value_class": value_class,
            "real_runtime_hook": "int_from_digits",
        }),
        iterations,
        || unsafe {
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

fn byte_case(width: usize) -> Value {
    let input = vec![0xa5_u8; width];
    let mut output = vec![0_u8; width];
    let iterations = if width >= 4096 {
        128
    } else if width >= 256 {
        512
    } else {
        2048
    };
    unsafe {
        let value = _PyLong_FromByteArray(input.as_ptr(), width, 1, 0);
        assert!(
            !value.is_null(),
            "runtime byte preflight failed for {width}"
        );
        assert_eq!(
            _PyLong_AsByteArray(value.cast(), output.as_mut_ptr(), width, 1, 0),
            0
        );
        assert_eq!(output, input, "runtime byte round-trip changed the value");
        assert_ne!(_PyLong_NumBits(value), usize::MAX);
        Py_DECREF(value);
    }
    measure(
        &format!("runtime.bytes.{width}"),
        CaseFamily::BigInt,
        json!({
            "bytes": width,
            "little_endian": true,
            "signed": false,
            "operations": ["int_from_bytes", "int_to_bytes", "int_num_bits"],
            "real_runtime_hooks": true,
        }),
        iterations,
        || unsafe {
            let value = _PyLong_FromByteArray(input.as_ptr(), width, 1, 0);
            if value.is_null() {
                return 0;
            }
            let export_status = _PyLong_AsByteArray(value.cast(), output.as_mut_ptr(), width, 1, 0);
            let num_bits = _PyLong_NumBits(value);
            let midpoint = width / 2;
            let valid = export_status == 0
                && num_bits == width * 8
                && output[0] == input[0]
                && output[midpoint] == input[midpoint]
                && output[width - 1] == input[width - 1];
            Py_DECREF(value);
            u64::from(black_box(valid))
        },
    )
}

#[derive(Clone, Copy)]
enum ScalarKind {
    Integer,
    Float,
}
impl ScalarKind {
    fn name(self) -> &'static str {
        match self {
            Self::Integer => "int",
            Self::Float => "float",
        }
    }
    fn value(self) -> Value {
        match self {
            Self::Integer => json!(1000),
            Self::Float => json!(1.25),
        }
    }
    unsafe fn create(self) -> *mut molt_cpython_abi::abi_types::PyObject {
        unsafe {
            match self {
                Self::Integer => PyLong_FromLong(1000),
                Self::Float => PyFloat_FromDouble(1.25),
            }
        }
    }
    unsafe fn valid(self, object: *mut molt_cpython_abi::abi_types::PyObject) -> bool {
        if object.is_null() {
            return false;
        }
        unsafe {
            match self {
                Self::Integer => PyLong_AsLong(object) == 1000,
                Self::Float => PyFloat_AsDouble(object).to_bits() == 1.25_f64.to_bits(),
            }
        }
    }
}

unsafe fn scalar_bridge_operation(kind: ScalarKind, roundtrip: bool) -> u64 {
    unsafe {
        let object = kind.create();
        if object.is_null() {
            return 0;
        }
        let object = if roundtrip {
            let bits = molt_cpython_abi::bridge::molt_capi_pyobj_to_handle(object);
            if !PyErr_Occurred().is_null() {
                Py_DECREF(object);
                return 0;
            }
            // Manufacture the runtime owner before releasing the original C
            // owner. The result crossing consumes exactly this runtime hold.
            (molt_cpython_abi::hooks::hooks_or_stubs().inc_ref)(bits);
            Py_DECREF(object);
            molt_cpython_abi::bridge::molt_capi_result_to_pyobj(bits)
        } else {
            object
        };
        let valid = kind.valid(object);
        if !object.is_null() {
            Py_DECREF(object);
        }
        u64::from(black_box(valid))
    }
}

fn scalar_bridge_case(kind: ScalarKind, roundtrip: bool) -> Value {
    let operation = if roundtrip {
        "runtime_hold_roundtrip"
    } else {
        "construct_extract_release"
    };
    measure(
        &format!("runtime.scalar.{}.{operation}", kind.name()),
        CaseFamily::ScalarBridge,
        json!({"scalar": kind.name(), "value": kind.value(), "operation": operation, "real_runtime_hooks": true}),
        2048,
        || unsafe { scalar_bridge_operation(kind, roundtrip) },
    )
}

// Diagnostic only for the unchanged baseline. The independent correctness
// regression requires preservation in the candidate. Compare while BOTH C
// references are alive, so allocator address reuse cannot fake identity.
fn scalar_origin_preserved(kind: ScalarKind) -> bool {
    unsafe {
        let original = kind.create();
        assert!(!original.is_null());
        let bits = molt_cpython_abi::bridge::molt_capi_pyobj_to_handle(original);
        assert_no_pending_exception();
        (molt_cpython_abi::hooks::hooks_or_stubs().inc_ref)(bits);
        let returned = molt_cpython_abi::bridge::molt_capi_result_to_pyobj(bits);
        assert!(kind.valid(returned));
        let preserved = original == returned;
        Py_DECREF(original);
        assert!(kind.valid(returned));
        Py_DECREF(returned);
        assert_no_pending_exception();
        preserved
    }
}

fn required_env(name: &str) -> String {
    let value = std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be provided by the attestation runner"));
    assert!(!value.is_empty(), "{name} must not be empty");
    value
}

#[test]
#[ignore = "release runtime profiler; use tools/bench/run_l7_numeric_attestation.py"]
fn l7_numeric_runtime_performance_attestation() {
    assert!(
        !cfg!(debug_assertions),
        "L7 numeric runtime attestation is release-only"
    );
    let affinity_mask = enforce_current_thread_affinity(&required_env("MOLT_L7_AFFINITY_MASK"));
    // Execute the allocation/failure contract in this exact admitted image
    // before collecting costs; its preparation is outside every timed loop.
    integer_byte_encoder_runs_with_allocation_denied_after_preparation();
    let mut cases = Vec::new();
    for digits in [25, 37, 256, 4096, 4300] {
        cases.push(decimal_case(digits));
    }
    for width in [1, 2, 4, 8, 17, 256, 4096] {
        cases.push(byte_case(width));
    }
    for kind in [ScalarKind::Integer, ScalarKind::Float] {
        for roundtrip in [false, true] {
            cases.push(scalar_bridge_case(kind, roundtrip));
        }
    }
    let integer_origin = scalar_origin_preserved(ScalarKind::Integer);
    let float_origin = scalar_origin_preserved(ScalarKind::Float);
    let payload = json!({
        "schema_version": 3,
        "kind": "l7_numeric_runtime_performance_attestation",
        "profile": "release",
        "allocator_scope": "test_feature_counting_wrapper_over_production_mimalloc",
        "sample_count": SAMPLE_COUNT,
        "scope": {
            "native": true,
            "wasm32": false,
            "assembly": false,
            "code_size": false,
            "component_rss_only": true,
        },
        "host": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logical_cpus": std::thread::available_parallelism().map_or(1, usize::from),
        },
        "execution_control": {
            "affinity_mask": normalized_affinity_mask(affinity_mask),
            "scope": "current_benchmark_thread",
        },
        "source": {
            "git_commit": required_env("MOLT_L7_GIT_COMMIT"),
            "git_dirty": required_env("MOLT_L7_GIT_DIRTY") == "true",
            "rustc": required_env("MOLT_L7_RUSTC"),
            "build_fingerprint": required_env("MOLT_L7_BUILD_FINGERPRINT"),
            "run_nonce": required_env("MOLT_L7_RUN_NONCE"),
        },
        "coverage": {
            "scalar_integer_origin_preserved": integer_origin,
            "scalar_float_origin_preserved": float_origin,
            "scalar_identity_scope": "diagnostic only; independent candidate correctness gate; compared with both C owners alive",
            "decimal": "real RuntimeHooks::int_from_digits and BigInt::from_radix_be",
            "bytes": "real RuntimeHooks int_from_bytes/int_to_bytes/int_num_bits",
            "numeric_hook_calls_per_op": "observed in the separate untimed probe pass",
            "semantic_witness": "every batch must complete every operation, preserve representative value bits, and leave no pending exception",
            "process_peak_rss": "component harness only; added by tools/bench/run_l7_numeric_attestation.py",
        },
        "cases": cases,
    });
    println!("L7_NUMERIC_RUNTIME_ATTESTATION={payload}");
}

// Keep storage measurements in the existing allocator/affinity/sample authority.
// These exported operations are identical in the dense control and sparse
// candidate; no representation inspection or instrumentation enters guest code.
unsafe extern "C" {
    fn molt_dict_new(capacity: u64) -> u64;
    fn molt_dict_set(dict: u64, key: u64, value: u64) -> u64;
    fn molt_dict_get(dict: u64, key: u64, default: u64) -> u64;
    fn molt_dict_pop(dict: u64, key: u64, default: u64, has_default: u64) -> u64;
    fn molt_dict_items(dict: u64) -> u64;
    fn molt_bit_xor(left: u64, right: u64) -> u64;
    fn molt_len(value: u64) -> u64;
    fn molt_iter(value: u64) -> u64;
    fn molt_iter_next_unboxed(iter: u64, value_out: u64) -> u64;
    fn molt_set_new(capacity: u64) -> u64;
    fn molt_set_add(set: u64, key: u64) -> u64;
    fn molt_set_discard(set: u64, key: u64) -> u64;
    fn molt_frozenset_new(capacity: u64) -> u64;
    fn molt_frozenset_add(set: u64, key: u64) -> u64;
    fn molt_hash_builtin(value: u64) -> u64;
    fn molt_eq(left: u64, right: u64) -> u64;
    fn molt_dict_popitem(dict: u64) -> u64;
    fn molt_set_pop(set: u64) -> u64;
    fn molt_frozenset_difference_multi(set: u64, others: u64) -> u64;
    fn molt_frozenset_symmetric_difference(set: u64, other: u64) -> u64;
    fn molt_list_from_values(address: u64, len: u64) -> u64;
    fn molt_tuple_from_values(address: u64, len: u64) -> u64;
    fn molt_list_extend(list: u64, source: u64) -> u64;
    fn molt_dec_ref_obj(value: u64);
}

fn storage_int(value: usize) -> u64 {
    molt_obj_model::MoltObject::from_int(value as i64).bits()
}

unsafe fn storage_dict(count: usize) -> u64 {
    unsafe {
        let dict = molt_dict_new(count as u64);
        for key in 0..count {
            molt_dict_set(dict, storage_int(key), storage_int(key + 1));
        }
        dict
    }
}

unsafe fn storage_set(count: usize, frozen: bool) -> u64 {
    unsafe {
        let set = if frozen {
            molt_frozenset_new(count as u64)
        } else {
            molt_set_new(count as u64)
        };
        for key in 0..count {
            if frozen {
                molt_frozenset_add(set, storage_int(key));
            } else {
                molt_set_add(set, storage_int(key));
            }
        }
        set
    }
}

fn storage_measure(name: &str, input: Value, operation: impl FnMut() -> u64) -> Value {
    measure(name, CaseFamily::SharedHashStorage, input, 4, operation)
}

unsafe fn storage_walk(value: u64, expected_count: usize, expected_sum: usize) -> u64 {
    unsafe {
        let iter = molt_iter(value);
        let mut item = 0;
        let mut count = 0;
        let mut sum = 0usize;
        loop {
            let done = molt_iter_next_unboxed(iter, (&raw mut item) as usize as u64);
            if molt_obj_model::MoltObject::from_bits(done).as_bool() == Some(true) {
                break;
            }
            sum = sum.wrapping_add(
                molt_obj_model::MoltObject::from_bits(item)
                    .as_int()
                    .expect("integer key") as usize,
            );
            count += 1;
            molt_dec_ref_obj(item);
        }
        molt_dec_ref_obj(iter);
        u64::from(count == expected_count && sum == expected_sum)
    }
}

#[test]
#[ignore = "release-only matched dense/sparse storage performance attestation"]
fn shared_hash_storage_performance_attestation() {
    assert!(
        !cfg!(debug_assertions),
        "storage attestation is release-only"
    );
    let affinity_mask = enforce_current_thread_affinity(&required_env("MOLT_L7_AFFINITY_MASK"));
    initialize_runtime();
    let mut cases = Vec::new();
    unsafe {
        for n in [0usize, 1, 4, 8] {
            for set in [false, true] {
                cases.push(storage_measure(if set { "small_set_construct_release" } else { "small_dict_construct_release" }, json!({"n": n, "scope": "construction and release, including backing descriptors and payload"}), || {
                    let bits = if set { storage_set(n, false) } else { storage_dict(n) };
                    let valid = molt_len(bits) == storage_int(n);
                    molt_dec_ref_obj(bits);
                    u64::from(valid)
                }));
            }
        }
        for n in [256usize, 512, 1024] {
            cases.push(storage_measure(
                "equal_items_cancellation",
                json!({"n": n, "scope": "construct two dictionaries, views, xor, release"}),
                || {
                    let left = storage_dict(n);
                    let right = storage_dict(n);
                    let left_view = molt_dict_items(left);
                    let right_view = molt_dict_items(right);
                    let result = molt_bit_xor(left_view, right_view);
                    let valid = molt_len(result) == storage_int(0);
                    for bits in [result, left_view, right_view, left, right] {
                        molt_dec_ref_obj(bits);
                    }
                    u64::from(valid)
                },
            ));
            for set in [false, true] {
                cases.push(storage_measure(
                    if set {
                        "set_pop_all"
                    } else {
                        "dict_popitem_all"
                    },
                    json!({"n": n, "scope": "construct, pop all members, release"}),
                    || {
                        let bits = if set {
                            storage_set(n, false)
                        } else {
                            storage_dict(n)
                        };
                        for _ in 0..n {
                            let value = if set {
                                molt_set_pop(bits)
                            } else {
                                molt_dict_popitem(bits)
                            };
                            molt_dec_ref_obj(value);
                        }
                        let valid = molt_len(bits) == storage_int(0);
                        molt_dec_ref_obj(bits);
                        u64::from(valid)
                    },
                ));
            }
            let snapshot_dict = storage_dict(n);
            cases.push(storage_measure(
                "dictionary_items_snapshot",
                json!({"n": n, "scope": "new list from dict items; source construction excluded"}),
                || {
                    let view = molt_dict_items(snapshot_dict);
                    let list = molt_list_from_values(0, 0);
                    molt_list_extend(list, view);
                    let valid = molt_len(list) == storage_int(n);
                    for bits in [list, view] {
                        molt_dec_ref_obj(bits);
                    }
                    u64::from(valid)
                },
            ));
            molt_dec_ref_obj(snapshot_dict);
            let frozen_source = storage_set(n * 4, true);
            let remove_keys: Vec<u64> = (1..n * 4).map(storage_int).collect();
            let remove_list = molt_list_from_values(
                remove_keys.as_ptr() as usize as u64,
                remove_keys.len() as u64,
            );
            let arg = [remove_list];
            let others = molt_tuple_from_values(arg.as_ptr() as usize as u64, 1);
            for symmetric in [false, true] {
                let construct = || {
                    if symmetric {
                        molt_frozenset_symmetric_difference(frozen_source, remove_list)
                    } else {
                        molt_frozenset_difference_multi(frozen_source, others)
                    }
                };
                cases.push(storage_measure(if symmetric { "sparse_frozen_symdiff_first_hash" } else { "sparse_frozen_generic_difference_first_hash" }, json!({"n": n, "inserted_extent": n * 4, "live": 1, "scope": "result construction, first hash, release; source operands excluded"}), || {
                    let frozen = construct();
                    let hash = molt_hash_builtin(frozen);
                    let valid = molt_len(frozen) == storage_int(1);
                    for bits in [hash, frozen] { molt_dec_ref_obj(bits); }
                    u64::from(valid)
                }));
                let frozen = construct();
                let expected = molt_hash_builtin(frozen);
                cases.push(storage_measure(if symmetric { "sparse_frozen_symdiff_cached_hash" } else { "sparse_frozen_generic_difference_cached_hash" }, json!({"n": n, "inserted_extent": n * 4, "live": 1, "scope": "cached hash only; result construction and first hash excluded"}), || {
                    let hash = molt_hash_builtin(frozen);
                    let valid = molt_obj_model::MoltObject::from_bits(molt_eq(hash, expected)).as_bool() == Some(true);
                    molt_dec_ref_obj(hash);
                    u64::from(valid)
                }));
                for bits in [expected, frozen] {
                    molt_dec_ref_obj(bits);
                }
            }
            for bits in [others, remove_list, frozen_source] {
                molt_dec_ref_obj(bits);
            }
            cases.push(storage_measure("dict_delete_reinsert_churn", json!({"n": n, "rounds": 4, "scope": "construct, four erase/reinsert rounds, release"}), || {
            let dict = storage_dict(n);
                for round in 0..4 {
                    for key in 0..n {
                        let old = molt_dict_pop(dict, storage_int(round * n + key), 0, storage_int(0));
                        molt_dec_ref_obj(old);
                        molt_dict_set(dict, storage_int((round + 1) * n + key), storage_int(key + 1));
                    }
                }
                let valid = molt_len(dict) == storage_int(n);
                molt_dec_ref_obj(dict);
                u64::from(valid)
            }));
            cases.push(storage_measure("set_delete_reinsert_churn", json!({"n": n, "rounds": 4, "scope": "construct, four erase/reinsert rounds, release"}), || {
                let set = storage_set(n, false);
                for round in 0..4 {
                    for key in 0..n {
                        molt_set_discard(set, storage_int(round * n + key));
                        molt_set_add(set, storage_int((round + 1) * n + key));
                    }
                }
                let valid = molt_len(set) == storage_int(n);
                molt_dec_ref_obj(set);
                u64::from(valid)
            }));
            let dict = storage_dict(n);
            cases.push(storage_measure("dense_dict_lookup", json!({"n": n, "lookups_per_op": n, "scope": "lookup batch; construction excluded"}), || {
                let mut sum = 0usize;
                for key in 0..n {
                    let value = molt_dict_get(dict, storage_int(key), storage_int(0));
                    sum += molt_obj_model::MoltObject::from_bits(value).as_int().expect("lookup result") as usize;
                    molt_dec_ref_obj(value);
                }
                u64::from(sum == n * (n + 1) / 2)
            }));
            molt_dec_ref_obj(dict);
            for live in [1usize, n] {
                let dict = storage_dict(n * 4);
                let set = storage_set(n * 4, false);
                for key in 0..(n * 4 - live) {
                    let value = molt_dict_pop(dict, storage_int(key), 0, storage_int(0));
                    molt_dec_ref_obj(value);
                    molt_set_discard(set, storage_int(key));
                }
                let sum = (n * 4 - live + n * 4 - 1) * live / 2;
                for (kind, bits) in [
                    ("sparse_dict_full_walk", dict),
                    ("sparse_set_full_walk", set),
                ] {
                    cases.push(storage_measure(kind, json!({"n": n, "inserted_extent": n * 4, "live": live, "scope": "complete iterator walk; construction and deletion excluded"}), || storage_walk(bits, live, sum)));
                    molt_dec_ref_obj(bits);
                }
            }
            cases.push(storage_measure(
                "frozenset_construct_first_hash",
                json!({"n": n, "scope": "construct, first hash, release"}),
                || {
                    let frozen = storage_set(n, true);
                    let hash = molt_hash_builtin(frozen);
                    let valid = molt_len(frozen) == storage_int(n);
                    molt_dec_ref_obj(hash);
                    molt_dec_ref_obj(frozen);
                    u64::from(valid)
                },
            ));
            let frozen = storage_set(n, true);
            let expected = molt_hash_builtin(frozen);
            cases.push(storage_measure(
                "frozenset_cached_hash",
                json!({"n": n, "scope": "cached hash only; construction and first hash excluded"}),
                || {
                    let hash = molt_hash_builtin(frozen);
                    let valid = molt_obj_model::MoltObject::from_bits(molt_eq(hash, expected))
                        .as_bool()
                        == Some(true);
                    molt_dec_ref_obj(hash);
                    u64::from(valid)
                },
            ));
            molt_dec_ref_obj(expected);
            molt_dec_ref_obj(frozen);
        }
    }
    println!(
        "SHARED_HASH_STORAGE_ATTESTATION={}",
        json!({
            "schema_version": 1,
            "kind": "shared_hash_storage_performance_attestation",
            "profile": "release",
            "allocator_scope": "existing test feature observer over production mimalloc; disabled in timed loops",
            "source": {
                "git_commit": required_env("MOLT_L7_GIT_COMMIT"),
                "git_dirty": required_env("MOLT_L7_GIT_DIRTY"),
                "rustc": required_env("MOLT_L7_RUSTC"),
                "build_fingerprint": required_env("MOLT_L7_BUILD_FINGERPRINT"),
                "run_nonce": required_env("MOLT_L7_RUN_NONCE")
            },
            "host": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
            "affinity_mask": normalized_affinity_mask(affinity_mask),
            "callback_scope": "immediate integer keys have no Python callbacks; reentrant callbacks require separate semantic fixtures",
            "coverage": {"process_tree_memory_and_artifact_size": "external guard and pinned artifact census", "wasm": false},
            "cases": cases
        })
    );
}

#[test]
fn integer_byte_encoder_runs_with_allocation_denied_after_preparation() {
    use std::alloc::{Layout, alloc, dealloc};
    initialize_runtime();
    let hooks = molt_cpython_abi::hooks::hooks().expect("real numeric hooks");
    let source = [0x35u8; 33];
    let bits = unsafe { (hooks.int_from_bytes)(source.as_ptr(), source.len(), 1, 0) };
    assert_ne!(bits, 0);
    let mut output = [0u8; 33];
    // Warm the same execution/GIL path before denying allocations; source and
    // caller buffer already exist. The positive denial control avoids a no-op probe.
    assert_eq!(
        unsafe { (hooks.int_to_bytes)(bits, output.as_mut_ptr(), output.len(), 1, 0) },
        0
    );
    let layout = Layout::from_size_align(17, 1).unwrap();
    let denial = attestation_probe::deny_allocations();
    let refused = unsafe { alloc(layout) };
    drop(denial);
    if !refused.is_null() {
        unsafe { dealloc(refused, layout) };
    }
    assert!(refused.is_null());
    assert_eq!(attestation_probe::denied_allocations(), 1);
    let denial = attestation_probe::deny_allocations();
    let heap_status =
        unsafe { (hooks.int_to_bytes)(bits, output.as_mut_ptr(), output.len(), 1, 0) };
    let mut word = [0u8; 8];
    let word_status = unsafe {
        (hooks.int_to_bytes)(
            molt_obj_model::MoltObject::from_int(-129).bits(),
            word.as_mut_ptr(),
            word.len(),
            0,
            1,
        )
    };
    drop(denial);
    assert_eq!(attestation_probe::denied_allocations(), 0);
    assert_eq!(heap_status, 0);
    assert_eq!(word_status, 0);
    assert_eq!(output, source);
    assert_eq!(word, [255, 255, 255, 255, 255, 255, 255, 127]);
    // The Python method writes its single final bytes allocation directly.
    unsafe extern "C" {
        fn molt_int_to_bytes(value: u64, length: u64, byteorder: u64, signed: u64) -> u64;
    }
    let order = unsafe { hooks.alloc_str(b"little".as_ptr(), 6) };
    let length = molt_obj_model::MoltObject::from_int(33).bits();
    let signed = molt_obj_model::MoltObject::from_bool(false).bits();
    let warm = unsafe { molt_int_to_bytes(bits, length, order, signed) };
    unsafe { (hooks.dec_ref)(warm) };
    assert_no_pending_exception();
    attestation_probe::reset();
    attestation_probe::set_tracking(true);
    let result = unsafe { molt_int_to_bytes(bits, length, order, signed) };
    attestation_probe::set_tracking(false);
    assert_eq!(
        attestation_probe::snapshot().allocations,
        1,
        "only final immutable bytes storage"
    );
    let mut size = 0usize;
    let data = unsafe { (hooks.bytes_data)(result, &raw mut size) };
    assert_eq!(size, source.len());
    assert_eq!(unsafe { std::slice::from_raw_parts(data, size) }, source);
    unsafe { (hooks.dec_ref)(result) };
    let denial = attestation_probe::deny_allocations();
    let refused_result = unsafe { molt_int_to_bytes(bits, length, order, signed) };
    drop(denial);
    assert!(molt_obj_model::MoltObject::from_bits(refused_result).is_none());
    assert_eq!(
        attestation_probe::denied_allocations(),
        1,
        "only final result allocation was attempted"
    );
    assert_eq!(
        unsafe {
            PyErr_ExceptionMatches((&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast())
        },
        1,
        "refusing the final allocation must raise MemoryError"
    );
    unsafe { molt_exception_clear() };
    let recovered = unsafe { molt_int_to_bytes(bits, length, order, signed) };
    assert_no_pending_exception();
    let recovered_data = unsafe { (hooks.bytes_data)(recovered, &raw mut size) };
    assert_eq!(size, source.len());
    assert!(!recovered_data.is_null());
    assert_eq!(
        unsafe { std::slice::from_raw_parts(recovered_data, size) },
        source
    );
    unsafe {
        (hooks.dec_ref)(recovered);
        (hooks.dec_ref)(order);
        (hooks.dec_ref)(bits);
    };
    assert_no_pending_exception();
}
