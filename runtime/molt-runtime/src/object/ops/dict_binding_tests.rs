use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
use std::sync::atomic::{AtomicU64, Ordering};

unsafe fn dict_find_entry_kv_in_place(
    py: &PyToken<'_>,
    dict: *mut u8,
    key_bits: u64,
) -> Result<Option<(u64, u64)>, ()> {
    unsafe {
        let hash = dict_key_hash(py, key_bits).ok_or(())?;
        dict_find_entry_kv_with_hash(py, dict, key_bits, hash)
    }
}

const PROBE_UNEQUAL: u64 = 0;
const PROBE_RAISE: u64 = 1;
const PROBE_RESTRUCTURE: u64 = 2;
const PROBE_EQUAL: u64 = 3;
const PROBE_REPLACE_FIRST: u64 = 4;
const PROBE_REPLACE_SECOND: u64 = 5;
const PROBE_DELETE_SELF: u64 = 6;
const PROBE_REPLACE_FIRST_REFUSE_REPEAT: u64 = 7;
const PROBE_RAISE_OWNED: u64 = 8;

static PROBE_MODE: AtomicU64 = AtomicU64::new(PROBE_UNEQUAL);
static PROBE_CALLS: AtomicU64 = AtomicU64::new(0);
static PROBE_DICT: AtomicU64 = AtomicU64::new(0);
static PROBE_DELETE: AtomicU64 = AtomicU64::new(0);
static PROBE_INSERT: AtomicU64 = AtomicU64::new(0);
static PROBE_VALUE: AtomicU64 = AtomicU64::new(0);
static PROBE_HASH: AtomicU64 = AtomicU64::new(0);
static OBSERVED_DICT: AtomicU64 = AtomicU64::new(0);
static OBSERVED_VALUE: AtomicU64 = AtomicU64::new(0);
static OBSERVED_COMMITTED: AtomicU64 = AtomicU64::new(0);
static FINALIZER_CALLS: AtomicU64 = AtomicU64::new(0);

/// `__eq__` of a key stored under the hash of a bound `str`: ordinary lookup of
/// that `str` must consult it, and it may raise or restructure the mapping.
extern "C" fn same_hash_key_eq(self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let call = PROBE_CALLS.fetch_add(1, Ordering::SeqCst);
        match PROBE_MODE.load(Ordering::SeqCst) {
            PROBE_EQUAL => MoltObject::from_bool(true).bits(),
            PROBE_RAISE => raise_exception::<u64>(py, "RuntimeError", "probe equality failure"),
            PROBE_RAISE_OWNED => {
                crate::builtins::exceptions::molt_exception_set_last(
                    PROBE_VALUE.load(Ordering::SeqCst),
                );
                MoltObject::none().bits()
            }
            PROBE_DELETE_SELF => {
                let dict = obj_from_bits(PROBE_DICT.load(Ordering::SeqCst))
                    .as_ptr()
                    .unwrap();
                let detached = unsafe {
                    dict_remove_identity_with_hash_deferred(
                        py,
                        dict,
                        self_bits,
                        PROBE_HASH.load(Ordering::SeqCst),
                    )
                };
                drop(detached);
                MoltObject::from_bool(true).bits()
            }
            mode @ (PROBE_REPLACE_FIRST
            | PROBE_REPLACE_SECOND
            | PROBE_REPLACE_FIRST_REFUSE_REPEAT) => {
                if mode == PROBE_REPLACE_FIRST_REFUSE_REPEAT && call != 0 {
                    return raise_exception::<u64>(
                        py,
                        "RuntimeError",
                        "unexpected second equality",
                    );
                }
                if call == u64::from(mode == PROBE_REPLACE_SECOND) {
                    let dict = obj_from_bits(PROBE_DICT.load(Ordering::SeqCst))
                        .as_ptr()
                        .unwrap();
                    unsafe {
                        assert!(dict_set_with_hash_in_place(
                            py,
                            dict,
                            self_bits,
                            PROBE_VALUE.load(Ordering::SeqCst),
                            PROBE_HASH.load(Ordering::SeqCst),
                        ));
                    }
                }
                MoltObject::from_bool(true).bits()
            }
            PROBE_RESTRUCTURE if call == 0 => {
                let dict = obj_from_bits(PROBE_DICT.load(Ordering::SeqCst))
                    .as_ptr()
                    .unwrap();
                unsafe {
                    // Delete an earlier row, then bind a key the transition
                    // already probed as absent. Only exhausted usable capacity
                    // makes the insertion compact/reindex the remaining rows.
                    assert!(
                        dict_del_in_place(py, dict, PROBE_DELETE.load(Ordering::SeqCst)).unwrap()
                    );
                    assert!(dict_set_in_place(
                        py,
                        dict,
                        PROBE_INSERT.load(Ordering::SeqCst),
                        MoltObject::from_int(40).bits(),
                    ));
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
        assert!(dict_set_with_hash_in_place(
            py,
            dict,
            probe,
            MoltObject::from_int(7).bits(),
            hash_string_bytes(py, name) as u64,
        ));
    }
    assert!(!exception_pending(py));
    dict
}

/// Consume the initial key table's append capacity without changing its live
/// entries. The next absent insertion must perform real compaction/reindexing.
unsafe fn exhaust_dict_usable_with_deleted_rows(py: &PyToken<'_>, dict: *mut u8) {
    unsafe {
        assert_eq!(dict_len(dict), 2);
        assert_eq!(dict_table(dict).len(), 8);
        assert_eq!(dict_storage(dict).usable, 3);
        for integer in 0..3 {
            let key = MoltObject::from_int(integer).bits();
            assert!(dict_set_in_place(py, dict, key, MoltObject::none().bits()));
            assert!(dict_del_in_place(py, dict, key).unwrap());
        }
        assert_eq!(dict_len(dict), 2);
        assert_eq!(dict_entries(dict).len(), 5);
        assert_eq!(dict_storage(dict).usable, 0);
    }
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
                assert_eq!(dict_get_in_place(py, dict, key).unwrap(), Some(value));
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
                assert!(dict_set_in_place(
                    py,
                    dict,
                    MoltObject::from_int(filler).bits(),
                    MoltObject::none().bits(),
                ));
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
                assert_eq!(dict_get_in_place(py, dict, key).unwrap(), Some(value));
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
            assert_eq!(dict_find_entry(py, dict, query).unwrap(), Some(0));
            assert_eq!(dict_get_in_place(py, dict, query).unwrap(), Some(value));
            assert_eq!(
                dict_find_entry_kv_in_place(py, dict, query).unwrap(),
                Some((stored, value))
            );
            assert_eq!(dict_get_in_place(py, dict, absent).unwrap(), None);
            assert_eq!(dict_find_entry_kv_in_place(py, dict, absent).unwrap(), None);
            assert_eq!(dict_find_entry(py, dict, absent).unwrap(), None);
            assert_eq!(
                crate::object::ops_dict::molt_dict_getitem_borrowed(
                    MoltObject::from_ptr(dict).bits(),
                    query
                ),
                value
            );
            assert_eq!((owners(stored), owners(query), owners(value)), before);

            // Exact storage reads succeed under an admitted prior error, as
            // PyDict_GetItemWithError does; no callback or indicator inference
            // changes the lookup result or the caller's selected exception.
            raise_exception::<u64>(py, "RuntimeError", "preexisting read error");
            let pending = exception_last_bits_noinc(py);
            assert_eq!(dict_find_entry(py, dict, query).unwrap(), Some(0));
            assert_eq!(dict_get_in_place(py, dict, query).unwrap(), Some(value));
            assert_eq!(
                dict_find_entry_kv_in_place(py, dict, query).unwrap(),
                Some((stored, value))
            );
            assert_eq!(dict_get_in_place(py, dict, absent).unwrap(), None);
            assert_eq!(exception_last_bits_noinc(py), pending);
            take_pending_error(py, "RuntimeError");
            assert!(dict_del_in_place(py, dict, stored).unwrap());
            assert_eq!(dict_get_in_place(py, dict, query).unwrap(), None);
            assert_eq!(dict_find_entry_kv_in_place(py, dict, query).unwrap(), None);
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
            assert!(dict_set_in_place(py, dict, stored, value));
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
                            dict_find_entry(py, dict, query).unwrap(),
                            Some(if mode == PROBE_EQUAL { 0 } else { 1 })
                        );
                    } else if projection == 1 {
                        assert_eq!(
                            dict_find_entry_kv_in_place(py, dict, query).unwrap(),
                            Some(expected)
                        );
                    } else {
                        assert_eq!(
                            dict_get_in_place(py, dict, query).unwrap(),
                            Some(expected.1)
                        );
                    }
                    assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
                }
                PROBE_MODE.store(PROBE_RAISE, Ordering::SeqCst);
                PROBE_CALLS.store(0, Ordering::SeqCst);
                if projection == 0 {
                    assert_eq!(dict_find_entry(py, dict, query), Err(()));
                } else if projection == 1 {
                    assert_eq!(dict_find_entry_kv_in_place(py, dict, query), Err(()));
                } else {
                    assert_eq!(dict_get_in_place(py, dict, query), Err(()));
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
            for (compact, probe_first) in [(false, false), (true, false), (true, true)] {
                for projection in 0..3 {
                    let initial_entries = [removed, MoltObject::none().bits()];
                    let dict = dict_with_same_hash_key(
                        py,
                        if probe_first { &[] } else { &initial_entries },
                        probe,
                        b"inserted",
                    );
                    if probe_first {
                        assert!(dict_set_in_place(
                            py,
                            dict,
                            removed,
                            MoltObject::none().bits()
                        ));
                    }
                    if compact {
                        exhaust_dict_usable_with_deleted_rows(py, dict);
                    }
                    let table_before = dict_table(dict).as_ptr();
                    let layout_before =
                        crate::object::backing::tracked_vec_mutation_epoch(dict_table_ptr(dict));
                    PROBE_DICT.store(MoltObject::from_ptr(dict).bits(), Ordering::SeqCst);
                    PROBE_DELETE.store(removed, Ordering::SeqCst);
                    PROBE_INSERT.store(inserted, Ordering::SeqCst);
                    PROBE_CALLS.store(0, Ordering::SeqCst);
                    PROBE_MODE.store(PROBE_RESTRUCTURE, Ordering::SeqCst);
                    if projection == 0 {
                        // Without reindexing the deleted row remains a hole;
                        // after real compaction the original dense index applies.
                        assert_eq!(
                            dict_find_entry(py, dict, query).unwrap(),
                            Some(if compact { 1 } else { 2 })
                        );
                    } else if projection == 1 {
                        assert_eq!(
                            dict_find_entry_kv_in_place(py, dict, query).unwrap(),
                            Some((inserted, MoltObject::from_int(40).bits()))
                        );
                    } else {
                        assert_eq!(
                            dict_get_in_place(py, dict, query).unwrap(),
                            Some(MoltObject::from_int(40).bits())
                        );
                    }
                    // Initial read + callback insertion. A changed key layout
                    // adds exactly one restarted comparison; a mere hole does not.
                    assert_eq!(
                        PROBE_CALLS.load(Ordering::SeqCst),
                        if compact { 3 } else { 2 }
                    );
                    let layout_after =
                        crate::object::backing::tracked_vec_mutation_epoch(dict_table_ptr(dict));
                    assert_eq!(layout_before != layout_after, compact);
                    // Same-capacity reindexing reuses the index allocation. The
                    // probe-first case also leaves the compared row at index 0.
                    assert_eq!(dict_table(dict).as_ptr(), table_before);
                    assert_eq!(dict_entries(dict).len(), if compact { 2 } else { 3 });
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
                for (mode, compact) in [
                    (PROBE_UNEQUAL, false),
                    (PROBE_EQUAL, false),
                    (PROBE_RAISE, false),
                    (PROBE_RESTRUCTURE, false),
                    (PROBE_RESTRUCTURE, true),
                ] {
                    let dict = dict_with_same_hash_key(
                        py,
                        &[removed, MoltObject::none().bits()],
                        probe,
                        b"selected",
                    );
                    if compact {
                        exhaust_dict_usable_with_deleted_rows(py, dict);
                    }
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
                        if mode == PROBE_RESTRUCTURE {
                            if compact { 3 } else { 2 }
                        } else {
                            1
                        }
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
                assert_eq!(result, None);
                // The prepared reserve publishes and transfers a real instance
                // while the same zero-allocation resource budget is active.
                crate::test_support::assert_and_clear_emergency_memory_error(py);
                set_tracker(Box::new(UnlimitedTracker));
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
            assert!(dict_del_in_place(py, dict, int(1)).unwrap());
            assert!(dict_del_in_place(py, dict, int(3)).unwrap());
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
            // Erasure does not refund append capacity or trigger compaction.
            // This fifth append retains the earlier holes and the live order.
            assert!(dict_set_in_place(py, dict, int(1), int(21)));
            assert_eq!(dict_entries(dict).len(), 5);
            assert_eq!(
                dict_live_entries(dict)
                    .flat_map(|row| [row.key, row.value])
                    .collect::<Vec<_>>(),
                vec![int(0), int(10), int(2), int(12), int(1), int(21)]
            );
            assert_eq!(dict_entries_ptr(dict), entries_owner);
            assert_eq!(dict_table_ptr(dict), table_owner);
            assert_eq!(dict_get_in_place(py, dict, int(1)).unwrap(), Some(int(21)));
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
            assert!(!dict_set_with_hash_in_place(py, dict, key, value, u64::MAX));
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(
                (owners(key), owners(value), dict_structural_epoch(dict)),
                before
            );
            assert_eq!(dict_len(dict), 0);
            assert!(dict_entries(dict).is_empty());
            assert!(dict_del_with_hash_deferred(py, dict, key, u64::MAX).is_err());
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(
                (owners(key), owners(value), dict_structural_epoch(dict)),
                before
            );
            let set = crate::molt_set_new(0);
            let ptr = obj_from_bits(set).as_ptr().unwrap();
            let before = owners(key);
            assert!(!set_add_with_hash_in_place(py, ptr, key, u64::MAX));
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(owners(key), before);
            assert_eq!(set_len(ptr), 0);
            assert!(set_entries(ptr).is_empty());
            assert_eq!(set_del_with_hash_in_place(py, ptr, key, u64::MAX), Err(()));
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

#[test]
fn prehashed_setdefault_preserves_canonical_key_and_owned_result() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let key = string_key(py, b"cache-key");
            let equal = string_key(py, b"cache-key");
            let value = mortal_value(py);
            let other = mortal_value(py);
            let dict = alloc_dict_with_pairs(py, &[]);
            assert!(!dict.is_null());
            let hash = dict_key_hash(py, key).unwrap();
            let baseline = owners(value);
            let (published, inserted) =
                dict_setdefault_with_hash_in_place(py, dict, key, Some(value), hash).unwrap();
            assert!(inserted);
            assert_eq!(published, value);
            assert_eq!(owners(value), baseline + 2); // dictionary and returned owner
            dec_ref_bits(py, published);
            let other_before = owners(other);
            let (resident, inserted) =
                dict_setdefault_with_hash_in_place(py, dict, equal, Some(other), hash).unwrap();
            assert!(!inserted);
            assert_eq!(resident, value);
            assert_eq!(owners(other), other_before);
            assert_eq!(
                dict_find_entry_kv_with_hash(py, dict, equal, hash).unwrap(),
                Some((key, value))
            );
            dec_ref_bits(py, resident);
            let retired = dict_remove_identity_with_hash_deferred(py, dict, key, hash).unwrap();
            assert_eq!(dict_len(dict), 0);
            assert_eq!(owners(value), baseline + 1, "retirement is deferred");
            drop(retired);
            assert_eq!(owners(value), baseline);
            for bits in [MoltObject::from_ptr(dict).bits(), key, equal, value, other] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn expected_value_removal_resolves_equality_once_and_checks_the_final_value() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (probe, class, function) = same_hash_probe_key(py);
            let key = string_key(py, b"expected-value-removal");
            let hash = dict_key_hash(py, key).unwrap();
            let original = mortal_value(py);
            let replacement = mortal_value(py);
            for mode in [PROBE_REPLACE_FIRST, PROBE_REPLACE_SECOND] {
                let dict = dict_with_same_hash_key(py, &[], probe, b"expected-value-removal");
                assert!(dict_set_with_hash_in_place(py, dict, probe, original, hash));
                PROBE_DICT.store(MoltObject::from_ptr(dict).bits(), Ordering::SeqCst);
                PROBE_VALUE.store(replacement, Ordering::SeqCst);
                PROBE_HASH.store(hash, Ordering::SeqCst);
                PROBE_CALLS.store(0, Ordering::SeqCst);
                PROBE_MODE.store(mode, Ordering::SeqCst);
                let retired = dict_del_if_value_deferred(py, dict, key, Some(original)).unwrap();
                assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
                if mode == PROBE_REPLACE_FIRST {
                    assert!(
                        retired.is_none(),
                        "callback replacement is outside rollback custody"
                    );
                    assert_eq!(dict_entries(dict)[0].value, replacement);
                    assert_eq!(owners(original), 1);
                    assert_eq!(owners(replacement), 2);
                } else {
                    assert!(retired.is_some(), "one lookup removes the original value");
                    assert_eq!(dict_len(dict), 0);
                    assert_eq!(owners(original), 2, "release remains deferred");
                    assert_eq!(owners(replacement), 1);
                }
                drop(retired);
                PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
                dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
                assert_eq!(owners(original), 1);
                assert_eq!(owners(replacement), 1);
            }
            for bits in [key, original, replacement, probe, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn eval_builtins_dictionary_survives_last_module_owner_deleted_during_lookup() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = string_key(py, b"builtins");
            let module = MoltObject::from_ptr(crate::alloc_module_obj(py, name)).bits();
            let builtins = crate::module_dict_bits(obj_from_bits(module).as_ptr().unwrap());
            crate::builtins::module_table::publish_interpreter_builtins_for_test(py, module);
            let (probe, class, function) = same_hash_probe_key(py);
            let dictionary = crate::builtins::module_table::module_dictionary(py).unwrap();
            let dict = obj_from_bits(dictionary).as_ptr().unwrap();
            let previous =
                crate::builtins::module_table::replace_module_owned_for_test(py, "builtins", None);
            let hash = dict_key_hash(py, name).unwrap();
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            assert!(dict_set_with_hash_in_place(py, dict, probe, module, hash));
            dec_ref_bits(py, module); // the entry is the module's sole owner
            PROBE_DICT.store(dictionary, Ordering::SeqCst);
            PROBE_HASH.store(hash, Ordering::SeqCst);
            PROBE_CALLS.store(0, Ordering::SeqCst);
            PROBE_MODE.store(PROBE_DELETE_SELF, Ordering::SeqCst);
            assert!(
                crate::builtins::module_table::module_lookup(py, "builtins")
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                PROBE_CALLS.load(Ordering::SeqCst),
                1,
                "the lookup actually ran destructive Python equality"
            );
            assert_eq!(
                owners(builtins),
                1,
                "the interpreter role is the surviving runtime owner"
            );
            let view = molt_cpython_abi::api::eval::PyEval_GetBuiltins();
            assert!(!view.is_null());
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(view)
                    .unwrap()
                    .bits(),
                builtins
            );
            assert_eq!(
                PROBE_CALLS.load(Ordering::SeqCst),
                1,
                "the interpreter-role borrow runs no module lookup callbacks"
            );
            let projected_owners = owners(builtins);
            assert_eq!(molt_cpython_abi::api::eval::PyEval_GetBuiltins(), view);
            assert_eq!(
                owners(builtins),
                projected_owners,
                "reborrowing the stable C view adds no runtime owner"
            );
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            crate::builtins::module_table::replace_module_owned_for_test(py, "builtins", previous);
            for bits in [name, probe, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn shutdown_role_census_does_not_execute_python_key_equality() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = string_key(py, b"_molt_shutdown_census");
            let (probe, class, function) = same_hash_probe_key(py);
            let value = mortal_value(py);
            let state = runtime_state(py);
            let dictionary = crate::builtins::module_table::module_dictionary(py).unwrap();
            let dict = obj_from_bits(dictionary).as_ptr().unwrap();
            let hash = dict_key_hash(py, name).unwrap();
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            assert!(dict_set_with_hash_in_place(py, dict, probe, value, hash));
            PROBE_MODE.store(PROBE_RAISE, Ordering::SeqCst);
            PROBE_CALLS.store(0, Ordering::SeqCst);
            assert_eq!(
                crate::builtins::module_table::retain_shutdown_namespace(
                    py,
                    state,
                    "_molt_shutdown_census"
                ),
                None
            );
            assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 0);
            assert!(!exception_pending(py));
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            assert!(dict_set_in_place(py, dict, name, value));
            let before = owners(value);
            PROBE_MODE.store(PROBE_RAISE, Ordering::SeqCst);
            // Inserting the measured exact name legitimately compared it to
            // the colliding key. Measure only the following shutdown census.
            PROBE_CALLS.store(0, Ordering::SeqCst);
            let retained = crate::builtins::module_table::retain_shutdown_namespace(
                py,
                state,
                "_molt_shutdown_census",
            )
            .unwrap();
            assert_eq!(retained, value);
            assert_eq!(owners(value), before + 1);
            dec_ref_bits(py, retained);
            assert_eq!(owners(value), before);
            assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 0);
            for key in [name, probe] {
                drop(dict_remove_identity_with_hash_deferred(py, dict, key, hash).unwrap());
            }
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            for bits in [name, value, probe, class, function] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn module_rollback_keeps_a_value_replaced_by_key_equality() {
    use crate::builtins::module_table::{self, tests::ModuleTestRestore};
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Unregistered,
        Ready,
        Replaced,
        Initializing,
    }
    for phase in [
        Phase::Unregistered,
        Phase::Ready,
        Phase::Replaced,
        Phase::Initializing,
    ] {
        for ordinary_delete in [false, true] {
            for already_pending in [false, true] {
                let _transaction = crate::test_support::RuntimeTestTransaction::new();
                module_table::tests::install_test_registry();
                crate::with_gil_entry_nopanic!(py, {
                    unsafe {
                        let registered = phase != Phase::Unregistered;
                        let text = if registered {
                            "g4_src"
                        } else {
                            "_molt_callback_rollback"
                        };
                        let (_restore, original) = if registered {
                            let (scope, module) = ModuleTestRestore::fresh(py, text);
                            (Some(scope), module)
                        } else {
                            (None, mortal_value(py))
                        };
                        let name = string_key(py, text.as_bytes());
                        let (probe, class, function) = same_hash_probe_key(py);
                        let dictionary =
                            crate::builtins::module_table::module_dictionary(py).unwrap();
                        let dict = obj_from_bits(dictionary).as_ptr().unwrap();
                        if phase == Phase::Replaced {
                            crate::builtins::modules::module_cache_publish(
                                name,
                                original,
                                crate::builtins::modules::ModuleCachePublication::Extension,
                            );
                            assert!(!exception_pending(py));
                        }
                        let initializing = (phase == Phase::Initializing)
                            .then(|| ModuleTestRestore::initializing(py, text, original));
                        if registered {
                            // Remove the exact public entry before installing the custom
                            // key, preserving the actually produced registry phase.
                            drop(dict_del_deferred(py, dict, name).unwrap());
                        }
                        let before = owners(original);
                        let replacement = mortal_value(py);
                        let hash = dict_key_hash(py, name).unwrap();
                        PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
                        assert!(dict_set_with_hash_in_place(py, dict, probe, original, hash));
                        PROBE_DICT.store(dictionary, Ordering::SeqCst);
                        PROBE_HASH.store(hash, Ordering::SeqCst);
                        PROBE_VALUE.store(replacement, Ordering::SeqCst);
                        PROBE_CALLS.store(0, Ordering::SeqCst);
                        PROBE_MODE.store(PROBE_REPLACE_FIRST_REFUSE_REPEAT, Ordering::SeqCst);
                        let pending = already_pending.then(|| {
                            raise_exception::<()>(py, "ValueError", "original rollback failure");
                            crate::builtins::exceptions::molt_exception_last_pending()
                        });
                        crate::builtins::modules::module_cache_remove(
                            name,
                            (!ordinary_delete).then_some(original),
                        );
                        assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
                        if let Some(expected) = pending {
                            let observed =
                                crate::builtins::exceptions::molt_exception_last_pending();
                            assert_eq!(
                                observed, expected,
                                "rollback preserves the original exception identity"
                            );
                            dec_ref_bits(py, observed);
                            crate::clear_exception(py);
                            dec_ref_bits(py, expected);
                        } else {
                            assert!(
                                !exception_pending(py),
                                "no second equality may raise after the mutation"
                            );
                        }
                        assert_eq!(
                            dict_find_entry_kv_with_hash(py, dict, probe, hash).unwrap(),
                            (!ordinary_delete).then_some((probe, replacement))
                        );
                        assert_eq!(
                            owners(original),
                            before - u32::from(phase == Phase::Initializing)
                        );
                        assert_eq!(owners(replacement), if ordinary_delete { 1 } else { 2 });
                        assert!(!module_table::module_execution_owns_initializing_namespace(
                            py, text, original
                        ));
                        if !ordinary_delete {
                            drop(
                                dict_remove_identity_with_hash_deferred(py, dict, probe, hash)
                                    .unwrap(),
                            );
                        }
                        assert_eq!(owners(replacement), 1);
                        PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
                        drop(initializing);
                        if registered {
                            assert!(module_table::module_lookup(py, text).unwrap().is_none());
                            let fresh = module_table::module_ensure(
                                py,
                                module_table::module_id_of(text).unwrap(),
                            );
                            assert!(!exception_pending(py));
                            assert_ne!(
                                fresh, original,
                                "ordinary Missing admission reruns the initializer"
                            );
                            dec_ref_bits(py, fresh);
                        }
                        for bits in [name, original, replacement, probe, class, function] {
                            dec_ref_bits(py, bits);
                        }
                        assert!(!exception_pending(py));
                    }
                });
            }
        }
    }
}

#[test]
fn module_failure_cleanup_selects_exact_error_and_chains_callback_context() {
    use crate::builtins::exceptions::{self, ExceptionFieldSlot};
    for conditional in [false, true] {
        for pending in [false, true] {
            for kind in ["KeyError", "RuntimeError"] {
                let _transaction = crate::test_support::RuntimeTestTransaction::new();
                crate::with_gil_entry_nopanic!(py, {
                    unsafe {
                        let text = b"_molt_failure_cleanup";
                        let name = string_key(py, text);
                        let (probe, class, function) = same_hash_probe_key(py);
                        let dictionary =
                            crate::builtins::module_table::module_dictionary(py).unwrap();
                        let dict = obj_from_bits(dictionary).as_ptr().unwrap();
                        let publication = mortal_value(py);
                        let hash = hash_string_bytes(py, text) as u64;
                        PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
                        assert!(dict_set_with_hash_in_place(
                            py,
                            dict,
                            probe,
                            publication,
                            hash
                        ));
                        let prior = MoltObject::from_ptr(exceptions::alloc_exception(
                            py,
                            "ValueError",
                            "source body",
                        ))
                        .bits();
                        let cleanup = MoltObject::from_ptr(exceptions::alloc_exception(
                            py,
                            kind,
                            "cleanup callback",
                        ))
                        .bits();
                        if pending {
                            exceptions::molt_exception_set_last(prior);
                        }
                        PROBE_VALUE.store(cleanup, Ordering::SeqCst);
                        PROBE_CALLS.store(0, Ordering::SeqCst);
                        PROBE_MODE.store(PROBE_RAISE_OWNED, Ordering::SeqCst);
                        crate::builtins::modules::module_cache_remove(
                            name,
                            conditional.then_some(publication),
                        );
                        assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
                        assert!(exception_pending(py));
                        let selected = exceptions::molt_exception_last_pending();
                        let preserves = pending && (conditional || kind == "KeyError");
                        assert_eq!(selected, if preserves { prior } else { cleanup });
                        let context =
                            exceptions::exception_field(py, cleanup, ExceptionFieldSlot::Context)
                                .unwrap();
                        assert_eq!(
                            context.bits(),
                            if pending && !conditional {
                                prior
                            } else {
                                MoltObject::none().bits()
                            }
                        );
                        drop(context);
                        crate::clear_exception(py);
                        dec_ref_bits(py, selected);
                        PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
                        drop(
                            dict_remove_identity_with_hash_deferred(py, dict, probe, hash).unwrap(),
                        );
                        // Explicitly sever the graph so retirement proves that
                        // neither the callback nor context transfer leaked an edge.
                        exceptions::exception_replace_field_bits(
                            py,
                            cleanup,
                            ExceptionFieldSlot::Context,
                            MoltObject::none().bits(),
                        )
                        .unwrap();
                        for bits in [name, probe, class, function, publication, prior, cleanup] {
                            dec_ref_bits(py, bits);
                        }
                        assert!(!exception_pending(py));
                    }
                });
            }
        }
    }
}

#[test]
fn exact_identity_retirement_does_not_call_colliding_equality() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (probe, class, function) = same_hash_probe_key(py);
            let key = string_key(py, b"identity-retirement");
            let equal_but_not_stored = string_key(py, b"identity-retirement");
            let value = mortal_value(py);
            let dict = dict_with_same_hash_key(py, &[], probe, b"identity-retirement");
            assert!(dict_set_in_place(py, dict, key, value));
            let hash = dict_key_hash(py, key).unwrap();
            PROBE_CALLS.store(0, Ordering::SeqCst);
            PROBE_MODE.store(PROBE_RAISE, Ordering::SeqCst);
            assert!(
                dict_remove_identity_with_hash_deferred(py, dict, equal_but_not_stored, hash)
                    .is_none()
            );
            let retired = dict_remove_identity_with_hash_deferred(py, dict, key, hash).unwrap();
            assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 0);
            assert!(!exception_pending(py));
            assert_eq!(dict_len(dict), 1);
            drop(retired);
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            for bits in [
                MoltObject::from_ptr(dict).bits(),
                key,
                equal_but_not_stored,
                value,
                probe,
                class,
                function,
            ] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn prehashed_setdefault_observes_reentrant_publication_and_lookup_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (probe, class, function) = same_hash_probe_key(py);
            let removed = string_key(py, b"removed");
            let query = string_key(py, b"inserted");
            let inner_key = string_key(py, b"inserted");
            let proposed = mortal_value(py);
            let dict = dict_with_same_hash_key(
                py,
                &[removed, MoltObject::none().bits()],
                probe,
                b"inserted",
            );
            let hash = dict_key_hash(py, query).unwrap();
            PROBE_DICT.store(MoltObject::from_ptr(dict).bits(), Ordering::SeqCst);
            PROBE_DELETE.store(removed, Ordering::SeqCst);
            PROBE_INSERT.store(inner_key, Ordering::SeqCst);
            PROBE_CALLS.store(0, Ordering::SeqCst);
            PROBE_MODE.store(PROBE_RESTRUCTURE, Ordering::SeqCst);
            let before = owners(proposed);
            let (actual, inserted) =
                dict_setdefault_with_hash_in_place(py, dict, query, Some(proposed), hash).unwrap();
            assert!(
                !inserted,
                "an equality callback already published the entry"
            );
            assert_eq!(actual, MoltObject::from_int(40).bits());
            dec_ref_bits(py, actual);
            assert_eq!(owners(proposed), before);
            assert_eq!(
                dict_find_entry_kv_with_hash(py, dict, query, hash).unwrap(),
                Some((inner_key, MoltObject::from_int(40).bits()))
            );
            PROBE_MODE.store(PROBE_RAISE, Ordering::SeqCst);
            assert!(
                dict_setdefault_with_hash_in_place(py, dict, query, Some(proposed), hash).is_none()
            );
            take_pending_error(py, "RuntimeError");
            assert_eq!(owners(proposed), before);
            assert_eq!(dict_len(dict), 2);
            PROBE_MODE.store(PROBE_UNEQUAL, Ordering::SeqCst);
            for bits in [
                MoltObject::from_ptr(dict).bits(),
                removed,
                query,
                inner_key,
                proposed,
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
fn dictionary_append_budget_controls_real_compaction_and_popitem_extent() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let int = |value| MoltObject::from_int(value).bits();
            let dict = alloc_dict_with_pairs(py, &[]);
            let entries_owner = dict_entries_ptr(dict);
            let table_owner = dict_table_ptr(dict);
            for key in 0..5 {
                assert!(dict_set_in_place(py, dict, int(key), int(key + 10)));
            }
            assert_eq!((dict_table(dict).len(), dict_storage(dict).usable), (8, 0));
            for key in [1, 3] {
                assert!(dict_del_in_place(py, dict, int(key)).unwrap());
            }
            assert_eq!((dict_entries(dict).len(), dict_len(dict)), (5, 3));
            assert_eq!(dict_storage(dict).usable, 0, "erasure refunds no appends");
            assert!(dict_set_in_place(py, dict, int(5), int(15)));
            assert_eq!(
                (
                    dict_table(dict).len(),
                    dict_entries(dict).len(),
                    dict_storage(dict).usable
                ),
                (16, 4, 6)
            );
            assert_eq!(
                dict_live_entries(dict)
                    .map(|row| row.key)
                    .collect::<Vec<_>>(),
                vec![int(0), int(2), int(4), int(5)]
            );
            assert_eq!(dict_entries_ptr(dict), entries_owner);
            assert_eq!(dict_table_ptr(dict), table_owner);
            drop(dict_clear_deferred(py, dict).unwrap());
            assert_eq!(
                (
                    dict_table(dict).len(),
                    dict_entries(dict).len(),
                    dict_storage(dict).usable
                ),
                (0, 0, 0)
            );
            assert_eq!(dict_storage(dict).kind, DictKeyKind::Unicode);
            for key in 0..4 {
                assert!(dict_set_in_place(py, dict, int(key), int(key + 10)));
            }
            assert!(dict_del_in_place(py, dict, int(2)).unwrap());
            let popped = dict_remove_last(py, dict).unwrap().into_owned_storage();
            assert_eq!(popped, [int(3), int(13)]);
            for bits in popped {
                dec_ref_bits(py, bits);
            }
            assert_eq!(
                (dict_entries(dict).len(), dict_storage(dict).usable),
                (3, 1)
            );
            assert!(dict_set_in_place(py, dict, int(4), int(14)));
            assert_eq!(
                (dict_entries(dict).len(), dict_storage(dict).usable),
                (4, 0)
            );
            assert!(dict_set_in_place(py, dict, int(5), int(15)));
            assert_eq!(
                (
                    dict_table(dict).len(),
                    dict_entries(dict).len(),
                    dict_storage(dict).usable
                ),
                (16, 4, 6)
            );
            assert_eq!(
                dict_live_entries(dict)
                    .map(|row| row.key)
                    .collect::<Vec<_>>(),
                vec![int(0), int(1), int(4), int(5)]
            );
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn dictionary_copy_preserves_dense_holes_and_compacts_sparse_sources_without_rehashing() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let int = |value| MoltObject::from_int(value).bits();
            let dict = alloc_dict_with_pairs(py, &[]);
            for key in 0..5 {
                assert!(dict_set_in_place(py, dict, int(key), int(key + 10)));
            }
            assert!(dict_del_in_place(py, dict, int(1)).unwrap());
            let original = MoltObject::from_ptr(dict).bits();
            let copied = crate::object::ops_dict::molt_dict_copy(original);
            let copy = obj_from_bits(copied).as_ptr().unwrap();
            assert_eq!(
                dict_entries(copy),
                dict_entries(dict),
                "dense copy retains physical holes"
            );
            assert_eq!((dict_table(copy).len(), dict_storage(copy).usable), (8, 0));
            assert_ne!(dict_entries_ptr(copy), dict_entries_ptr(dict));
            let constructed = crate::molt_dict_from_obj(original);
            let compact = obj_from_bits(constructed).as_ptr().unwrap();
            assert_eq!(
                (
                    dict_entries(compact).len(),
                    dict_table(compact).len(),
                    dict_storage(compact).usable
                ),
                (4, 8, 1)
            );
            for key in [2, 3] {
                assert!(dict_del_in_place(py, dict, int(key)).unwrap());
            }
            let sparse_copy = crate::object::ops_dict::molt_dict_copy(original);
            let sparse = obj_from_bits(sparse_copy).as_ptr().unwrap();
            assert_eq!(
                (
                    dict_entries(sparse).len(),
                    dict_table(sparse).len(),
                    dict_storage(sparse).usable
                ),
                (2, 8, 3)
            );
            assert_eq!(
                dict_live_entries(sparse)
                    .map(|row| row.key)
                    .collect::<Vec<_>>(),
                vec![int(0), int(4)]
            );
            for bits in [sparse_copy, constructed, copied, original] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn dictionary_general_key_conversion_reserves_before_compacting_or_publishing() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let keys = [
                string_key(py, b"first"),
                string_key(py, b"hole"),
                string_key(py, b"last"),
            ];
            let int = |value| MoltObject::from_int(value).bits();
            let dict =
                alloc_dict_with_pairs(py, &[keys[0], int(1), keys[1], int(2), keys[2], int(3)]);
            assert!(dict_del_in_place(py, dict, keys[1]).unwrap());
            let before = dict_entries(dict).clone();
            let epoch = dict_structural_epoch(dict);
            // Force the general-key conversion to grow from eight index
            // slots before denying its real accounted reservation.
            for (key, value) in [
                (string_key(py, b"fourth"), int(4)),
                (string_key(py, b"fifth"), int(5)),
            ] {
                assert!(dict_set_in_place(py, dict, key, value));
                dec_ref_bits(py, key);
            }
            assert_eq!(dict_storage(dict).kind, DictKeyKind::Unicode);
            let rows = dict_entries(dict).clone();
            let conversion_epoch = dict_structural_epoch(dict);
            let reset = TrackerReset;
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                max_allocations: Some(0),
                ..Default::default()
            })));
            assert!(!dict_set_in_place(py, dict, int(9), int(9)));
            crate::test_support::assert_and_clear_emergency_memory_error(py);
            drop(reset);
            assert_eq!(dict_entries(dict), &rows);
            assert_eq!(dict_structural_epoch(dict), conversion_epoch);
            assert_eq!(dict_storage(dict).kind, DictKeyKind::Unicode);
            assert!(dict_set_in_place(py, dict, int(9), int(9)));
            assert_eq!(dict_storage(dict).kind, DictKeyKind::General);
            assert_eq!(
                (
                    dict_table(dict).len(),
                    dict_entries(dict).len(),
                    dict_storage(dict).usable
                ),
                (16, 5, 5)
            );
            assert!(dict_structural_epoch(dict) > epoch);
            assert_eq!(before.len(), 3);
            dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
            for key in keys {
                dec_ref_bits(py, key);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn dictionary_full_pairs_prescan_and_fromkeys_share_real_capacity_policy() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let int = |value| MoltObject::from_int(value).bits();
            let keys = [int(1), int(2), int(3), int(4), int(5), int(5)];
            let values = [int(10), int(20), int(30), int(40), int(50), int(51)];
            let literal = crate::c_api::molt_dict_from_pairs(keys.as_ptr(), values.as_ptr(), 6);
            let dict = obj_from_bits(literal).as_ptr().unwrap();
            assert_eq!(dict_storage(dict).kind, DictKeyKind::General);
            assert_eq!(
                (
                    dict_table(dict).len(),
                    dict_len(dict),
                    dict_storage(dict).usable
                ),
                (16, 5, 5)
            );
            assert_eq!(
                dict_live_entries(dict)
                    .map(|row| row.value)
                    .collect::<Vec<_>>(),
                vec![int(10), int(20), int(30), int(40), int(51)]
            );
            assert!(dict_del_in_place(py, dict, int(2)).unwrap());
            assert!(dict_set_in_place(py, dict, int(6), int(60)));
            assert_eq!(
                (dict_entries(dict).len(), dict_storage(dict).usable),
                (6, 4)
            );
            // Exact dict fromkeys presizes from live cardinality, not source
            // extent; generic tuple fromkeys grows from the shared empty state.
            let cls = builtin_classes(py).dict;
            let exact = dict_fromkeys_method(cls, literal, int(99)) as u64;
            let exact_ptr = obj_from_bits(exact).as_ptr().unwrap();
            assert_eq!(
                (
                    dict_table(exact_ptr).len(),
                    dict_entries(exact_ptr).len(),
                    dict_storage(exact_ptr).usable
                ),
                (8, 5, 0)
            );
            let tuple = alloc_tuple(py, &keys);
            let tuple_bits = MoltObject::from_ptr(tuple).bits();
            let generic = dict_fromkeys_method(cls, tuple_bits, int(99)) as u64;
            let generic_ptr = obj_from_bits(generic).as_ptr().unwrap();
            assert_eq!(
                (
                    dict_table(generic_ptr).len(),
                    dict_len(generic_ptr),
                    dict_storage(generic_ptr).usable
                ),
                (8, 5, 0)
            );
            for bits in [generic, tuple_bits, exact, literal] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn dictionary_equal_nonstring_probe_obeys_selected_conversion_phase() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let modern = crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
            let int = |value| MoltObject::from_int(value).bits();
            let keys = [
                string_key(py, b"resident"),
                string_key(py, b"hole"),
                string_key(py, b"last"),
            ];
            let (probe, class, function) = same_hash_probe_key(py);
            for setdefault in [false, true] {
                let dict =
                    alloc_dict_with_pairs(py, &[keys[0], int(1), keys[1], int(2), keys[2], int(3)]);
                assert!(dict_del_in_place(py, dict, keys[1]).unwrap());
                let epoch = dict_structural_epoch(dict);
                PROBE_MODE.store(PROBE_EQUAL, Ordering::SeqCst);
                PROBE_CALLS.store(0, Ordering::SeqCst);
                let hash = hash_string_bytes(py, b"resident") as u64;
                if setdefault {
                    let (value, inserted) =
                        dict_setdefault_with_hash_in_place(py, dict, probe, Some(int(9)), hash)
                            .unwrap();
                    assert_eq!(value, int(1));
                    assert!(!inserted);
                    dec_ref_bits(py, value);
                } else {
                    assert!(dict_set_with_hash_in_place(py, dict, probe, int(9), hash));
                }
                assert_eq!(PROBE_CALLS.load(Ordering::SeqCst), 1);
                assert_eq!(
                    dict_storage(dict).kind,
                    if modern {
                        DictKeyKind::Unicode
                    } else {
                        DictKeyKind::General
                    }
                );
                assert_eq!(dict_entries(dict).len(), if modern { 3 } else { 2 });
                assert_eq!(dict_structural_epoch(dict) == epoch, modern);
                let rows: Vec<_> = dict_live_entries(dict).collect();
                assert_eq!(rows[0].key, keys[0]);
                assert_eq!(rows[0].value, if setdefault { int(1) } else { int(9) });
                assert_eq!(rows[1].key, keys[2]);
                dec_ref_bits(py, MoltObject::from_ptr(dict).bits());
            }
            for bits in [probe, class, function, keys[0], keys[1], keys[2]] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

static FROMKEYS_RETURN_DICT: AtomicU64 = AtomicU64::new(0);

extern "C" fn fromkeys_return_existing_dict(_class: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = FROMKEYS_RETURN_DICT.load(Ordering::SeqCst);
        inc_ref_bits(py, result);
        result
    })
}

#[test]
fn dictionary_fromkeys_nonempty_exact_constructor_preserves_live_capacity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let int = |value| MoltObject::from_int(value).bits();
            let target = alloc_dict_with_pairs(py, &[]);
            for key in 0..8 {
                assert!(dict_set_in_place(py, target, int(key), int(key)));
            }
            let capacity = dict_table(target).len();
            assert_eq!(capacity, 16);
            let target_bits = MoltObject::from_ptr(target).bits();
            FROMKEYS_RETURN_DICT.store(target_bits, Ordering::SeqCst);
            let name = string_key(py, b"ExistingFromKeysTarget");
            let class = crate::molt_class_new(name);
            crate::molt_class_set_base(class, builtin_classes(py).dict);
            let method = string_key(py, b"__new__");
            let function = alloc_runtime_function_obj(
                py,
                runtime_fn_addr(
                    "fromkeys_return_existing_dict",
                    fromkeys_return_existing_dict as *const (),
                ),
                1,
            );
            assert!(!function.is_null());
            let function = MoltObject::from_ptr(function).bits();
            crate::molt_set_attr_name(class, method, function);
            crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
            let source = alloc_dict_with_pairs(py, &[int(0), int(42)]);
            let source_bits = MoltObject::from_ptr(source).bits();
            let result = dict_fromkeys_method(class, source_bits, int(99)) as u64;
            assert!(!exception_pending(py));
            assert_eq!(result, target_bits);
            assert_eq!(dict_len(target), 8);
            assert_eq!(dict_table(target).len(), capacity);
            assert_eq!(
                dict_get_in_place(py, target, int(0)).unwrap(),
                Some(int(99))
            );
            assert_eq!(dict_get_in_place(py, target, int(7)).unwrap(), Some(int(7)));
            FROMKEYS_RETURN_DICT.store(0, Ordering::SeqCst);
            for bits in [
                result,
                source_bits,
                function,
                method,
                class,
                name,
                target_bits,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}
