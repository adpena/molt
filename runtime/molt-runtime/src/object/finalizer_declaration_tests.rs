use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use std::sync::atomic::{AtomicU64, Ordering};

static BASE_FINALIZERS: AtomicU64 = AtomicU64::new(0);
static OWN_FINALIZERS: AtomicU64 = AtomicU64::new(0);
static REENTRANT_INSTANCE: AtomicU64 = AtomicU64::new(0);
static REENTRANT_ELIGIBLE: AtomicU64 = AtomicU64::new(0);
static FINALIZER_RAW_OBSERVER: AtomicU64 = AtomicU64::new(0);
static FINALIZER_BOUND_OBSERVER: AtomicU64 = AtomicU64::new(0);
static FINALIZER_RAW_COUNT_AT_BOUND: AtomicU64 = AtomicU64::new(0);
static MUTATING_FINALIZER_RAW: AtomicU64 = AtomicU64::new(0);
static MUTATING_FINALIZER_REPLACEMENT: AtomicU64 = AtomicU64::new(0);
static MUTATING_FINALIZER_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn inspect_namespace_commit(_weak: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let instance = REENTRANT_INSTANCE.swap(0, Ordering::SeqCst);
        let ptr = obj_from_bits(instance).as_ptr().unwrap();
        REENTRANT_ELIGIBLE.store(
            u64::from(unsafe { object_class_has_finalizer(py, ptr) }),
            Ordering::SeqCst,
        );
        dec_ref_bits(py, instance);
        MoltObject::none().bits()
    })
}

extern "C" fn record_base_finalizer(_self_bits: u64) -> u64 {
    BASE_FINALIZERS.fetch_add(1, Ordering::SeqCst);
    MoltObject::none().bits()
}

extern "C" fn record_own_finalizer(_self_bits: u64) -> u64 {
    OWN_FINALIZERS.fetch_add(1, Ordering::SeqCst);
    MoltObject::none().bits()
}

fn refcount(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

fn panic_after_raw_retention(
    py: &PyToken<'_>,
    stage: super::FinalizerResourceStage,
    raw_bits: u64,
    bound_bits: u64,
) {
    assert_eq!(stage, super::FinalizerResourceStage::RawRetained);
    assert_eq!(bound_bits, 0);
    inc_ref_bits(py, raw_bits);
    FINALIZER_RAW_OBSERVER.store(raw_bits, Ordering::SeqCst);
    panic!("injected finalizer panic after raw retention");
}

fn panic_after_bound_ownership(
    py: &PyToken<'_>,
    stage: super::FinalizerResourceStage,
    raw_bits: u64,
    bound_bits: u64,
) {
    assert_eq!(stage, super::FinalizerResourceStage::BoundOwned);
    assert_ne!(bound_bits, 0);
    FINALIZER_RAW_COUNT_AT_BOUND.store(refcount(raw_bits).into(), Ordering::SeqCst);
    inc_ref_bits(py, bound_bits);
    FINALIZER_BOUND_OBSERVER.store(bound_bits, Ordering::SeqCst);
    panic!("injected finalizer panic after bound ownership");
}

extern "C" fn mutate_own_finalizer_and_raise(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        MUTATING_FINALIZER_CALLS.fetch_add(1, Ordering::SeqCst);
        let raw_bits = MUTATING_FINALIZER_RAW.load(Ordering::SeqCst);
        assert_ne!(raw_bits, 0);
        // Keep one test-owned observer after the class slot, bound method, and
        // finalizer transaction release their independent strong edges.
        inc_ref_bits(py, raw_bits);
        FINALIZER_RAW_OBSERVER.store(raw_bits, Ordering::SeqCst);

        let self_ptr = obj_from_bits(self_bits)
            .as_ptr()
            .expect("finalized instance");
        let class_bits = unsafe { object_class_bits(self_ptr) };
        let del_bits = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let replacement = MUTATING_FINALIZER_REPLACEMENT.load(Ordering::SeqCst);
        if replacement == 0 {
            crate::molt_del_attr_name(class_bits, del_bits);
        } else {
            crate::molt_set_attr_name(class_bits, del_bits, replacement);
        }
        dec_ref_bits(py, del_bits);
        assert!(!crate::exception_pending(py));
        crate::raise_exception::<u64>(py, "RuntimeError", "mutating finalizer failure")
    })
}

fn child_class(py: &PyToken<'_>, name: &[u8], base: u64) -> u64 {
    let name_bits = crate::attr_name_bits_from_bytes(py, name).unwrap();
    let class = crate::molt_class_new(name_bits);
    crate::molt_class_set_base(class, base);
    let ptr = obj_from_bits(class).as_ptr().unwrap();
    unsafe { class_finish_definition(py, ptr) }.expect("seal class");
    dec_ref_bits(py, name_bits);
    assert!(!crate::exception_pending(py));
    class
}

fn instance(py: &PyToken<'_>, class: u64) -> u64 {
    let class_ptr = obj_from_bits(class).as_ptr().unwrap();
    let size = unsafe { layout::class_cached_layout_size(class_ptr) }.unwrap();
    let bits = builders::alloc_class_instance(py, size, class);
    let ptr = obj_from_bits(bits).as_ptr().unwrap();
    unsafe { gc::gc_publish_initialized(py, ptr) };
    bits
}

fn finalizer(py: &PyToken<'_>, own: bool) -> u64 {
    let (name, target) = if own {
        (
            "own_declaration_finalizer",
            record_own_finalizer as extern "C" fn(u64) -> u64,
        )
    } else {
        (
            "base_declaration_finalizer",
            record_base_finalizer as extern "C" fn(u64) -> u64,
        )
    };
    let ptr = alloc_runtime_function_obj(py, runtime_fn_addr(name, target as *const ()), 1);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

#[test]
fn late_base_finalizers_follow_current_mro_without_descendant_flag_copies() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        BASE_FINALIZERS.store(0, Ordering::SeqCst);
        OWN_FINALIZERS.store(0, Ordering::SeqCst);
        let base = child_class(py, b"FinalizerBase", crate::builtin_classes(py).object);
        let child = child_class(py, b"FinalizerChild", base);
        let leaf = child_class(py, b"FinalizerLeaf", child);
        let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let base_del = finalizer(py, false);
        let own_del = finalizer(py, true);
        let preexisting = instance(py, leaf);
        assert!(!unsafe {
            object_class_has_finalizer(py, obj_from_bits(preexisting).as_ptr().unwrap())
        });

        crate::molt_set_attr_name(base, del, base_del);
        assert!(unsafe { class_header_declares_finalizer(obj_from_bits(base).as_ptr().unwrap()) });
        for descendant in [child, leaf] {
            assert!(!unsafe {
                class_header_declares_finalizer(obj_from_bits(descendant).as_ptr().unwrap())
            });
        }
        assert!(unsafe {
            object_class_has_finalizer(py, obj_from_bits(preexisting).as_ptr().unwrap())
        });
        dec_ref_bits(py, preexisting);
        assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);

        let removed = instance(py, leaf);
        crate::molt_del_attr_name(base, del);
        assert!(!unsafe {
            object_class_has_finalizer(py, obj_from_bits(removed).as_ptr().unwrap())
        });
        dec_ref_bits(py, removed);
        assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);

        crate::molt_set_attr_name(base, del, base_del);
        crate::molt_set_attr_name(child, del, own_del);
        let overridden = instance(py, leaf);
        dec_ref_bits(py, overridden);
        assert_eq!(OWN_FINALIZERS.load(Ordering::SeqCst), 1);
        assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);
        crate::molt_del_attr_name(child, del);
        dec_ref_bits(py, instance(py, leaf));
        assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 2);
        crate::molt_del_attr_name(base, del);

        for bits in [leaf, child, base, del, base_del, own_del] {
            dec_ref_bits(py, bits);
        }
        assert!(!crate::exception_pending(py));
    });
}

#[test]
fn cyclic_finalization_uses_late_inherited_declarations() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        BASE_FINALIZERS.store(0, Ordering::SeqCst);
        let base = child_class(py, b"CycleFinalizerBase", crate::builtin_classes(py).object);
        let child = child_class(py, b"CycleFinalizerChild", base);
        let value = instance(py, child);
        let peer = crate::attr_name_bits_from_bytes(py, b"peer").unwrap();
        crate::molt_set_attr_name(value, peer, value);
        let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let callback = finalizer(py, false);
        crate::molt_set_attr_name(base, del, callback);
        assert!(!crate::exception_pending(py));
        dec_ref_bits(py, value);
        crate::molt_gc_collect(MoltObject::from_int(2).bits());
        assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);
        crate::molt_del_attr_name(base, del);
        for bits in [child, base, peer, del, callback] {
            dec_ref_bits(py, bits);
        }
        assert!(!crate::exception_pending(py));
    });
}

#[test]
fn displaced_namespace_values_release_after_finalizer_metadata_commit() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for remove in [false, true] {
            BASE_FINALIZERS.store(0, Ordering::SeqCst);
            OWN_FINALIZERS.store(0, Ordering::SeqCst);
            let base = child_class(
                py,
                b"ReentrantNamespaceBase",
                crate::builtin_classes(py).object,
            );
            let child = child_class(py, b"ReentrantNamespaceChild", base);
            let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
            let old = finalizer(py, true);
            crate::molt_set_attr_name(base, del, old);
            let callback = MoltObject::from_ptr(alloc_runtime_function_obj(
                py,
                runtime_fn_addr(
                    "inspect_namespace_commit",
                    inspect_namespace_commit as *const (),
                ),
                1,
            ))
            .bits();
            let weak_type = crate::molt_weakref_reference_type();
            let weak = crate::molt_weakref_new(weak_type, old, callback);
            dec_ref_bits(py, weak_type);
            dec_ref_bits(py, old);
            REENTRANT_INSTANCE.store(instance(py, child), Ordering::SeqCst);
            REENTRANT_ELIGIBLE.store(2, Ordering::SeqCst);
            let replacement = finalizer(py, false);
            if remove {
                crate::molt_del_attr_name(base, del);
            } else {
                crate::molt_set_attr_name(base, del, replacement);
            }
            assert_eq!(REENTRANT_INSTANCE.load(Ordering::SeqCst), 0);
            assert_eq!(
                REENTRANT_ELIGIBLE.load(Ordering::SeqCst),
                u64::from(!remove)
            );
            assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), u64::from(!remove));
            assert_eq!(OWN_FINALIZERS.load(Ordering::SeqCst), 0);
            assert!(!crate::exception_pending(py));
            for bits in [child, base, del, replacement, weak, callback] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

fn dynamic_class(py: &PyToken<'_>, name: &[u8], metaclass: u64, namespace: u64, bases: u64) -> u64 {
    let name_bits = crate::attr_name_bits_from_bytes(py, name).unwrap();
    let result = crate::molt_type_new(
        metaclass,
        name_bits,
        bases,
        namespace,
        MoltObject::none().bits(),
    );
    dec_ref_bits(py, name_bits);
    result
}

#[test]
fn metaclass_finalizers_follow_late_mro_mutation_for_type_payloads() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        BASE_FINALIZERS.store(0, Ordering::SeqCst);
        OWN_FINALIZERS.store(0, Ordering::SeqCst);
        let meta = child_class(py, b"FinalizingMeta", crate::builtin_classes(py).type_obj);
        let derived = child_class(py, b"DerivedFinalizingMeta", meta);
        let namespace = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
        let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let base_del = finalizer(py, false);
        let own_del = finalizer(py, true);
        let preexisting = dynamic_class(
            py,
            b"PreexistingType",
            derived,
            namespace,
            MoltObject::none().bits(),
        );
        assert!(!crate::exception_pending(py));
        assert_eq!(
            unsafe { object_type_id(obj_from_bits(preexisting).as_ptr().unwrap()) },
            TYPE_ID_TYPE
        );
        assert!(!unsafe {
            object_class_has_finalizer(py, obj_from_bits(preexisting).as_ptr().unwrap())
        });
        crate::molt_set_attr_name(meta, del, base_del);
        assert!(unsafe {
            object_class_has_finalizer(py, obj_from_bits(preexisting).as_ptr().unwrap())
        });
        assert!(!unsafe {
            class_header_declares_finalizer(obj_from_bits(derived).as_ptr().unwrap())
        });
        dec_ref_bits(py, preexisting);
        crate::molt_gc_collect(MoltObject::from_int(2).bits());
        assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);

        let removed = dynamic_class(
            py,
            b"RemovedType",
            derived,
            namespace,
            MoltObject::none().bits(),
        );
        crate::molt_del_attr_name(meta, del);
        dec_ref_bits(py, removed);
        crate::molt_gc_collect(MoltObject::from_int(2).bits());
        assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);

        crate::molt_set_attr_name(meta, del, base_del);
        crate::molt_set_attr_name(derived, del, own_del);
        let overridden = dynamic_class(
            py,
            b"OverriddenType",
            derived,
            namespace,
            MoltObject::none().bits(),
        );
        dec_ref_bits(py, overridden);
        crate::molt_gc_collect(MoltObject::from_int(2).bits());
        assert_eq!(OWN_FINALIZERS.load(Ordering::SeqCst), 1);
        assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);
        crate::molt_del_attr_name(derived, del);
        crate::molt_del_attr_name(meta, del);
        for bits in [namespace, derived, meta, del, base_del, own_del] {
            dec_ref_bits(py, bits);
        }
        assert!(!crate::exception_pending(py));
    });
}

#[test]
fn type_construction_finalizes_only_after_preallocation_admission() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        BASE_FINALIZERS.store(0, Ordering::SeqCst);
        let meta = child_class(
            py,
            b"ConstructionFinalizingMeta",
            crate::builtin_classes(py).type_obj,
        );
        let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let callback = finalizer(py, false);
        crate::molt_set_attr_name(meta, del, callback);
        let slots = crate::attr_name_bits_from_bytes(py, b"__slots__").unwrap();
        let qualname = crate::attr_name_bits_from_bytes(py, b"__qualname__").unwrap();
        let bad_slots =
            MoltObject::from_ptr(crate::alloc_tuple(py, &[MoltObject::from_int(42).bits()])).bits();
        let base = child_class(
            py,
            b"DuplicateConstructionBase",
            crate::builtin_classes(py).object,
        );
        let duplicate_bases = MoltObject::from_ptr(crate::alloc_tuple(py, &[base, base])).bits();
        let nonbase =
            MoltObject::from_ptr(crate::alloc_tuple(py, &[MoltObject::from_int(42).bits()])).bits();
        for (key, value, bases, expected) in [
            (slots, bad_slots, MoltObject::none().bits(), 0),
            (
                qualname,
                MoltObject::from_int(42).bits(),
                MoltObject::none().bits(),
                1,
            ),
            (qualname, MoltObject::none().bits(), nonbase, 1),
            (
                slots,
                MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits(),
                duplicate_bases,
                2,
            ),
        ] {
            let namespace =
                MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[key, value])).bits();
            let result = dynamic_class(py, b"RejectedType", meta, namespace, bases);
            assert!(obj_from_bits(result).is_none());
            assert!(crate::exception_pending(py));
            crate::molt_exception_clear();
            crate::molt_gc_collect(MoltObject::from_int(2).bits());
            assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), expected);
            dec_ref_bits(py, namespace);
            if bases == duplicate_bases {
                dec_ref_bits(py, value);
            }
        }
        crate::molt_del_attr_name(meta, del);
        for bits in [
            meta,
            del,
            callback,
            slots,
            qualname,
            bad_slots,
            base,
            duplicate_bases,
            nonbase,
        ] {
            dec_ref_bits(py, bits);
        }
        assert!(!crate::exception_pending(py));
    });
}

#[test]
fn builtin_spelled_class_and_metaclass_names_do_not_suppress_finalizers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let callback = finalizer(py, false);
        let namespace = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
        for name in [b"frame".as_slice(), b"traceback".as_slice()] {
            BASE_FINALIZERS.store(0, Ordering::SeqCst);
            let ordinary = child_class(py, name, crate::builtin_classes(py).object);
            crate::molt_set_attr_name(ordinary, del, callback);
            let value = instance(py, ordinary);
            assert!(unsafe {
                object_class_has_finalizer(py, obj_from_bits(value).as_ptr().unwrap())
            });
            dec_ref_bits(py, value);
            assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);
            crate::molt_del_attr_name(ordinary, del);
            dec_ref_bits(py, ordinary);

            BASE_FINALIZERS.store(0, Ordering::SeqCst);
            let meta = child_class(py, name, crate::builtin_classes(py).type_obj);
            crate::molt_set_attr_name(meta, del, callback);
            let class = dynamic_class(
                py,
                b"NamedMetaclassInstance",
                meta,
                namespace,
                MoltObject::none().bits(),
            );
            assert!(!crate::exception_pending(py));
            assert!(unsafe {
                object_class_has_finalizer(py, obj_from_bits(class).as_ptr().unwrap())
            });
            dec_ref_bits(py, class);
            crate::molt_gc_collect(MoltObject::from_int(2).bits());
            assert_eq!(BASE_FINALIZERS.load(Ordering::SeqCst), 1);
            crate::molt_del_attr_name(meta, del);
            dec_ref_bits(py, meta);
        }
        for bits in [del, callback, namespace] {
            dec_ref_bits(py, bits);
        }
        assert!(!crate::exception_pending(py));
    });
}

#[test]
fn finalizer_raw_owner_releases_when_callback_panics_before_binding() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        FINALIZER_RAW_OBSERVER.store(0, Ordering::SeqCst);
        let class = child_class(
            py,
            b"RawOwnerPanicFinalizer",
            crate::builtin_classes(py).object,
        );
        let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let callback = finalizer(py, false);
        crate::molt_set_attr_name(class, del, callback);
        let baseline = refcount(callback);
        let value = instance(py, class);
        let value_ptr = obj_from_bits(value).as_ptr().unwrap();
        super::FINALIZER_RESOURCE_TEST_HOOK.with(|slot| {
            slot.set(Some((
                super::FinalizerResourceStage::RawRetained,
                panic_after_raw_retention,
            )));
        });

        let outcome = crate::test_support::catch_expected_unwind(|| unsafe {
            run_object_del_in_revival_window(py, value_ptr);
        });
        assert!(outcome.is_err());
        assert_eq!(FINALIZER_RAW_OBSERVER.load(Ordering::SeqCst), callback);
        assert_eq!(
            refcount(callback),
            baseline + 1,
            "only the injected observer may remain above the class/test baseline"
        );
        dec_ref_bits(py, callback);
        assert_eq!(refcount(callback), baseline);

        crate::molt_del_attr_name(class, del);
        for bits in [value, callback, class, del] {
            dec_ref_bits(py, bits);
        }
        assert!(!crate::exception_pending(py));
    });
}

#[test]
fn finalizer_bound_owner_and_exception_scope_restore_on_panic() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        FINALIZER_BOUND_OBSERVER.store(0, Ordering::SeqCst);
        FINALIZER_RAW_COUNT_AT_BOUND.store(0, Ordering::SeqCst);
        let class = child_class(
            py,
            b"BoundOwnerPanicFinalizer",
            crate::builtin_classes(py).object,
        );
        let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let callback = finalizer(py, false);
        crate::molt_set_attr_name(class, del, callback);
        let callback_baseline = refcount(callback);
        let value = instance(py, class);
        let value_ptr = obj_from_bits(value).as_ptr().unwrap();
        let value_baseline = refcount(value);

        let entry_depth = crate::builtins::exceptions::exception_stack_depth();
        crate::builtins::exceptions::exception_stack_push();
        crate::builtins::exceptions::exception_stack_push();
        let prior_depth = crate::builtins::exceptions::exception_stack_depth();
        let _ = crate::raise_exception::<u64>(py, "ValueError", "prior finalizer exception");
        let prior_exception =
            crate::builtins::exceptions::exception_last_bits_noinc(py).expect("prior exception");
        super::FINALIZER_RESOURCE_TEST_HOOK.with(|slot| {
            slot.set(Some((
                super::FinalizerResourceStage::BoundOwned,
                panic_after_bound_ownership,
            )));
        });

        let outcome = crate::test_support::catch_expected_unwind(|| unsafe {
            run_object_del_in_revival_window(py, value_ptr);
        });
        assert!(outcome.is_err());
        assert_eq!(
            crate::builtins::exceptions::exception_stack_depth(),
            prior_depth,
            "synthetic finalizer frame must unwind to the exact entry depth"
        );
        assert_eq!(
            crate::builtins::exceptions::exception_last_bits_noinc(py),
            Some(prior_exception),
            "unraisable transaction must restore the exact prior exception"
        );

        let bound = FINALIZER_BOUND_OBSERVER.swap(0, Ordering::SeqCst);
        assert_ne!(bound, 0);
        assert_eq!(
            refcount(bound),
            1,
            "only the injected bound observer remains"
        );
        assert_eq!(
            FINALIZER_RAW_COUNT_AT_BOUND.load(Ordering::SeqCst),
            u64::from(callback_baseline + 2),
            "raw transaction owner and bound-method edge are both live at the hook"
        );
        assert_eq!(
            refcount(callback),
            callback_baseline + 1,
            "raw transaction owner released while observed bound method retains callable"
        );
        assert_eq!(refcount(value), value_baseline + 1);
        dec_ref_bits(py, bound);
        assert_eq!(refcount(callback), callback_baseline);
        assert_eq!(refcount(value), value_baseline);

        crate::clear_exception(py);
        crate::builtins::exceptions::exception_stack_set_depth(py, entry_depth);
        crate::molt_del_attr_name(class, del);
        for bits in [value, callback, class, del] {
            dec_ref_bits(py, bits);
        }
        assert!(!crate::exception_pending(py));
    });
}

#[test]
fn finalizer_raw_callable_survives_self_deletion_and_replacement_through_reporting() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for replace in [false, true] {
            FINALIZER_RAW_OBSERVER.store(0, Ordering::SeqCst);
            MUTATING_FINALIZER_CALLS.store(0, Ordering::SeqCst);
            let class = child_class(
                py,
                if replace {
                    b"ReplacingOwnFinalizer".as_slice()
                } else {
                    b"DeletingOwnFinalizer".as_slice()
                },
                crate::builtin_classes(py).object,
            );
            let del = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
            let raw_ptr = alloc_runtime_function_obj(
                py,
                runtime_fn_addr(
                    "mutate_own_finalizer_and_raise",
                    mutate_own_finalizer_and_raise as *const (),
                ),
                1,
            );
            assert!(!raw_ptr.is_null());
            let raw = MoltObject::from_ptr(raw_ptr).bits();
            let replacement = finalizer(py, false);
            crate::molt_set_attr_name(class, del, raw);
            MUTATING_FINALIZER_RAW.store(raw, Ordering::SeqCst);
            MUTATING_FINALIZER_REPLACEMENT
                .store(if replace { replacement } else { 0 }, Ordering::SeqCst);
            // The class slot is deliberately the only pre-finalizer owner.
            dec_ref_bits(py, raw);

            dec_ref_bits(py, instance(py, class));
            assert_eq!(MUTATING_FINALIZER_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(FINALIZER_RAW_OBSERVER.load(Ordering::SeqCst), raw);
            assert_eq!(
                refcount(raw),
                1,
                "self-mutation must leave only the callback's test observer"
            );
            dec_ref_bits(py, raw);

            if replace {
                crate::molt_del_attr_name(class, del);
            }
            for bits in [replacement, class, del] {
                dec_ref_bits(py, bits);
            }
            assert!(!crate::exception_pending(py));
        }
        MUTATING_FINALIZER_RAW.store(0, Ordering::SeqCst);
        MUTATING_FINALIZER_REPLACEMENT.store(0, Ordering::SeqCst);
    });
}
