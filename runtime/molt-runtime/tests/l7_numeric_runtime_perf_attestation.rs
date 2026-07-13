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

use molt_cpython_abi::api::errors::PyErr_Occurred;
use molt_cpython_abi::api::numbers::{
    _PyLong_AsByteArray, _PyLong_FromByteArray, _PyLong_NumBits, PyLong_FromString,
};
use molt_cpython_abi::api::refcount::Py_DECREF;
use molt_cpython_abi::l7_attestation::{
    CALIBRATION_TARGET_NS, MINIMUM_SAMPLE_NS, SAMPLE_COUNT, calibrate_timed_iterations,
    enforce_current_thread_affinity, normalized_affinity_mask,
};
use molt_obj_model::MoltObject;
use molt_runtime::attestation_probe;
use num_bigint::BigUint;
use serde_json::{Value, json};
use std::ffi::CString;
use std::hint::black_box;
use std::time::Instant;

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    MoltObject::none().bits()
}

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

fn measure(
    name: &str,
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
        "family": "runtime_bigint",
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
    let mut values: Vec<f64> = samples.iter().map(field).collect();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    values.sort_by(f64::total_cmp);
    json!({
        "median": values[values.len() / 2],
        "cv": if mean == 0.0 { 0.0 } else { variance.sqrt() / mean },
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
    initialize_runtime();
    let mut cases = Vec::new();
    for digits in [25, 37, 256, 4096, 4300] {
        cases.push(decimal_case(digits));
    }
    for width in [1, 2, 4, 8, 17, 256, 4096] {
        cases.push(byte_case(width));
    }
    let payload = json!({
        "schema_version": 2,
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
