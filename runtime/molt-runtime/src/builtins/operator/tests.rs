use super::{
    molt_operator_attrgetter_type, molt_operator_itemgetter_type, molt_operator_length_hint,
    molt_operator_methodcaller_type, operator_clear_runtime_state,
};
use crate::builtins::exceptions::{molt_exception_kind, molt_exception_last_pending};
use crate::{
    MoltObject, alloc_string, dec_ref_bits, exception_pending, obj_from_bits, runtime_state,
    string_obj_to_owned, to_i64,
};
use std::sync::atomic::Ordering;

#[test]
fn operator_type_caches_are_runtime_owned_and_clearable() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let state = runtime_state(py);
        operator_clear_runtime_state(py, state);
        let text = |name: &[u8]| {
            let ptr = alloc_string(py, name);
            assert!(!ptr.is_null());
            MoltObject::from_ptr(ptr).bits()
        };
        let attribute = |object, name: &[u8]| {
            let key = text(name);
            let value = crate::molt_getattr_builtin(object, key, MoltObject::none().bits());
            dec_ref_bits(py, key);
            assert!(!exception_pending(py));
            value
        };
        // Return borrowed identities backed by the owned class namespace, not
        // extra descriptor owners that could hide a lost class/result edge.
        let descriptors = |class| {
            [b"__call__".as_slice(), b"__init__"].map(|name| {
                let descriptor = attribute(class, name);
                assert_eq!(
                    crate::type_of_bits(py, descriptor),
                    crate::builtin_classes(py).wrapper_descriptor
                );
                let owner = attribute(descriptor, b"__objclass__");
                assert_eq!(owner, class);
                dec_ref_bits(py, owner);
                dec_ref_bits(py, descriptor);
                descriptor
            })
        };
        let exports: [extern "C" fn() -> u64; 3] = [
            molt_operator_itemgetter_type,
            molt_operator_attrgetter_type,
            molt_operator_methodcaller_type,
        ];
        let classes = exports.map(|export| export());
        let declared = classes.map(descriptors);
        let slots = state.operator.slots();
        for (class, slot) in classes.into_iter().zip(slots) {
            assert_eq!(slot.load(Ordering::Acquire), class);
        }

        // A cache hit reuses the canonical class and its namespace descriptors;
        // callback draining must neither replace them nor require private slots.
        super::operator_clear_runtime_callbacks(py, state);
        for ((export, class), expected) in exports.into_iter().zip(classes).zip(declared) {
            let repeated = export();
            assert_eq!(repeated, class);
            assert_eq!(descriptors(repeated), expected);
            dec_ref_bits(py, repeated);
        }

        let target_ptr = crate::alloc_tuple(py, &[MoltObject::from_int(73).bits()]);
        assert!(!target_ptr.is_null());
        let target = MoltObject::from_ptr(target_ptr).bits();
        let class_name = text(b"__class__");
        let len_name = text(b"__len__");
        let cases = [
            (
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(73).bits(),
            ),
            (class_name, crate::builtin_classes(py).tuple),
            (len_name, MoltObject::from_int(1).bits()),
        ];
        let exercise = |class, declared: [u64; 2], argument, expected| {
            let ptr = obj_from_bits(class).as_ptr().expect("owned operator type");
            assert_eq!(unsafe { crate::object_type_id(ptr) }, crate::TYPE_ID_TYPE);
            let instance = unsafe { crate::alloc_instance_for_class(py, ptr) };
            assert!(!exception_pending(py));
            assert_eq!(crate::type_of_bits(py, instance), class);
            let initialized = unsafe {
                crate::call::dispatch::call_callable2(py, declared[1], instance, argument)
            };
            assert!(!exception_pending(py));
            assert!(obj_from_bits(initialized).is_none());
            dec_ref_bits(py, initialized);
            let value =
                unsafe { crate::call::dispatch::call_callable2(py, declared[0], instance, target) };
            assert!(!exception_pending(py));
            assert_eq!(value, expected);
            dec_ref_bits(py, value);
            dec_ref_bits(py, instance);
        };
        for ((class, declared), (argument, expected)) in
            classes.into_iter().zip(declared).zip(cases)
        {
            exercise(class, declared, argument, expected);
        }

        operator_clear_runtime_state(py, state);
        for slot in slots {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
        // Caller-owned results retain the same working descriptor namespace
        // after every runtime cache owner has been released.
        for ((class, declared), (argument, expected)) in
            classes.into_iter().zip(declared).zip(cases)
        {
            assert_eq!(descriptors(class), declared);
            exercise(class, declared, argument, expected);
            dec_ref_bits(py, class);
        }
        for slot in slots {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
        for owned in [target, class_name, len_name] {
            dec_ref_bits(py, owned);
        }
    });
}

#[test]
fn operator_type_exports_own_results_across_alias_release_gc_and_cold_restart() {
    for _ in 0..2 {
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
            crate::with_gil_entry_nopanic!(py, {
                let state = runtime_state(py);
                let exports: [extern "C" fn() -> u64; 3] = [
                    molt_operator_itemgetter_type,
                    molt_operator_attrgetter_type,
                    molt_operator_methodcaller_type,
                ];
                let classes = exports.map(|export| {
                    let first = export();
                    assert!(!exception_pending(py));
                    let ptr = obj_from_bits(first).as_ptr().expect("operator type");
                    let refs =
                        || unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() };
                    let first_refs = refs();
                    let second = export();
                    assert_eq!(second, first);
                    assert_eq!(refs(), first_refs + 1, "each export owns a result");
                    dec_ref_bits(py, second);
                    assert_eq!(refs(), first_refs);

                    // Three namespace owners model _operator, operator and
                    // pickle publication of the same canonical class.
                    let key = alloc_string(py, b"exported_type");
                    assert!(!key.is_null());
                    let key = MoltObject::from_ptr(key).bits();
                    let aliases: [u64; 3] = std::array::from_fn(|_| {
                        let dict = crate::alloc_dict_with_pairs(py, &[key, first]);
                        assert!(!dict.is_null());
                        MoltObject::from_ptr(dict).bits()
                    });
                    assert_eq!(refs(), first_refs + 3);
                    dec_ref_bits(py, first);
                    assert_eq!(refs(), first_refs + 2);
                    for alias in aliases {
                        dec_ref_bits(py, alias);
                    }
                    dec_ref_bits(py, key);
                    assert_eq!(refs(), first_refs - 1, "cache keeps its own anchor");
                    first // Borrowed identity only; every exported owner is gone.
                });

                super::operator_clear_runtime_callbacks(py, state);
                assert_eq!(
                    unsafe { crate::object::gc::collect_cycles(py) }.status,
                    crate::object::gc::GcCollectStatus::Completed
                );
                for ((export, class), slot) in exports.into_iter().zip(classes).zip([
                    &state.operator.itemgetter_class,
                    &state.operator.attrgetter_class,
                    &state.operator.methodcaller_class,
                ]) {
                    assert_eq!(slot.load(Ordering::Acquire), class);
                    assert!(crate::object::class_storage::is_canonical_runtime_class(
                        py, class
                    ));
                    let owned = export();
                    assert_eq!(owned, class);
                    dec_ref_bits(py, owned);
                }
                assert!(!exception_pending(py));
            });
            // Production teardown must release the cache anchors exactly once
            // before the next independent runtime starts.
        });
    }
}

#[test]
fn operator_length_hint_validates_default_before_fallback() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();

        let negative = molt_operator_length_hint(none_bits, MoltObject::from_int(-1).bits());
        assert_eq!(to_i64(obj_from_bits(negative)), Some(-1));

        let truthy = molt_operator_length_hint(none_bits, MoltObject::from_bool(true).bits());
        assert_eq!(to_i64(obj_from_bits(truthy)), Some(1));

        let invalid_ptr = alloc_string(_py, b"x");
        assert!(!invalid_ptr.is_null());
        let invalid_bits = MoltObject::from_ptr(invalid_ptr).bits();
        let result = molt_operator_length_hint(none_bits, invalid_bits);
        dec_ref_bits(_py, invalid_bits);

        assert!(obj_from_bits(result).is_none());
        assert!(exception_pending(_py));
        let exc_bits = molt_exception_last_pending();
        let kind_bits = molt_exception_kind(exc_bits);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(kind_bits)).as_deref(),
            Some("TypeError")
        );
        dec_ref_bits(_py, kind_bits);
        dec_ref_bits(_py, exc_bits);
        crate::molt_exception_clear();
    });
}
