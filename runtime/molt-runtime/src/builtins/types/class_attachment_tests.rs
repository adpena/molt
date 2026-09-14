use super::*;
use crate::builtins::attributes::molt_set_attr_name;
use crate::object::accessors::{molt_object_field_get, molt_object_field_set};
use crate::object::builders::{molt_alloc_class, molt_object_publish_initialized};

unsafe fn new_class_with_base(_py: &PyToken<'_>, name: &[u8], base: u64) -> u64 {
    let name_bits = attr_name_bits_from_bytes(_py, name).expect("class name");
    let class_bits = molt_class_new(name_bits);
    dec_ref_bits(_py, name_bits);
    assert!(obj_from_bits(class_bits).as_ptr().is_some());
    assert_eq!(
        molt_class_set_base(class_bits, base),
        MoltObject::none().bits()
    );
    assert!(!exception_pending(_py));
    class_bits
}

unsafe fn new_slotted_class(_py: &PyToken<'_>, name: &[u8], slot: &[u8]) -> u64 {
    unsafe { new_slotted_class_with_base(_py, name, slot, builtin_classes(_py).object) }
}

unsafe fn new_slotted_class_with_base(
    _py: &PyToken<'_>,
    name: &[u8],
    slot: &[u8],
    base: u64,
) -> u64 {
    let class_bits = unsafe { new_class_with_base(_py, name, base) };
    let slots_name = attr_name_bits_from_bytes(_py, b"__slots__").expect("slots name");
    let slot_name = attr_name_bits_from_bytes(_py, slot).expect("slot name");
    molt_set_attr_name(class_bits, slots_name, slot_name);
    dec_ref_bits(_py, slot_name);
    dec_ref_bits(_py, slots_name);
    assert!(!exception_pending(_py));
    class_bits
}

fn set_class_via_attribute(_py: &PyToken<'_>, object_bits: u64, class_bits: u64) -> u64 {
    let class_name = attr_name_bits_from_bytes(_py, b"__class__").expect("class attribute");
    let result = molt_set_attr_name(object_bits, class_name, class_bits);
    dec_ref_bits(_py, class_name);
    result
}

#[test]
fn compatible_slot_class_reassignment_preserves_exact_scalar_bits() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let first = unsafe { new_slotted_class(_py, b"SlotFirst", b"value") };
        let second = unsafe { new_slotted_class(_py, b"SlotSecond", b"value") };
        let first_ptr = obj_from_bits(first).as_ptr().expect("first class");
        let second_ptr = obj_from_bits(second).as_ptr().expect("second class");
        unsafe { crate::object::class_finish_definition(_py, first_ptr) }
            .expect("seal first class");
        unsafe { crate::object::class_finish_definition(_py, second_ptr) }
            .expect("seal second class");
        let size = unsafe { crate::object::layout::class_cached_layout_size(first_ptr) }
            .expect("sealed size");
        let object = molt_alloc_class(size as u64, first);
        let object_ptr = obj_from_bits(object).as_ptr().expect("instance");
        assert_eq!(molt_object_publish_initialized(object), object);
        let negative_zero = MoltObject::from_float(-0.0).bits();
        assert_eq!(
            unsafe { molt_object_field_set(object, 0, negative_zero) },
            MoltObject::none().bits()
        );

        assert_eq!(
            set_class_via_attribute(_py, object, second),
            MoltObject::none().bits()
        );
        assert!(!exception_pending(_py));
        assert_eq!(unsafe { object_class_bits(object_ptr) }, second);
        assert_eq!(unsafe { molt_object_field_get(object, 0) }, negative_zero);

        dec_ref_bits(_py, object);
        dec_ref_bits(_py, second);
        dec_ref_bits(_py, first);
        assert!(!exception_pending(_py));
    });
}

#[test]
fn incompatible_slot_names_reject_without_changing_class_or_fields() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let first = unsafe { new_slotted_class(_py, b"SlotAlpha", b"alpha") };
        let second = unsafe { new_slotted_class(_py, b"SlotBeta", b"beta") };
        let first_ptr = obj_from_bits(first).as_ptr().expect("first class");
        unsafe { crate::object::class_finish_definition(_py, first_ptr) }
            .expect("seal first class");
        let size = unsafe { crate::object::layout::class_cached_layout_size(first_ptr) }
            .expect("sealed size");
        let object = molt_alloc_class(size as u64, first);
        let object_ptr = obj_from_bits(object).as_ptr().expect("instance");
        assert_eq!(molt_object_publish_initialized(object), object);
        let negative_zero = MoltObject::from_float(-0.0).bits();
        assert_eq!(
            unsafe { molt_object_field_set(object, 0, negative_zero) },
            MoltObject::none().bits()
        );

        assert_eq!(
            set_class_via_attribute(_py, object, second),
            MoltObject::none().bits()
        );
        assert!(exception_pending(_py));
        assert_eq!(unsafe { object_class_bits(object_ptr) }, first);
        crate::molt_exception_clear();
        assert_eq!(unsafe { molt_object_field_get(object, 0) }, negative_zero);

        dec_ref_bits(_py, object);
        dec_ref_bits(_py, second);
        dec_ref_bits(_py, first);
        assert!(!exception_pending(_py));
    });
}

#[test]
fn class_assignment_respects_data_descriptor_in_public_and_explicit_setters() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let first = unsafe { new_slotted_class(_py, b"DescriptorFirst", b"value") };
        let second = unsafe { new_slotted_class(_py, b"DescriptorSecond", b"value") };
        let class_name = attr_name_bits_from_bytes(_py, b"__class__").expect("class attribute");
        let none = MoltObject::none().bits();
        let property_ptr = alloc_property_obj(_py, none, none, none);
        assert!(!property_ptr.is_null());
        let property = MoltObject::from_ptr(property_ptr).bits();
        assert_eq!(molt_set_attr_name(first, class_name, property), none);
        assert!(!exception_pending(_py));

        let first_ptr = obj_from_bits(first).as_ptr().expect("first class");
        unsafe { crate::object::class_finish_definition(_py, first_ptr) }
            .expect("seal first class");
        let size = unsafe { crate::object::layout::class_cached_layout_size(first_ptr) }
            .expect("sealed size");
        let object = molt_alloc_class(size as u64, first);
        let object_ptr = obj_from_bits(object).as_ptr().expect("instance");
        assert_eq!(molt_object_publish_initialized(object), object);

        assert_eq!(molt_set_attr_name(object, class_name, second), none);
        assert!(exception_pending(_py));
        assert_eq!(unsafe { object_class_bits(object_ptr) }, first);
        crate::molt_exception_clear();

        assert_eq!(molt_object_setattr(object, class_name, second), none);
        assert!(exception_pending(_py));
        assert_eq!(unsafe { object_class_bits(object_ptr) }, first);
        crate::molt_exception_clear();

        dec_ref_bits(_py, object);
        dec_ref_bits(_py, property);
        dec_ref_bits(_py, class_name);
        dec_ref_bits(_py, second);
        dec_ref_bits(_py, first);
        assert!(!exception_pending(_py));
    });
}

#[test]
fn tuple_subclass_construction_is_always_fresh_and_never_retags_empty_singleton() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let class_bits =
            unsafe { new_class_with_base(_py, b"TupleChild", builtin_classes(_py).tuple) };
        let missing = missing_bits(_py);
        let first = molt_tuple_new_bound(class_bits, missing);
        let second = molt_tuple_new_bound(class_bits, missing);
        let first_ptr = obj_from_bits(first).as_ptr().expect("first tuple child");
        let second_ptr = obj_from_bits(second).as_ptr().expect("second tuple child");
        let empty_ptr = alloc_tuple(_py, &[]);
        assert_ne!(first_ptr, second_ptr);
        assert_ne!(first_ptr, empty_ptr);
        assert_ne!(second_ptr, empty_ptr);
        assert_eq!(unsafe { object_class_bits(first_ptr) }, class_bits);
        assert_eq!(unsafe { object_class_bits(second_ptr) }, class_bits);
        assert_eq!(unsafe { object_class_bits(empty_ptr) }, 0);

        let descendant = unsafe { new_class_with_base(_py, b"TupleGrandchild", class_bits) };
        let inherited = molt_tuple_new_bound(descendant, missing);
        let inherited_ptr = obj_from_bits(inherited)
            .as_ptr()
            .expect("inherited tuple child");
        assert_ne!(inherited_ptr, empty_ptr);
        assert_eq!(unsafe { object_class_bits(inherited_ptr) }, descendant);

        let source_ptr = alloc_tuple(_py, &[MoltObject::from_int(7).bits()]);
        let source = MoltObject::from_ptr(source_ptr).bits();
        let populated = molt_tuple_new_bound(class_bits, source);
        let copied = molt_tuple_new_bound(class_bits, populated);
        let populated_ptr = obj_from_bits(populated)
            .as_ptr()
            .expect("populated tuple child");
        let copied_ptr = obj_from_bits(copied).as_ptr().expect("copied tuple child");
        assert_ne!(populated_ptr, source_ptr);
        assert_ne!(copied_ptr, populated_ptr);
        assert_eq!(unsafe { object_class_bits(copied_ptr) }, class_bits);
        assert_eq!(
            unsafe { with_immutable_tuple_slice(copied_ptr, |items| items.to_vec()) },
            Some(vec![MoltObject::from_int(7).bits()])
        );

        dec_ref_bits(_py, copied);
        dec_ref_bits(_py, populated);
        dec_ref_bits(_py, source);
        dec_ref_bits(_py, inherited);
        dec_ref_bits(_py, descendant);
        dec_ref_bits(_py, second);
        dec_ref_bits(_py, first);
        dec_ref_bits(_py, class_bits);
        assert!(!exception_pending(_py));
    });
}

#[test]
fn tuple_sequence_descriptors_reject_other_physical_receivers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let list =
            MoltObject::from_ptr(crate::alloc_list(py, &[MoltObject::from_int(7).bits()])).bits();
        for receiver in [
            list,
            MoltObject::none().bits(),
            MoltObject::from_int(1).bits(),
        ] {
            for (name, binary) in [
                ("__iter__", false),
                ("__len__", false),
                ("__getitem__", true),
                ("__contains__", true),
            ] {
                let method = crate::builtins::containers::tuple_method_bits(py, name).unwrap();
                let result = unsafe {
                    if binary {
                        crate::call_callable2(py, method, receiver, MoltObject::from_int(0).bits())
                    } else {
                        crate::call_callable1(py, method, receiver)
                    }
                };
                assert!(obj_from_bits(result).is_none(), "{name}: invalid receiver");
                assert!(
                    exception_pending(py),
                    "{name}: descriptor receiver admission"
                );
                let exception = crate::molt_exception_last();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    exception,
                    "TypeError",
                ));
                crate::molt_exception_clear();
                dec_ref_bits(py, exception);
            }
        }
        dec_ref_bits(py, list);
    });
}

#[test]
fn tuple_sequence_methods_survive_runtime_reinitialization() {
    for _ in 0..2 {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let class = unsafe {
                    new_class_with_base(py, b"TupleSequenceChild", builtin_classes(py).tuple)
                };
                let seven = MoltObject::from_int(7).bits();
                let tuple =
                    unsafe { crate::object::builders::alloc_tuple_subclass(py, class, &[seven]) };
                assert!(!exception_pending(py));
                for (name, args, expected) in [
                    ("__len__", vec![], MoltObject::from_int(1).bits()),
                    ("__getitem__", vec![MoltObject::from_int(0).bits()], seven),
                    (
                        "__contains__",
                        vec![seven],
                        MoltObject::from_bool(true).bits(),
                    ),
                    ("count", vec![seven], MoltObject::from_int(1).bits()),
                    (
                        "index",
                        vec![seven, missing_bits(py), missing_bits(py)],
                        MoltObject::from_int(0).bits(),
                    ),
                ] {
                    let name_bits = attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
                    let method = crate::molt_get_attr_name(tuple, name_bits);
                    assert!(!exception_pending(py), "{name}: lookup");
                    let result = unsafe {
                        match args.as_slice() {
                            [] => crate::call_callable0(py, method),
                            [arg] => crate::call_callable1(py, method, *arg),
                            [a, b, c] => crate::call_callable3(py, method, *a, *b, *c),
                            _ => unreachable!(),
                        }
                    };
                    assert!(!exception_pending(py), "{name}: call");
                    assert_eq!(result, expected, "{name}");
                    for bits in [result, method, name_bits] {
                        dec_ref_bits(py, bits);
                    }
                }
                let copied = molt_tuple_new_bound(builtin_classes(py).tuple, tuple);
                assert!(!exception_pending(py));
                let ptr = obj_from_bits(copied).as_ptr().expect("exact tuple copy");
                assert_eq!(unsafe { object_class_bits(ptr) }, 0);
                assert_eq!(
                    unsafe { with_immutable_tuple_slice(ptr, |items| items.to_vec()) },
                    Some(vec![seven]),
                );
                for bits in [copied, tuple, class] {
                    dec_ref_bits(py, bits);
                }
            });
        });
    }
}

extern "C" fn tuple_iteration_override(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let source =
            MoltObject::from_ptr(alloc_tuple(py, &[MoltObject::from_int(9).bits()])).bits();
        let iterator = crate::molt_iter(source);
        dec_ref_bits(py, source);
        iterator
    })
}

#[test]
fn tuple_subclass_conversion_observes_overrides_but_explicit_base_slot_does_not() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = unsafe { new_class_with_base(py, b"TupleOverride", builtin_classes(py).tuple) };
        let name = attr_name_bits_from_bytes(py, b"__iter__").unwrap();
        let callback = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "tuple_iteration_override",
                tuple_iteration_override as *const (),
            ),
            1,
        );
        assert!(!callback.is_null());
        let callback = MoltObject::from_ptr(callback).bits();
        molt_set_attr_name(class, name, callback);
        let source = unsafe {
            crate::object::builders::alloc_tuple_subclass(
                py,
                class,
                &[MoltObject::from_int(7).bits()],
            )
        };
        assert!(!exception_pending(py));
        for destination in [builtin_classes(py).tuple, class] {
            let copied = molt_tuple_new_bound(destination, source);
            assert!(!exception_pending(py));
            let ptr = obj_from_bits(copied)
                .as_ptr()
                .expect("overridden tuple copy");
            assert_eq!(
                unsafe { with_immutable_tuple_slice(ptr, |items| items.to_vec()) },
                Some(vec![MoltObject::from_int(9).bits()]),
            );
            dec_ref_bits(py, copied);
        }
        let base_method = crate::builtins::containers::tuple_method_bits(py, "__iter__").unwrap();
        let iterator = unsafe { crate::call_callable1(py, base_method, source) };
        assert!(!exception_pending(py));
        let item = crate::molt_next_builtin(iterator, missing_bits(py));
        assert!(!exception_pending(py));
        assert_eq!(item, MoltObject::from_int(7).bits());
        molt_set_attr_name(class, name, MoltObject::none().bits());
        let rejected = molt_tuple_new_bound(class, source);
        assert!(obj_from_bits(rejected).is_none());
        assert!(
            exception_pending(py),
            "None override must not expose builtin iteration"
        );
        crate::molt_exception_clear();
        for bits in [item, iterator, source, callback, name, class] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn tuple_subclass_admission_rejects_nominal_and_physical_layout_mismatches() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        let object_base = builtin_classes(_py).object;
        let tuple_base = builtin_classes(_py).tuple;

        // An ordinary class has the same minimum cached byte extent as an
        // empty tuple, but size equality cannot manufacture tuple ancestry.
        let unrelated = unsafe { new_class_with_base(_py, b"TupleSizedImpostor", object_base) };
        assert_eq!(
            molt_tuple_new_bound(unrelated, missing),
            MoltObject::none().bits()
        );
        assert!(exception_pending(_py));
        crate::molt_exception_clear();

        let expanded = unsafe {
            new_slotted_class_with_base(_py, b"ExpandedTupleChild", b"extra", tuple_base)
        };
        assert_eq!(
            molt_tuple_new_bound(expanded, missing),
            MoltObject::none().bits()
        );
        assert!(exception_pending(_py));
        crate::molt_exception_clear();

        let shaped = unsafe { new_class_with_base(_py, b"ShapedTupleChild", tuple_base) };
        let shaped_ptr = obj_from_bits(shaped).as_ptr().expect("shaped tuple class");
        assert!(unsafe {
            crate::object::class_set_instance_shape_id(
                shaped_ptr,
                crate::object::ObjectShapeId::DictSubclass,
            )
        });
        assert_eq!(
            molt_tuple_new_bound(shaped, missing),
            MoltObject::none().bits()
        );
        assert!(exception_pending(_py));
        crate::molt_exception_clear();

        dec_ref_bits(_py, shaped);
        dec_ref_bits(_py, expanded);
        dec_ref_bits(_py, unrelated);
        assert!(!exception_pending(_py));
    });
}
