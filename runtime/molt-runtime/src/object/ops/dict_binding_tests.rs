use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
use std::sync::atomic::{AtomicU64, Ordering};

const PROBE_UNEQUAL: u64 = 0;
const PROBE_RAISE: u64 = 1;
const PROBE_RESTRUCTURE: u64 = 2;
const PROBE_EQUAL: u64 = 3;

static PROBE_MODE: AtomicU64 = AtomicU64::new(PROBE_UNEQUAL);
static PROBE_CALLS: AtomicU64 = AtomicU64::new(0);
static PROBE_DICT: AtomicU64 = AtomicU64::new(0);
static PROBE_DELETE: AtomicU64 = AtomicU64::new(0);
static PROBE_INSERT: AtomicU64 = AtomicU64::new(0);
static OBSERVED_DICT: AtomicU64 = AtomicU64::new(0);
static OBSERVED_VALUE: AtomicU64 = AtomicU64::new(0);
static OBSERVED_COMMITTED: AtomicU64 = AtomicU64::new(0);
static FINALIZER_CALLS: AtomicU64 = AtomicU64::new(0);

/// `__eq__` of a key stored under the hash of a bound `str`: ordinary lookup of
/// that `str` must consult it, and it may raise or restructure the mapping.
extern "C" fn same_hash_key_eq(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let call = PROBE_CALLS.fetch_add(1, Ordering::SeqCst);
        match PROBE_MODE.load(Ordering::SeqCst) {
            PROBE_EQUAL => MoltObject::from_bool(true).bits(),
            PROBE_RAISE => raise_exception::<u64>(py, "RuntimeError", "probe equality failure"),
            PROBE_RESTRUCTURE if call == 0 => {
                let dict = obj_from_bits(PROBE_DICT.load(Ordering::SeqCst))
                    .as_ptr()
                    .unwrap();
                unsafe {
                    // Shift every later entry, then bind a key the transition
                    // already probed as absent.
                    assert!(dict_del_in_place(
                        py,
                        dict,
                        PROBE_DELETE.load(Ordering::SeqCst)
                    ));
                    dict_set_in_place(
                        py,
                        dict,
                        PROBE_INSERT.load(Ordering::SeqCst),
                        MoltObject::from_int(40).bits(),
                    );
                }
                MoltObject::from_bool(false).bits()
            }
            _ => MoltObject::from_bool(false).bits(),
        }
    })
}

/// Finalizer of a displaced value: it must observe the complete transition.
extern "C" fn observe_committed_bindings(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        FINALIZER_CALLS.fetch_add(1, Ordering::SeqCst);
        let dict = obj_from_bits(OBSERVED_DICT.load(Ordering::SeqCst))
            .as_ptr()
            .unwrap();
        let value = OBSERVED_VALUE.load(Ordering::SeqCst);
        let mut committed = true;
        for name in [b"alpha".as_slice(), b"beta".as_slice()] {
            committed &= unsafe { dict_get_str_bytes_borrowed(py, dict, name) } == Some(value);
        }
        OBSERVED_COMMITTED.store(u64::from(committed), Ordering::SeqCst);
        MoltObject::none().bits()
    })
}

struct TrackerReset;

impl Drop for TrackerReset {
    fn drop(&mut self) {
        set_tracker(Box::new(UnlimitedTracker));
    }
}

/// A distinct mortal exact `str`, so key-object identity stays observable.
fn string_key(py: &PyToken<'_>, name: &[u8]) -> u64 {
    let ptr = crate::object::builders::alloc_string_nointern(py, name);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn mortal_value(py: &PyToken<'_>) -> u64 {
    let ptr = alloc_list(py, &[]);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn owners(bits: u64) -> u32 {
    unsafe { (*header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot() }
}

/// An instance of a fresh class whose `method` is the runtime callable `target`.
fn instance_with_method(
    py: &PyToken<'_>,
    class_name: &[u8],
    method: &[u8],
    symbol: &str,
    target: *const (),
    arity: u64,
) -> (u64, u64, u64) {
    let name = attr_name_bits_from_bytes(py, class_name).unwrap();
    let class = crate::molt_class_new(name);
    crate::molt_class_set_base(class, builtin_classes(py).object);
    let method_name = attr_name_bits_from_bytes(py, method).unwrap();
    let function = alloc_runtime_function_obj(py, runtime_fn_addr(symbol, target), arity);
    assert!(!function.is_null());
    let function = MoltObject::from_ptr(function).bits();
    crate::molt_set_attr_name(class, method_name, function);
    let class_ptr = obj_from_bits(class).as_ptr().unwrap();
    unsafe {
        crate::object::class_finish_definition(py, class_ptr).unwrap();
    }
    let size = unsafe { crate::object::layout::class_cached_layout_size(class_ptr).unwrap() };
    let instance = crate::object::builders::alloc_class_instance(py, size, class);
    unsafe {
        crate::object::gc::gc_publish_initialized(py, obj_from_bits(instance).as_ptr().unwrap());
    }
    dec_ref_bits(py, method_name);
    dec_ref_bits(py, name);
    assert!(!exception_pending(py));
    (instance, class, function)
}

fn same_hash_probe_key(py: &PyToken<'_>) -> (u64, u64, u64) {
    instance_with_method(
        py,
        b"SameHashProbeKey",
        b"__eq__",
        "same_hash_key_eq",
        same_hash_key_eq as *const (),
        2,
    )
}

/// A dictionary of `pairs` that then holds `probe` under the exact `str` hash
/// of `name`, as a user key of another kind with a colliding hash would be.
unsafe fn dict_with_same_hash_key(
    py: &PyToken<'_>,
    pairs: &[u64],
    probe: u64,
    name: &[u8],
) -> *mut u8 {
    let dict = alloc_dict_with_pairs(py, pairs);
    assert!(!dict.is_null());
    PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
    unsafe {
        dict_set_with_hash_in_place(
            py,
            dict,
            probe,
            MoltObject::from_int(7).bits(),
            hash_string_bytes(py, name) as u64,
        );
    }
    assert!(!exception_pending(py));
    dict
}

fn take_pending_error(py: &PyToken<'_>, expected: &str) {
    assert!(exception_pending(py));
    let error = crate::builtins::exceptions::molt_exception_last_pending();
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        py, error, expected
    ));
    crate::clear_exception(py);
    dec_ref_bits(py, error);
}

#[test]
fn string_binding_commits_every_key_as_one_transition() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let none = MoltObject::none().bits();
            let omega = string_key(py, b"omega");
            let alpha = string_key(py, b"alpha");
            let alpha_again = string_key(py, b"alpha");
            let beta = string_key(py, b"beta");
            let gamma = string_key(py, b"gamma");
            let old = mortal_value(py);
            let value = mortal_value(py);
            let dict = alloc_dict_with_pairs(py, &[omega, none, alpha, old]);
            assert!(!dict.is_null());
            let epoch = dict_structural_epoch(dict);
            let (old_owners, value_owners) = (owners(old), owners(value));
            let displaced = dict_bind_string_entries(
                py,
                dict,
                &[(alpha_again, value), (beta, value), (gamma, value)],
            )
            .expect("binding commits");
            // Every binding is visible before any displaced owner is released,
            // and the present key keeps its original key object.
            assert_eq!(
                dict_live_entries(dict)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>()
                    .as_slice(),
                &[omega, none, alpha, value, beta, value, gamma, value]
            );
            assert_ne!(dict_structural_epoch(dict), epoch);
            assert_eq!(
                owners(old),
                old_owners,
                "displaced owner outlives the commit"
            );
            assert_eq!(owners(value), value_owners + 3);
            drop(displaced);
            assert_eq!(owners(old), old_owners - 1);
            for key in [alpha, alpha_again, beta, gamma] {
                assert_eq!(dict_get_in_place(py, dict, key), Some(value));
            }
            for bits in [
                MoltObject::from_ptr(dict).bits(),
                omega,
                alpha,
                alpha_again,
                beta,
                gamma,
                old,
                value,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn string_binding_failure_leaves_every_binding_unchanged() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let present = string_key(py, b"present");
            let first = string_key(py, b"first_absent");
            let second = string_key(py, b"second_absent");
            let old = mortal_value(py);
            let value = mortal_value(py);
            let dict = alloc_dict_with_pairs(py, &[present, old]);
            assert!(!dict.is_null());
            // Exhaust entry storage: binding two new keys must grow it.
            let mut filler = 0;
            while dict_entries(dict).capacity() - dict_entries(dict).len() >= 2 {
                dict_set_in_place(
                    py,
                    dict,
                    MoltObject::from_int(filler).bits(),
                    MoltObject::none().bits(),
                );
                filler += 1;
            }
            let order = dict_live_entries(dict)
                .flat_map(|row| [row.key, row.value])
                .collect::<Vec<_>>();
            let epoch = dict_structural_epoch(dict);
            let owners_before = (owners(old), owners(value));
            let entries = [(present, value), (first, value), (second, value)];
            let reset = TrackerReset;
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            let denied = dict_bind_string_entries(py, dict, &entries).is_err();
            drop(reset);
            assert!(denied);
            assert!(exception_pending(py));
            let _ = crate::molt_exception_clear();
            // The present key was probed and its replacement planned, yet no
            // binding, order, key or owner changed.
            assert_eq!(
                dict_live_entries(dict)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>()
                    .as_slice(),
                order.as_slice()
            );
            assert_eq!(dict_structural_epoch(dict), epoch);
            assert_eq!((owners(old), owners(value)), owners_before);
            // The identical transition commits once storage may grow.
            drop(dict_bind_string_entries(py, dict, &entries).expect("binding commits"));
            for key in [present, first, second] {
                assert_eq!(dict_get_in_place(py, dict, key), Some(value));
            }
            for bits in [
                MoltObject::from_ptr(dict).bits(),
                present,
                first,
                second,
                old,
                value,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn probe_failure_leaves_every_binding_unchanged() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (probe, class, function) = same_hash_probe_key(py);
            let alpha = string_key(py, b"alpha");
            let beta = string_key(py, b"beta");
            let value = mortal_value(py);
            let dict = dict_with_same_hash_key(py, &[], probe, b"beta");
            let order = dict_live_entries(dict)
                .flat_map(|row| [row.key, row.value])
                .collect::<Vec<_>>();
            let epoch = dict_structural_epoch(dict);
            let value_owners = owners(value);
            PROBE_CALLS.store(0, Ordering::SeqCst);
            PROBE_MODE.store(PROBE_RAISE, Ordering::SeqCst);
            // "alpha" is probed absent first; the failing equality while
            // probing "beta" must still leave it unwritten.
            let failed =
                dict_bind_string_entries(py, dict, &[(alpha, value), (beta, value)]).is_err();
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            assert!(failed);
            assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
            take_pending_error(py, "RuntimeError");
            assert_eq!(
                dict_live_entries(dict)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>()
                    .as_slice(),
                order.as_slice()
            );
            assert_eq!(dict_structural_epoch(dict), epoch);
            assert_eq!(owners(value), value_owners);
            // Ordinary equality decides both keys once it succeeds.
            drop(
                dict_bind_string_entries(py, dict, &[(alpha, value), (beta, value)])
                    .expect("binding commits"),
            );
            assert_eq!(
                dict_live_entries(dict)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>()
                    .as_slice(),
                &[
                    probe,
                    MoltObject::from_int(7).bits(),
                    alpha,
                    value,
                    beta,
                    value
                ]
            );
            for bits in [
                MoltObject::from_ptr(dict).bits(),
                alpha,
                beta,
                value,
                probe,
                class,
                function,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn probe_restructuring_restarts_against_the_live_table() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (probe, class, function) = same_hash_probe_key(py);
            let delta = string_key(py, b"delta");
            let gamma = string_key(py, b"gamma");
            let gamma_again = string_key(py, b"gamma");
            let alpha = string_key(py, b"alpha");
            let alpha_from_callback = string_key(py, b"alpha");
            let beta = string_key(py, b"beta");
            let value = mortal_value(py);
            let dict = dict_with_same_hash_key(
                py,
                &[
                    delta,
                    MoltObject::from_int(1).bits(),
                    gamma,
                    MoltObject::from_int(2).bits(),
                ],
                probe,
                b"beta",
            );
            PROBE_DICT.store(MoltObject::from_ptr(dict).bits(), Ordering::SeqCst);
            PROBE_DELETE.store(delta, Ordering::SeqCst);
            PROBE_INSERT.store(alpha_from_callback, Ordering::SeqCst);
            PROBE_CALLS.store(0, Ordering::SeqCst);
            PROBE_MODE.store(PROBE_RESTRUCTURE, Ordering::SeqCst);
            // "gamma" is found and "alpha" probed absent before the first
            // "beta" probe deletes "delta" and binds "alpha". Results from that
            // pass would overwrite the probe key's value and duplicate "alpha".
            let displaced = dict_bind_string_entries(
                py,
                dict,
                &[(gamma_again, value), (alpha, value), (beta, value)],
            )
            .expect("binding commits");
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            drop(displaced);
            assert!(
                PROBE_CALLS.load(Ordering::SeqCst) >= 1,
                "equality was consulted"
            );
            assert_eq!(
                dict_live_entries(dict)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>()
                    .as_slice(),
                &[
                    gamma,
                    value,
                    probe,
                    MoltObject::from_int(7).bits(),
                    alpha_from_callback,
                    value,
                    beta,
                    value
                ]
            );
            PROBE_DICT.store(0, Ordering::SeqCst);
            PROBE_DELETE.store(0, Ordering::SeqCst);
            PROBE_INSERT.store(0, Ordering::SeqCst);
            for bits in [
                MoltObject::from_ptr(dict).bits(),
                delta,
                gamma,
                gamma_again,
                alpha,
                alpha_from_callback,
                beta,
                value,
                probe,
                class,
                function,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn displaced_values_release_after_the_whole_transition() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (finalizing, class, function) = instance_with_method(
                py,
                b"CommittedBindingObserver",
                b"__del__",
                "observe_committed_bindings",
                observe_committed_bindings as *const (),
                1,
            );
            let alpha = string_key(py, b"alpha");
            let beta = string_key(py, b"beta");
            let value = mortal_value(py);
            let dict = alloc_dict_with_pairs(py, &[alpha, finalizing]);
            assert!(!dict.is_null());
            let dict_bits = MoltObject::from_ptr(dict).bits();
            // The dictionary becomes the finalizing value's last owner.
            dec_ref_bits(py, finalizing);
            OBSERVED_DICT.store(dict_bits, Ordering::SeqCst);
            OBSERVED_VALUE.store(value, Ordering::SeqCst);
            OBSERVED_COMMITTED.store(0, Ordering::SeqCst);
            FINALIZER_CALLS.store(0, Ordering::SeqCst);
            // Replacing "alpha" displaces the finalizing value before "beta"
            // is bound; its release must wait for the whole transition.
            let displaced = dict_bind_string_entries(py, dict, &[(alpha, value), (beta, value)])
                .expect("binding commits");
            assert_eq!(FINALIZER_CALLS.load(Ordering::SeqCst), 0);
            drop(displaced);
            assert_eq!(FINALIZER_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(
                OBSERVED_COMMITTED.load(Ordering::SeqCst),
                1,
                "the finalizer observed every binding committed"
            );
            OBSERVED_DICT.store(0, Ordering::SeqCst);
            OBSERVED_VALUE.store(0, Ordering::SeqCst);
            for bits in [dict_bits, alpha, beta, value, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn string_binding_rejects_unbounded_repeated_or_inexact_keys() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let value = MoltObject::from_int(1).bits();
            let alpha = string_key(py, b"alpha");
            let alpha_again = string_key(py, b"alpha");
            let keys = [
                b"k0".as_slice(),
                b"k1".as_slice(),
                b"k2".as_slice(),
                b"k3".as_slice(),
                b"k4".as_slice(),
            ]
            .map(|name| string_key(py, name));
            let unbounded = keys.map(|key| (key, value));
            let dict = alloc_dict_with_pairs(py, &[]);
            assert!(!dict.is_null());
            for entries in [
                &[(alpha, value), (alpha_again, value)][..],
                &[(MoltObject::from_int(3).bits(), value)][..],
                &unbounded[..],
            ] {
                assert!(dict_bind_string_entries(py, dict, entries).is_err());
                take_pending_error(py, "SystemError");
                assert_eq!(dict_len(dict), 0);
            }
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
            for bits in keys.into_iter().chain([alpha, alpha_again]) {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn exact_string_reads_preserve_key_identity_borrowing_and_pending_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let stored = string_key(py, b"stored-name");
            let query = string_key(py, b"stored-name");
            let absent = string_key(py, b"absent-name");
            let value = mortal_value(py);
            let dict = alloc_dict_with_pairs(py, &[stored, value]);
            assert!(!dict.is_null());
            assert_ne!(stored, query);
            let before = (owners(stored), owners(query), owners(value));
            assert_eq!(dict_find_entry(py, dict, query), Some(0));
            assert_eq!(dict_get_in_place(py, dict, query), Some(value));
            assert_eq!(
                dict_find_entry_kv_in_place(py, dict, query),
                Some((stored, value))
            );
            assert_eq!(dict_get_in_place(py, dict, absent), None);
            assert_eq!(dict_find_entry_kv_in_place(py, dict, absent), None);
            assert_eq!(dict_find_entry(py, dict, absent), None);
            assert_eq!(
                crate::object::ops_dict::molt_dict_getitem_borrowed(
                    MoltObject::from_ptr(dict).bits(),
                    query
                ),
                value
            );
            assert_eq!((owners(stored), owners(query), owners(value)), before);

            // All projections retain the established pending-error admission: a
            // lookup must not clear or replace the caller's pending exception.
            raise_exception::<u64>(py, "RuntimeError", "preexisting read error");
            let pending = exception_last_bits_noinc(py);
            assert_eq!(dict_find_entry(py, dict, query), None);
            assert_eq!(dict_get_in_place(py, dict, query), None);
            assert_eq!(dict_find_entry_kv_in_place(py, dict, query), None);
            assert_eq!(exception_last_bits_noinc(py), pending);
            take_pending_error(py, "RuntimeError");
            assert!(dict_del_in_place(py, dict, stored));
            assert_eq!(dict_get_in_place(py, dict, query), None);
            assert_eq!(dict_find_entry_kv_in_place(py, dict, query), None);
            for bits in [
                MoltObject::from_ptr(dict).bits(),
                stored,
                query,
                absent,
                value,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn exact_string_reads_keep_same_hash_equality_order_and_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (probe, class, function) = same_hash_probe_key(py);
            let query = string_key(py, b"selected");
            let stored = string_key(py, b"selected");
            let value = MoltObject::from_int(42).bits();
            let dict = dict_with_same_hash_key(py, &[], probe, b"selected");
            dict_set_in_place(py, dict, stored, value);
            assert!(!exception_pending(py));
            for projection in 0..3 {
                for (mode, expected) in [
                    (PROBE_UNEQUAL, (stored, value)),
                    (PROBE_EQUAL, (probe, MoltObject::from_int(7).bits())),
                ] {
                    PROBE_MODE.store(mode, Ordering::SeqCst);
                    PROBE_CALLS.store(0, Ordering::SeqCst);
                    if projection == 0 {
                        assert_eq!(
                            dict_find_entry(py, dict, query),
                            Some(if mode == PROBE_EQUAL { 0 } else { 1 })
                        );
                    } else if projection == 1 {
                        assert_eq!(dict_find_entry_kv_in_place(py, dict, query), Some(expected));
                    } else {
                        assert_eq!(dict_get_in_place(py, dict, query), Some(expected.1));
                    }
                    assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
                }
                PROBE_MODE.store(PROBE_RAISE, Ordering::SeqCst);
                PROBE_CALLS.store(0, Ordering::SeqCst);
                if projection == 0 {
                    assert_eq!(dict_find_entry(py, dict, query), None);
                } else if projection == 1 {
                    assert_eq!(dict_find_entry_kv_in_place(py, dict, query), None);
                } else {
                    assert_eq!(dict_get_in_place(py, dict, query), None);
                }
                assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
                take_pending_error(py, "RuntimeError");
                assert_eq!(
                    dict_live_entries(dict)
                        .flat_map(|row| [row.key, row.value])
                        .collect::<Vec<_>>()
                        .as_slice(),
                    &[probe, MoltObject::from_int(7).bits(), stored, value]
                );
            }
            // Public method reads must not retain a default or attempt a
            // second lookup after the original equality callback raises.
            let default = mortal_value(py);
            let default_owners = owners(default);
            type Method = extern "C" fn(u64, u64, u64) -> u64;
            for method in [
                crate::object::ops_dict::molt_dict_get as Method,
                crate::object::ops_dict::molt_dict_setdefault as Method,
            ] {
                PROBE_MODE.store(PROBE_RAISE, Ordering::SeqCst);
                PROBE_CALLS.store(0, Ordering::SeqCst);
                assert_eq!(
                    method(MoltObject::from_ptr(dict).bits(), query, default),
                    MoltObject::none().bits()
                );
                assert_eq!(owners(default), default_owners);
                assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
                take_pending_error(py, "RuntimeError");
                assert_eq!(
                    dict_live_entries(dict)
                        .flat_map(|row| [row.key, row.value])
                        .collect::<Vec<_>>()
                        .as_slice(),
                    &[probe, MoltObject::from_int(7).bits(), stored, value]
                );
            }
            PROBE_CALLS.store(0, Ordering::SeqCst);
            assert_eq!(
                crate::object::ops_dict::molt_dict_getitem_borrowed(
                    MoltObject::from_ptr(dict).bits(),
                    query
                ),
                0
            );
            assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
            assert!(!exception_pending(py));
            dec_ref_bits(py, default);
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            for bits in [
                MoltObject::from_ptr(dict).bits(),
                query,
                stored,
                probe,
                class,
                function,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn exact_string_reads_reacquire_entries_after_equality_mutates_the_table() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (probe, class, function) = same_hash_probe_key(py);
            let removed = string_key(py, b"removed");
            let query = string_key(py, b"inserted");
            let inserted = string_key(py, b"inserted");
            for projection in 0..3 {
                let dict = dict_with_same_hash_key(
                    py,
                    &[removed, MoltObject::none().bits()],
                    probe,
                    b"inserted",
                );
                PROBE_DICT.store(MoltObject::from_ptr(dict).bits(), Ordering::SeqCst);
                PROBE_DELETE.store(removed, Ordering::SeqCst);
                PROBE_INSERT.store(inserted, Ordering::SeqCst);
                PROBE_CALLS.store(0, Ordering::SeqCst);
                PROBE_MODE.store(PROBE_RESTRUCTURE, Ordering::SeqCst);
                if projection == 0 {
                    assert_eq!(dict_find_entry(py, dict, query), Some(1));
                } else if projection == 1 {
                    assert_eq!(
                        dict_find_entry_kv_in_place(py, dict, query),
                        Some((inserted, MoltObject::from_int(40).bits()))
                    );
                } else {
                    assert_eq!(
                        dict_get_in_place(py, dict, query),
                        Some(MoltObject::from_int(40).bits())
                    );
                }
                // Initial read, callback insertion, then restarted read. The
                // speculative string probe itself may not call equality.
                assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 3);
                assert_eq!(
                    dict_live_entries(dict)
                        .flat_map(|row| [row.key, row.value])
                        .collect::<Vec<_>>()
                        .as_slice(),
                    &[
                        probe,
                        MoltObject::from_int(7).bits(),
                        inserted,
                        MoltObject::from_int(40).bits()
                    ]
                );
                PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
                PROBE_DICT.store(0, Ordering::SeqCst);
                PROBE_DELETE.store(0, Ordering::SeqCst);
                PROBE_INSERT.store(0, Ordering::SeqCst);
                dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
            }
            for bits in [removed, query, inserted, probe, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

static SETDEFAULT_HASH_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn setdefault_hash_once(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if SETDEFAULT_HASH_CALLS.fetch_add(1, Ordering::SeqCst) != 0 {
            return raise_exception::<u64>(py, "RuntimeError", "setdefault hashed twice");
        }
        MoltObject::from_int(37).bits()
    })
}

#[test]
fn exact_string_reads_setdefault_hashes_once_and_balances_result_ownership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (key, class, function) = instance_with_method(
                py,
                b"SetdefaultHashOnce",
                b"__hash__",
                "setdefault_hash_once",
                setdefault_hash_once as *const (),
                1,
            );
            for empty_list in [false, true] {
                let dict = alloc_dict_with_pairs(py, &[]);
                assert!(!dict.is_null());
                let dict_bits = MoltObject::from_ptr(dict).bits();
                let default = mortal_value(py);
                SETDEFAULT_HASH_CALLS.store(0, Ordering::SeqCst);
                let result = if empty_list {
                    crate::object::ops_dict::molt_dict_setdefault_empty_list(dict_bits, key)
                } else {
                    crate::object::ops_dict::molt_dict_setdefault(dict_bits, key, default)
                };
                assert!(!exception_pending(py));
                assert_eq!(SETDEFAULT_HASH_CALLS.load(Ordering::SeqCst), 1);
                assert_eq!(
                    dict_live_entries(dict)
                        .flat_map(|row| [row.key, row.value])
                        .collect::<Vec<_>>()
                        .as_slice(),
                    &[key, result]
                );
                assert_eq!(owners(result), if empty_list { 2 } else { 3 });
                assert_eq!(owners(default), if empty_list { 1 } else { 3 });
                dec_ref_bits(py, result);

                SETDEFAULT_HASH_CALLS.store(0, Ordering::SeqCst);
                let hit = if empty_list {
                    crate::object::ops_dict::molt_dict_setdefault_empty_list(dict_bits, key)
                } else {
                    crate::object::ops_dict::molt_dict_setdefault(dict_bits, key, default)
                };
                assert_eq!(hit, result);
                assert_eq!(SETDEFAULT_HASH_CALLS.load(Ordering::SeqCst), 1);
                assert_eq!(owners(hit), if empty_list { 2 } else { 3 });
                dec_ref_bits(py, hit);
                dec_ref_bits(py, dict_bits);
                assert_eq!(owners(default), 1);
                dec_ref_bits(py, default);
            }
            for bits in [key, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn exact_string_reads_setdefault_reuses_probe_and_observes_reentrant_insertion() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (probe, class, function) = same_hash_probe_key(py);
            let removed = string_key(py, b"removed");
            let query = string_key(py, b"selected");
            for empty_list in [false, true] {
                for mode in [PROBE_UNEQUAL, PROBE_EQUAL, PROBE_RAISE, PROBE_RESTRUCTURE] {
                    let dict = dict_with_same_hash_key(
                        py,
                        &[removed, MoltObject::none().bits()],
                        probe,
                        b"selected",
                    );
                    let dict_bits = MoltObject::from_ptr(dict).bits();
                    let default = mortal_value(py);
                    PROBE_DICT.store(dict_bits, Ordering::SeqCst);
                    PROBE_DELETE.store(removed, Ordering::SeqCst);
                    PROBE_INSERT.store(query, Ordering::SeqCst);
                    PROBE_CALLS.store(0, Ordering::SeqCst);
                    PROBE_MODE.store(mode, Ordering::SeqCst);
                    let result = if empty_list {
                        crate::object::ops_dict::molt_dict_setdefault_empty_list(dict_bits, query)
                    } else {
                        crate::object::ops_dict::molt_dict_setdefault(dict_bits, query, default)
                    };
                    assert_eq!(
                        PROBE_CALLS.load(Ordering::SeqCst),
                        if mode == PROBE_RESTRUCTURE { 3 } else { 1 }
                    );
                    match mode {
                        PROBE_RAISE => {
                            assert!(obj_from_bits(result).is_none());
                            take_pending_error(py, "RuntimeError");
                            assert_eq!(
                                dict_live_entries(dict)
                                    .flat_map(|row| [row.key, row.value])
                                    .collect::<Vec<_>>()
                                    .len(),
                                4
                            );
                            assert_eq!(owners(default), 1);
                        }
                        PROBE_EQUAL => assert_eq!(result, MoltObject::from_int(7).bits()),
                        PROBE_RESTRUCTURE => {
                            assert_eq!(result, MoltObject::from_int(40).bits());
                            assert_eq!(
                                dict_live_entries(dict)
                                    .flat_map(|row| [row.key, row.value])
                                    .collect::<Vec<_>>()
                                    .as_slice(),
                                &[probe, MoltObject::from_int(7).bits(), query, result]
                            );
                        }
                        _ => {
                            assert_eq!(
                                dict_live_entries(dict)
                                    .flat_map(|row| [row.key, row.value])
                                    .collect::<Vec<_>>()
                                    .len(),
                                6
                            );
                            assert_eq!(
                                dict_live_entries(dict)
                                    .flat_map(|row| [row.key, row.value])
                                    .collect::<Vec<_>>()[4],
                                query
                            );
                            assert_eq!(
                                dict_live_entries(dict)
                                    .flat_map(|row| [row.key, row.value])
                                    .collect::<Vec<_>>()[5],
                                result
                            );
                            assert_eq!(owners(result), if empty_list { 2 } else { 3 });
                        }
                    }
                    dec_ref_bits(py, result);
                    PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
                    dec_ref_bits(py, dict_bits);
                    assert_eq!(owners(default), 1);
                    dec_ref_bits(py, default);
                }
            }
            PROBE_DICT.store(0, Ordering::SeqCst);
            PROBE_DELETE.store(0, Ordering::SeqCst);
            PROBE_INSERT.store(0, Ordering::SeqCst);
            for bits in [removed, query, probe, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn exact_string_reads_setdefault_reservation_failure_keeps_entries_and_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let key = string_key(py, b"new-default");
            let default = mortal_value(py);
            let _ = hash_bits(py, key);
            for empty_list in [false, true] {
                let dict = alloc_dict_with_pairs(py, &[]);
                assert!(!dict.is_null());
                let epoch = dict_structural_epoch(dict);
                let before = (owners(key), owners(default));
                let _reset = TrackerReset;
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_memory: Some(0),
                    max_allocations: Some(0),
                    ..ResourceLimits::default()
                })));
                let result =
                    dict_setdefault_in_place(py, dict, key, (!empty_list).then_some(default));
                set_tracker(Box::new(UnlimitedTracker));
                assert_eq!(result, None);
                // The zero-allocation budget also prevents constructing a
                // heap exception. The canonical raised state keeps its
                // allocation-free MemoryError marker, not an exception object.
                assert!(exception_pending(py));
                let raised = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(obj_from_bits(raised).is_none());
                crate::clear_exception(py);
                assert!(!exception_pending(py));
                assert!(
                    dict_live_entries(dict)
                        .flat_map(|row| [row.key, row.value])
                        .collect::<Vec<_>>()
                        .is_empty()
                );
                assert_eq!(dict_structural_epoch(dict), epoch);
                assert_eq!((owners(key), owners(default)), before);
                dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
            }
            dec_ref_bits(py, key);
            dec_ref_bits(py, default);
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn sparse_erase_is_allocation_free_and_reinsertion_preserves_live_order() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let int = |value| MoltObject::from_int(value).bits();
            let dict = alloc_dict_with_pairs(
                py,
                &[
                    int(0),
                    int(10),
                    int(1),
                    int(11),
                    int(2),
                    int(12),
                    int(3),
                    int(13),
                ],
            );
            let entries_owner = dict_entries_ptr(dict);
            let table_owner = dict_table_ptr(dict);
            let reset = TrackerReset;
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                max_allocations: Some(0),
                ..Default::default()
            })));
            assert!(dict_del_in_place(py, dict, int(1)));
            assert!(dict_del_in_place(py, dict, int(3)));
            assert!(!exception_pending(py));
            drop(reset);
            assert_eq!(dict_len(dict), 2);
            assert_eq!(
                dict_entries(dict).len(),
                4,
                "ordinary erase preserves physical cursor extent"
            );
            assert_eq!(dict_entries_ptr(dict), entries_owner);
            assert_eq!(dict_table_ptr(dict), table_owner);
            assert_eq!(
                dict_live_entries(dict)
                    .map(|row| row.key)
                    .collect::<Vec<_>>(),
                vec![int(0), int(2)]
            );
            // Insertion crosses the holes >= live compaction boundary. The live
            // order is the CPython oracle; physical row addresses are internal.
            dict_set_in_place(py, dict, int(1), int(21));
            assert_eq!(
                dict_live_entries(dict)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>(),
                vec![int(0), int(10), int(2), int(12), int(1), int(21)]
            );
            assert_eq!(dict_entries_ptr(dict), entries_owner);
            assert_eq!(dict_table_ptr(dict), table_owner);
            assert_eq!(dict_get_in_place(py, dict, int(1)), Some(int(21)));
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
        }
    });
}

#[test]
fn stored_hash_sentinel_is_rejected_before_ownership_or_storage_changes() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let key = string_key(py, b"sentinel key");
            let value = mortal_value(py);
            let dict = alloc_dict_with_pairs(py, &[]);
            let before = (owners(key), owners(value), dict_structural_epoch(dict));
            dict_set_with_hash_in_place(py, dict, key, value, u64::MAX);
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(
                (owners(key), owners(value), dict_structural_epoch(dict)),
                before
            );
            assert_eq!(dict_len(dict), 0);
            assert!(dict_entries(dict).is_empty());
            assert!(dict_del_with_hash_deferred(py, dict, key, u64::MAX).is_none());
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(
                (owners(key), owners(value), dict_structural_epoch(dict)),
                before
            );
            let set = crate::molt_set_new(0);
            let ptr = obj_from_bits(set).as_ptr().unwrap();
            let before = owners(key);
            set_add_with_hash_in_place(py, ptr, key, u64::MAX);
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(owners(key), before);
            assert_eq!(set_len(ptr), 0);
            assert!(set_entries(ptr).is_empty());
            assert!(!set_del_with_hash_in_place(py, ptr, key, u64::MAX));
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(owners(key), before);
            // Every other hash bit pattern, including zero and -2, is storable.
            for hash in [0, 1, i64::MIN as u64, u64::MAX - 1] {
                let stored = StoredHash::new(hash).expect("valid Python stored hash");
                assert_eq!(stored.get(), hash);
            }
            for bits in [MoltObject::from_ptr(dict).bits(), set, key, value] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}
