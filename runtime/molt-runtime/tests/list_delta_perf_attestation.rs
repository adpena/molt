//! Release-only performance and allocation attestation for exact containers.
//!
//! List cases exercise delta publication through a real `PyListObject` projection.
//! Tuple cases cover the canonical empty singleton, packed slot reads, identity
//! fast paths, and exact construction. Allocation observation is deliberately
//! separate from timing: every steady-state operation is a hard zero-allocation,
//! zero-byte, zero-peak-live gate, while list and tuple construction report their
//! unavoidable allocation traffic as positive controls.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::undocumented_unsafe_blocks)]

use molt_cpython_abi::abi_types::{PyListObject, PyObject};
use molt_cpython_abi::api::abstract_sequence::{PySequence_Fast_ITEMS, PySequence_Repeat};
use molt_cpython_abi::api::errors::PyErr_Occurred;
use molt_cpython_abi::api::numbers::PyLong_FromLong;
use molt_cpython_abi::api::refcount::{Py_DECREF, Py_INCREF};
use molt_cpython_abi::api::sequences::{
    PyList_New, PyList_SetItem, PyTuple_GET_ITEM, PyTuple_GET_SIZE, PyTuple_GetItem,
    PyTuple_GetSlice, PyTuple_New, PyTuple_SetItem,
};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::{DecodedHandleResult, hooks};
use molt_cpython_abi::l7_attestation::{
    CALIBRATION_TARGET_NS, MINIMUM_SAMPLE_NS, SAMPLE_COUNT, calibrate_timed_iterations,
    enforce_current_thread_affinity, normalized_affinity_mask, summarize_samples,
};
use molt_obj_model::MoltObject;
use molt_runtime::attestation_probe;
use serde_json::{Value, json};
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
}

struct PublishedList {
    pointer: *mut PyObject,
    bits: u64,
}

struct PublishedTuple {
    pointer: *mut PyObject,
    bits: u64,
}

impl Drop for PublishedTuple {
    fn drop(&mut self) {
        unsafe { Py_DECREF(self.pointer) };
    }
}

impl Drop for PublishedList {
    fn drop(&mut self) {
        unsafe { Py_DECREF(self.pointer) };
    }
}

fn initialize_runtime() {
    unsafe {
        molt_runtime_init();
        molt_exception_clear();
    }
    molt_runtime::cpython_abi_hooks::register_cpython_hooks();
    assert!(hooks().is_some(), "runtime hooks were not registered");
}

fn assert_no_pending_exception() {
    assert!(
        unsafe { PyErr_Occurred() }.is_null(),
        "container attestation left a pending exception"
    );
}

fn handle_bits(pointer: *mut PyObject) -> Option<u64> {
    GLOBAL_BRIDGE
        .molt_handle_for_pyobj(pointer)
        .map(|handle| handle.bits())
}

fn new_scalar(value: i64) -> *mut PyObject {
    let pointer = unsafe { PyLong_FromLong(value as _) };
    assert!(!pointer.is_null(), "failed to materialize scalar {value}");
    pointer
}

fn published_list(items: &[*mut PyObject]) -> PublishedList {
    let pointer = unsafe { PyList_New(items.len() as isize) };
    assert!(!pointer.is_null(), "failed to allocate published list");
    for (index, &item) in items.iter().enumerate() {
        unsafe { Py_INCREF(item) };
        assert_eq!(
            unsafe { PyList_SetItem(pointer, index as isize, item) },
            0,
            "failed to initialize published list slot {index}"
        );
    }
    let bits = handle_bits(pointer).expect("published list has no runtime handle");
    assert_no_pending_exception();
    PublishedList { pointer, bits }
}

fn published_tuple(items: &[*mut PyObject]) -> PublishedTuple {
    let pointer = unsafe { PyTuple_New(items.len() as isize) };
    assert!(!pointer.is_null(), "failed to allocate published tuple");
    for (index, &item) in items.iter().enumerate() {
        unsafe { Py_INCREF(item) };
        assert_eq!(
            unsafe { PyTuple_SetItem(pointer, index as isize, item) },
            0,
            "failed to initialize published tuple slot {index}"
        );
    }
    let bits = handle_bits(pointer).expect("published tuple has no runtime handle");
    assert_no_pending_exception();
    PublishedTuple { pointer, bits }
}

fn physical_len(pointer: *mut PyObject) -> usize {
    unsafe { (*pointer.cast::<PyListObject>()).ob_base.ob_size as usize }
}

fn physical_capacity(pointer: *mut PyObject) -> isize {
    unsafe { (*pointer.cast::<PyListObject>()).allocated }
}

fn physical_item(pointer: *mut PyObject, index: usize) -> *mut PyObject {
    unsafe {
        let list = &*pointer.cast::<PyListObject>();
        if index >= list.ob_base.ob_size as usize || list.ob_item.is_null() {
            return std::ptr::null_mut();
        }
        *list.ob_item.add(index)
    }
}

fn runtime_item(bits: u64, index: usize) -> Option<u64> {
    let result = unsafe { (hooks().expect("runtime hooks").list_item)(bits, index) };
    match result.decode() {
        DecodedHandleResult::Ok(item) => Some(item),
        DecodedHandleResult::Missing | DecodedHandleResult::Error => None,
    }
}

fn runtime_tuple_item(bits: u64, index: usize) -> Option<u64> {
    let result = unsafe { (hooks().expect("runtime hooks").tuple_item)(bits, index) };
    match result.decode() {
        DecodedHandleResult::Ok(item) => Some(item),
        DecodedHandleResult::Missing | DecodedHandleResult::Error => None,
    }
}

fn assert_semantic_batch(iterations: usize, operation: &mut impl FnMut() -> u64) {
    let mut witnesses = 0_u64;
    for _ in 0..iterations {
        witnesses = black_box(witnesses.wrapping_add(operation()));
    }
    assert_eq!(
        witnesses, iterations as u64,
        "container operation failed its semantic witness"
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

fn summary(samples: &[Sample], field: impl Fn(&Sample) -> f64) -> Value {
    let values: Vec<f64> = samples.iter().map(field).collect();
    let summary = summarize_samples(&values);
    json!({
        "median": summary.median,
        "cv": summary.cv,
        "robust_cv": summary.robust_cv,
    })
}

fn measure(
    name: &str,
    family: &str,
    input: Value,
    observer_iterations: usize,
    zero_allocation_gate: bool,
    mut operation: impl FnMut() -> u64,
) -> Value {
    assert_semantic_batch(observer_iterations.clamp(128, 4096), &mut operation);
    let iterations = calibrate_case_iterations(observer_iterations, &mut operation);

    // Consume any one-time allocator, bridge-ledger, or lazy runtime work before
    // taking the gated samples. This remains an observed (but untimed) pass.
    attestation_probe::reset();
    attestation_probe::set_tracking(true);
    assert_semantic_batch(observer_iterations, &mut operation);
    attestation_probe::set_tracking(false);
    attestation_probe::reset();

    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for sample_index in 0..SAMPLE_COUNT {
        let started = Instant::now();
        assert_semantic_batch(iterations, &mut operation);
        let elapsed = started.elapsed().as_nanos() as f64;

        attestation_probe::reset();
        attestation_probe::set_tracking(true);
        assert_semantic_batch(observer_iterations, &mut operation);
        attestation_probe::set_tracking(false);
        let observed = attestation_probe::snapshot();
        if zero_allocation_gate {
            assert_eq!(
                observed.allocations, 0,
                "{name} sample {sample_index} allocated in steady state"
            );
            assert_eq!(
                observed.allocated_bytes, 0,
                "{name} sample {sample_index} allocated bytes in steady state"
            );
            assert_eq!(
                observed.peak_live_bytes, 0,
                "{name} sample {sample_index} raised live bytes in steady state"
            );
        } else {
            assert!(
                observed.allocations > 0,
                "{name} sample {sample_index} did not exercise the allocation probe"
            );
            assert!(
                observed.allocated_bytes > 0,
                "{name} sample {sample_index} did not observe allocated bytes"
            );
            assert!(
                observed.peak_live_bytes > 0,
                "{name} sample {sample_index} did not observe peak live bytes"
            );
        }
        samples.push(Sample {
            ns_per_op: elapsed / iterations as f64,
            allocations_per_op: observed.allocations as f64 / observer_iterations as f64,
            allocated_bytes_per_op: observed.allocated_bytes as f64 / observer_iterations as f64,
            peak_live_bytes: observed.peak_live_bytes,
        });
    }

    json!({
        "name": name,
        "family": family,
        "input": input,
        "iterations_per_sample": iterations,
        "observer_iterations_per_sample": observer_iterations,
        "calibration_target_ns": CALIBRATION_TARGET_NS,
        "minimum_sample_ns": MINIMUM_SAMPLE_NS,
        "timing_scope": "loop_inclusive; allocation observer is untimed",
        "sample_count": SAMPLE_COUNT,
        "gates": {
            "semantic_witness": "pass",
            "steady_state_zero_allocations": {
                "required": zero_allocation_gate,
                "passed": !zero_allocation_gate || samples.iter().all(|sample| {
                    sample.allocations_per_op == 0.0
                        && sample.allocated_bytes_per_op == 0.0
                        && sample.peak_live_bytes == 0
                }),
            },
            "allocator_probe_positive_control": {
                "required": !zero_allocation_gate,
                "passed": zero_allocation_gate || samples.iter().all(|sample| {
                    sample.allocations_per_op > 0.0
                        && sample.allocated_bytes_per_op > 0.0
                        && sample.peak_live_bytes > 0
                }),
            },
        },
        "summary": {
            "ns_per_op": summary(&samples, |sample| sample.ns_per_op),
            "allocations_per_op": summary(&samples, |sample| sample.allocations_per_op),
            "allocated_bytes_per_op": summary(
                &samples,
                |sample| sample.allocated_bytes_per_op,
            ),
            "peak_live_bytes": summary(&samples, |sample| sample.peak_live_bytes as f64),
        },
        "samples": samples.iter().map(|sample| json!({
            "ns_per_op": sample.ns_per_op,
            "allocations_per_op": sample.allocations_per_op,
            "allocated_bytes_per_op": sample.allocated_bytes_per_op,
            "peak_live_bytes": sample.peak_live_bytes,
        })).collect::<Vec<_>>(),
    })
}

fn append_pop_case(item_pointer: *mut PyObject, item_bits: u64) -> Value {
    let list = published_list(&[item_pointer, item_pointer, item_pointer, item_pointer]);
    let list_pointer = list.pointer;
    let list_bits = list.bits;
    let base_len = physical_len(list_pointer);
    let none_bits = MoltObject::none().bits();
    let runtime_hooks = hooks().expect("runtime hooks");
    measure(
        "list.delta.append_pop",
        "published_list_delta",
        json!({
            "base_len": base_len,
            "operation": "RuntimeHooks::list_append + molt_list_pop",
            "identity": "canonical cached-small-int projection preserved",
            "physical_projection": "present_and_verified",
        }),
        4096,
        true,
        || {
            let append_status =
                unsafe { (runtime_hooks.list_append)(list_bits, item_bits, std::ptr::null_mut()) };
            let append_identity = physical_len(list_pointer) == base_len + 1
                && physical_item(list_pointer, base_len) == item_pointer;
            let popped = molt_runtime::molt_list_pop(list_bits, none_bits);
            let valid = append_status == 0
                && append_identity
                && popped == item_bits
                && unsafe { (runtime_hooks.list_len)(list_bits) } == base_len
                && physical_len(list_pointer) == base_len
                && runtime_item(list_bits, base_len - 1) == Some(item_bits)
                && physical_item(list_pointer, base_len - 1) == item_pointer;
            unsafe { (runtime_hooks.dec_ref)(popped) };
            u64::from(black_box(valid))
        },
    )
}

fn indexed_replace_case(
    first_pointer: *mut PyObject,
    first_bits: u64,
    second_pointer: *mut PyObject,
    second_bits: u64,
) -> Value {
    let list = published_list(&[first_pointer, first_pointer, first_pointer, first_pointer]);
    let list_pointer = list.pointer;
    let list_bits = list.bits;
    let index = 2usize;
    let index_bits = MoltObject::from_int(index as i64).bits();
    let mut use_second = false;
    measure(
        "list.delta.indexed_replace",
        "published_list_delta",
        json!({
            "len": 4,
            "index": index,
            "operation": "molt_store_index alternating two canonical scalars",
            "identity": "canonical cached-small-int projection preserved",
            "physical_projection": "present_and_verified",
        }),
        4096,
        true,
        || {
            use_second = !use_second;
            let (expected_pointer, expected_bits) = if use_second {
                (second_pointer, second_bits)
            } else {
                (first_pointer, first_bits)
            };
            let result = molt_runtime::molt_store_index(list_bits, index_bits, expected_bits);
            let valid = result == list_bits
                && runtime_item(list_bits, index) == Some(expected_bits)
                && physical_item(list_pointer, index) == expected_pointer;
            u64::from(black_box(valid))
        },
    )
}

fn reverse_case(
    first_pointer: *mut PyObject,
    first_bits: u64,
    second_pointer: *mut PyObject,
    second_bits: u64,
) -> Value {
    let list = published_list(&[first_pointer, first_pointer, second_pointer, second_pointer]);
    let list_pointer = list.pointer;
    let list_bits = list.bits;
    let mut reversed = false;
    measure(
        "list.delta.reverse",
        "published_list_reorder",
        json!({
            "len": 4,
            "operation": "molt_list_reverse",
            "physical_projection": "present_and_verified",
        }),
        4096,
        true,
        || {
            reversed = !reversed;
            let result = molt_runtime::molt_list_reverse(list_bits);
            let (
                expected_first_pointer,
                expected_first_bits,
                expected_last_pointer,
                expected_last_bits,
            ) = if reversed {
                (second_pointer, second_bits, first_pointer, first_bits)
            } else {
                (first_pointer, first_bits, second_pointer, second_bits)
            };
            let valid = result == MoltObject::none().bits()
                && runtime_item(list_bits, 0) == Some(expected_first_bits)
                && runtime_item(list_bits, 3) == Some(expected_last_bits)
                && physical_item(list_pointer, 0) == expected_first_pointer
                && physical_item(list_pointer, 3) == expected_last_pointer;
            u64::from(black_box(valid))
        },
    )
}

fn presized_construction_case(item_pointer: *mut PyObject, item_bits: u64) -> Value {
    const LEN: usize = 64;
    measure(
        "list.construction.pylist_new_presized",
        "published_list_construction",
        json!({
            "len": LEN,
            "operation": "PyList_New + indexed initialization + Py_DECREF",
            "allocation_gate": "positive_control_required",
        }),
        128,
        false,
        || {
            let list_pointer = unsafe { PyList_New(LEN as isize) };
            if list_pointer.is_null() {
                return 0;
            }
            for index in 0..LEN {
                unsafe { Py_INCREF(item_pointer) };
                if unsafe { PyList_SetItem(list_pointer, index as isize, item_pointer) } != 0 {
                    unsafe { Py_DECREF(list_pointer) };
                    return 0;
                }
            }
            let Some(list_bits) = handle_bits(list_pointer) else {
                unsafe { Py_DECREF(list_pointer) };
                return 0;
            };
            let runtime_hooks = hooks().expect("runtime hooks");
            let valid = unsafe { (runtime_hooks.list_len)(list_bits) } == LEN
                && physical_len(list_pointer) == LEN
                && physical_capacity(list_pointer) == LEN as isize
                && runtime_item(list_bits, 0) == Some(item_bits)
                && runtime_item(list_bits, LEN - 1) == Some(item_bits)
                && physical_item(list_pointer, 0) == item_pointer
                && physical_item(list_pointer, LEN - 1) == item_pointer;
            unsafe { Py_DECREF(list_pointer) };
            u64::from(black_box(valid))
        },
    )
}

fn empty_tuple_steady_state_case() -> Value {
    let empty_pointer = unsafe { PyTuple_New(0) };
    assert!(
        !empty_pointer.is_null(),
        "failed to materialize empty tuple singleton"
    );
    let empty_bits = handle_bits(empty_pointer).expect("empty tuple has no runtime handle");
    let runtime_hooks = hooks().expect("runtime hooks");
    let result = measure(
        "tuple.steady.empty_singleton",
        "published_tuple_steady_state",
        json!({
            "len": 0,
            "operation": "PyTuple_New(0) + identity/length checks + Py_DECREF",
            "identity": "canonical immortal empty tuple",
            "physical_projection": "present_and_verified",
        }),
        4096,
        true,
        || {
            let candidate = unsafe { PyTuple_New(0) };
            if candidate.is_null() {
                return 0;
            }
            let valid = candidate == empty_pointer
                && handle_bits(candidate) == Some(empty_bits)
                && unsafe { PyTuple_GET_SIZE(candidate) } == 0
                && unsafe { (runtime_hooks.tuple_len)(empty_bits) } == 0;
            unsafe { Py_DECREF(candidate) };
            u64::from(black_box(valid))
        },
    );
    unsafe { Py_DECREF(empty_pointer) };
    result
}

fn tuple_read_case(item_pointer: *mut PyObject, item_bits: u64) -> Value {
    const LEN: usize = 8;
    const INDEX: usize = 5;
    let tuple = published_tuple(&[item_pointer; LEN]);
    let tuple_pointer = tuple.pointer;
    let tuple_bits = tuple.bits;
    measure(
        "tuple.steady.checked_raw_fast_items",
        "published_tuple_read",
        json!({
            "len": LEN,
            "index": INDEX,
            "operation": "PyTuple_GetItem + PyTuple_GET_ITEM + PySequence_Fast_ITEMS",
            "identity": "all read lanes return the packed physical slot",
            "allocation_gate": "exact_zero_required",
        }),
        4096,
        true,
        || {
            let checked = unsafe { PyTuple_GetItem(tuple_pointer, INDEX as isize) };
            let raw = unsafe { PyTuple_GET_ITEM(tuple_pointer, INDEX as isize) };
            let fast = unsafe { PySequence_Fast_ITEMS(tuple_pointer) };
            let fast_item = if fast.is_null() {
                std::ptr::null_mut()
            } else {
                unsafe { *fast.add(INDEX) }
            };
            let valid = checked == item_pointer
                && raw == item_pointer
                && fast_item == item_pointer
                && unsafe { PyTuple_GET_SIZE(tuple_pointer) } == LEN as isize
                && runtime_tuple_item(tuple_bits, INDEX) == Some(item_bits);
            u64::from(black_box(valid))
        },
    )
}

fn tuple_identity_fast_paths_case(item_pointer: *mut PyObject) -> Value {
    const LEN: usize = 8;
    let tuple = published_tuple(&[item_pointer; LEN]);
    let tuple_pointer = tuple.pointer;
    measure(
        "tuple.steady.full_slice_repeat_one_identity",
        "published_tuple_identity",
        json!({
            "len": LEN,
            "operation": "PyTuple_GetSlice(full) + PySequence_Repeat(1)",
            "identity": "both return a new reference to the exact same tuple",
            "allocation_gate": "exact_zero_required",
        }),
        4096,
        true,
        || {
            let full_slice = unsafe { PyTuple_GetSlice(tuple_pointer, 0, LEN as isize) };
            if full_slice.is_null() {
                return 0;
            }
            let repeated_once = unsafe { PySequence_Repeat(tuple_pointer, 1) };
            if repeated_once.is_null() {
                unsafe { Py_DECREF(full_slice) };
                return 0;
            }
            let valid = full_slice == tuple_pointer
                && repeated_once == tuple_pointer
                && unsafe { PyTuple_GET_ITEM(full_slice, 0) } == item_pointer
                && unsafe { PyTuple_GET_ITEM(repeated_once, LEN as isize - 1) } == item_pointer;
            unsafe {
                Py_DECREF(repeated_once);
                Py_DECREF(full_slice);
            }
            u64::from(black_box(valid))
        },
    )
}

fn tuple_construction_case(item_pointer: *mut PyObject, item_bits: u64) -> Value {
    const LEN: usize = 64;
    measure(
        "tuple.construction.pytuple_new_fill",
        "published_tuple_construction",
        json!({
            "len": LEN,
            "operation": "PyTuple_New + fixed-slot initialization + Py_DECREF",
            "allocation_gate": "positive_control_required",
            "identity": "non-small exact scalar pointer preserved in every packed slot",
        }),
        128,
        false,
        || {
            let tuple_pointer = unsafe { PyTuple_New(LEN as isize) };
            if tuple_pointer.is_null() {
                return 0;
            }
            for index in 0..LEN {
                unsafe { Py_INCREF(item_pointer) };
                if unsafe { PyTuple_SetItem(tuple_pointer, index as isize, item_pointer) } != 0 {
                    unsafe { Py_DECREF(tuple_pointer) };
                    return 0;
                }
            }
            let Some(tuple_bits) = handle_bits(tuple_pointer) else {
                unsafe { Py_DECREF(tuple_pointer) };
                return 0;
            };
            let fast_items = unsafe { PySequence_Fast_ITEMS(tuple_pointer) };
            let valid = unsafe { PyTuple_GET_SIZE(tuple_pointer) } == LEN as isize
                && unsafe { (hooks().expect("runtime hooks").tuple_len)(tuple_bits) } == LEN
                && runtime_tuple_item(tuple_bits, 0) == Some(item_bits)
                && runtime_tuple_item(tuple_bits, LEN - 1) == Some(item_bits)
                && unsafe { PyTuple_GetItem(tuple_pointer, 0) } == item_pointer
                && !fast_items.is_null()
                && unsafe { *fast_items.add(LEN - 1) } == item_pointer;
            unsafe { Py_DECREF(tuple_pointer) };
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
#[ignore = "release profiler; use tools/bench/run_list_delta_attestation.py"]
fn sequence_container_performance_attestation() {
    assert!(
        !cfg!(debug_assertions),
        "sequence container performance attestation is release-only"
    );
    let affinity_mask = enforce_current_thread_affinity(&required_env("MOLT_L7_AFFINITY_MASK"));
    initialize_runtime();

    let first_pointer = new_scalar(101);
    let second_pointer = new_scalar(202);
    let tuple_pointer = new_scalar(1000);
    let first_bits = handle_bits(first_pointer).expect("first scalar has no runtime handle");
    let second_bits = handle_bits(second_pointer).expect("second scalar has no runtime handle");
    let tuple_bits = handle_bits(tuple_pointer).expect("tuple scalar has no runtime handle");

    let cases = vec![
        append_pop_case(first_pointer, first_bits),
        indexed_replace_case(first_pointer, first_bits, second_pointer, second_bits),
        reverse_case(first_pointer, first_bits, second_pointer, second_bits),
        presized_construction_case(first_pointer, first_bits),
        empty_tuple_steady_state_case(),
        tuple_read_case(tuple_pointer, tuple_bits),
        tuple_identity_fast_paths_case(tuple_pointer),
        tuple_construction_case(tuple_pointer, tuple_bits),
    ];
    unsafe {
        Py_DECREF(tuple_pointer);
        Py_DECREF(second_pointer);
        Py_DECREF(first_pointer);
    }
    assert_no_pending_exception();

    let payload = json!({
        "schema_version": 2,
        "kind": "sequence_container_performance_attestation",
        "profile": "release",
        "allocator_scope": "test_feature_counting_wrapper_over_production_mimalloc",
        "sample_count": SAMPLE_COUNT,
        "execution_mode": {
            "deterministic_default": true,
            "runtime_gil": "enabled",
            "free_threaded": false,
            "benchmark_threads": 1,
        },
        "scope": {
            "native": true,
            "wasm32": false,
            "assembly": false,
            "code_size": false,
            "process_tree_peak_rss": true,
            "windows_job_peak_commit": true,
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
            "append_pop": "warm runtime delta publication with a live CPython PyListObject view",
            "indexed_replace": "warm single-slot runtime and physical projection publication",
            "reverse": "allocation-free complete reorder of runtime and physical projections",
            "construction": "exact PyList_New presizing, indexed initialization, and destruction",
            "empty_tuple": "warm canonical PyTuple_New(0) identity and exact zero-allocation steady state",
            "tuple_reads": "checked, raw, and PySequence_Fast_ITEMS reads share one packed physical slot",
            "tuple_identity": "full slicing and repeat-one return new references to the exact tuple",
            "tuple_construction": "exact PyTuple_New sizing, fixed-slot identity publication, and destruction",
            "semantic_witness": "every operation verifies runtime and physical views and leaves no pending exception",
            "allocation_gate": "every steady-state list/tuple sample requires exactly zero allocations, allocated bytes, and peak live bytes; both construction families are positive controls",
            "process_tree_memory": "peak RSS and Windows Job peak commit are added by tools/bench/run_list_delta_attestation.py",
        },
        "cases": cases,
    });
    println!("SEQUENCE_CONTAINER_ATTESTATION={payload}");
}
