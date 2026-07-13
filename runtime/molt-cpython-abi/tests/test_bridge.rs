//! Tests for the ObjectBridge: handle ↔ PyObject translation, tag table,
//! singleton handling, and the global bridge init function.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::{f64::consts::PI, hint::black_box, ptr, sync::Arc, thread, time::Instant};

use molt_lang_obj_model::MoltObject;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

#[test]
#[ignore = "wall-clock profiler; run with --ignored --nocapture --release"]
fn bridge_crossing_timing_profile() {
    init();
    const LOOKUPS_PER_THREAD: usize = 2_000_000;
    const GLOBAL_MUTEX_BASELINE_NS: f64 = 18.72;
    let max_threads = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(16);
    let mut thread_counts = vec![1usize];
    while thread_counts.last().copied().unwrap_or(1) < max_threads {
        thread_counts.push((thread_counts.last().copied().unwrap_or(1) * 2).min(max_threads));
    }
    thread_counts.dedup();

    println!("\n=== bridge identity crossing profile ===");
    let mut one_thread_throughput = None;
    let mut four_thread_throughput = None;
    for thread_count in thread_counts {
        let pointers: Arc<Vec<usize>> = Arc::new(
            (0..thread_count)
                .map(|thread_index| {
                    let bits = MoltObject::from_int(10_000 + thread_index as i64).bits();
                    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }.expose_provenance()
                })
                .collect(),
        );
        let started = Instant::now();
        let workers: Vec<_> = (0..thread_count)
            .map(|thread_index| {
                let pointers = Arc::clone(&pointers);
                thread::spawn(move || {
                    let ptr =
                        core::ptr::with_exposed_provenance_mut::<PyObject>(pointers[thread_index]);
                    for _ in 0..LOOKUPS_PER_THREAD {
                        black_box(GLOBAL_BRIDGE.pyobj_to_handle(black_box(ptr)));
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("bridge profiler worker panicked");
        }
        let elapsed = started.elapsed();
        let total = LOOKUPS_PER_THREAD * thread_count;
        let throughput = total as f64 / elapsed.as_secs_f64() / 1_000_000.0;
        println!(
            "threads={thread_count:>2} total={total:>9} elapsed_ms={:>9.3} ns/crossing={:>8.2} throughput_mops={:>8.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_nanos() as f64 / total as f64,
            throughput,
        );
        if thread_count == 1 {
            one_thread_throughput = Some(throughput);
            let sharded_ns = elapsed.as_nanos() as f64 / LOOKUPS_PER_THREAD as f64;
            println!(
                "single-thread gate: sharded={sharded_ns:.2} ns measured-global-baseline={GLOBAL_MUTEX_BASELINE_NS:.2} ns delta={:+.2}%",
                (sharded_ns / GLOBAL_MUTEX_BASELINE_NS - 1.0) * 100.0
            );
            assert!(
                sharded_ns <= GLOBAL_MUTEX_BASELINE_NS,
                "sharded bridge regressed uncontended crossing: {sharded_ns:.2} ns > measured baseline {GLOBAL_MUTEX_BASELINE_NS:.2} ns"
            );
        }
        if thread_count == 4 {
            four_thread_throughput = Some(throughput);
        }
        for addr in pointers.iter().copied() {
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(ptr) };
        }
    }
    if let (Some(one), Some(four)) = (one_thread_throughput, four_thread_throughput) {
        assert!(
            four >= one * 1.5,
            "four-thread bridge throughput did not scale: {four:.2} Mops/s vs {one:.2} Mops/s"
        );
    }
}

// ---------------------------------------------------------------------------
// handle_to_pyobj: primitives
// ---------------------------------------------------------------------------

#[test]
fn test_bridge_int_roundtrip() {
    init();
    let bits = MoltObject::from_int(42).bits();
    let py = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    assert!(!py.is_null());

    let recovered = GLOBAL_BRIDGE.pyobj_to_handle(py);
    assert_eq!(recovered.map(|identity| identity.as_handle()), Some(bits));

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_bridge_float_roundtrip() {
    init();
    let bits = MoltObject::from_float(PI).bits();
    let py = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    assert!(!py.is_null());

    let recovered = GLOBAL_BRIDGE.pyobj_to_handle(py);
    assert_eq!(recovered.map(|identity| identity.as_handle()), Some(bits));

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// handle_to_pyobj: singletons
// ---------------------------------------------------------------------------

#[test]
fn test_bridge_none_returns_singleton() {
    init();
    let bits = MoltObject::none().bits();
    let py = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    assert!(std::ptr::eq(py, &raw mut Py_None));
}

#[test]
fn test_bridge_true_returns_singleton() {
    init();
    let bits = MoltObject::from_bool(true).bits();
    let py = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    assert!(std::ptr::eq(py, (&raw mut Py_True).cast::<PyObject>()));
}

#[test]
fn test_bridge_false_returns_singleton() {
    init();
    let bits = MoltObject::from_bool(false).bits();
    let py = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    assert!(std::ptr::eq(py, (&raw mut Py_False).cast::<PyObject>()));
}

// ---------------------------------------------------------------------------
// pyobj_to_handle: singletons
// ---------------------------------------------------------------------------

#[test]
fn test_pyobj_to_handle_none() {
    init();
    let none_ptr = &raw mut Py_None;
    let handle = GLOBAL_BRIDGE.pyobj_to_handle(none_ptr);
    assert_eq!(
        handle.map(|identity| identity.as_handle()),
        Some(MoltObject::none().bits())
    );
}

#[test]
fn test_pyobj_to_handle_true() {
    init();
    let true_ptr = (&raw mut Py_True).cast::<PyObject>();
    let handle = GLOBAL_BRIDGE.pyobj_to_handle(true_ptr);
    assert_eq!(
        handle.map(|identity| identity.as_handle()),
        Some(MoltObject::from_bool(true).bits())
    );
}

#[test]
fn test_pyobj_to_handle_false() {
    init();
    let false_ptr = (&raw mut Py_False).cast::<PyObject>();
    let handle = GLOBAL_BRIDGE.pyobj_to_handle(false_ptr);
    assert_eq!(
        handle.map(|identity| identity.as_handle()),
        Some(MoltObject::from_bool(false).bits())
    );
}

#[test]
fn test_pyobj_to_handle_null_returns_none() {
    init();
    let handle = GLOBAL_BRIDGE.pyobj_to_handle(ptr::null_mut());
    assert_eq!(handle, None);
}

#[test]
fn test_none_hash_uses_pointer_width_py_hash() {
    init();
    let none = molt_cpython_abi::bridge::bits_to_pyobject(MoltObject::none().bits());
    let hash = molt_cpython_abi::bridge::molt_bridge_hash(none);
    let expected = if std::mem::size_of::<isize>() >= 8 {
        0x0FCA_86420_u64 as isize
    } else {
        0x0FCA_86420_u32 as i32 as isize
    };
    assert_eq!(hash, expected);
}

// ---------------------------------------------------------------------------
// handle_to_pyobj: caching (second call should incref, not allocate)
// ---------------------------------------------------------------------------

#[test]
fn test_bridge_caches_second_lookup() {
    init();
    let bits = MoltObject::from_int(12345).bits();
    let py1 = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    let py2 = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };

    // Same pointer should be returned (cached)
    assert_eq!(py1, py2);
    // Refcount should be 2 now (initial 1 + cache hit incref)
    assert_eq!(unsafe { (*py1).ob_refcnt }, 2);

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py1);
        molt_cpython_abi::api::refcount::Py_DECREF(py2);
    }
}

#[test]
fn test_bridge_borrowed_lookup_does_not_incref_cached_entry() {
    init();
    let bits = MoltObject::from_int(54321).bits();
    let py = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    assert_eq!(unsafe { (*py).ob_refcnt }, 1);

    let borrowed = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
    assert_eq!(borrowed, py);
    assert_eq!(unsafe { (*py).ob_refcnt }, 1);

    unsafe {
        molt_cpython_abi::api::refcount::Py_INCREF(borrowed);
    }
    assert_eq!(unsafe { (*py).ob_refcnt }, 2);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(borrowed);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_bridge_borrowed_lookup_materializes_cache_anchor() {
    init();
    let bits = MoltObject::from_int(54322).bits();
    let py = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
    assert!(!py.is_null());
    assert_eq!(unsafe { (*py).ob_refcnt }, 1);
    assert_eq!(
        GLOBAL_BRIDGE
            .pyobj_to_handle(py)
            .map(|identity| identity.as_handle()),
        Some(bits)
    );
    assert!(GLOBAL_BRIDGE.release_pyobj(py));
}

// ---------------------------------------------------------------------------
// release_pyobj removes mapping
// ---------------------------------------------------------------------------

#[test]
fn test_release_pyobj_removes_mapping() {
    init();
    let bits = MoltObject::from_int(77777).bits();
    let py = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    assert!(GLOBAL_BRIDGE.pyobj_to_handle(py).is_some());

    assert!(GLOBAL_BRIDGE.release_pyobj(py));
    assert!(GLOBAL_BRIDGE.pyobj_to_handle(py).is_none());
}

// ---------------------------------------------------------------------------
// tag_to_type
// ---------------------------------------------------------------------------

#[test]
fn tag_table_uses_physical_types_only_for_exact_layout_carriers() {
    init();
    let managed = &raw mut MoltManaged_Type;
    let expected = [
        (MoltTypeTag::Bool, &raw mut PyBool_Type),
        (MoltTypeTag::Int, managed),
        (MoltTypeTag::Float, managed),
        (MoltTypeTag::Complex, managed),
        (MoltTypeTag::Str, managed),
        (MoltTypeTag::Bytes, managed),
        (MoltTypeTag::List, managed),
        (MoltTypeTag::Tuple, &raw mut PyTuple_Type),
        (MoltTypeTag::Dict, managed),
        (MoltTypeTag::Set, managed),
        (MoltTypeTag::FrozenSet, managed),
        (MoltTypeTag::Type, &raw mut PyType_Type),
        (MoltTypeTag::Module, managed),
        (MoltTypeTag::Traceback, managed),
        (MoltTypeTag::Exception, managed),
        (MoltTypeTag::Other, managed),
    ];

    for (tag, expected_type) in expected {
        let actual = unsafe { molt_cpython_abi::bridge::tag_to_type(tag) };
        assert!(
            std::ptr::eq(actual, expected_type),
            "wrong physical carrier for {tag:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// molt_cpython_abi_init is idempotent
// ---------------------------------------------------------------------------

#[test]
fn test_init_idempotent() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    // Should not panic or corrupt state
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Int) };
    assert!(!tp.is_null());
}
