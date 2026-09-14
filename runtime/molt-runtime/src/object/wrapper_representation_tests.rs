use super::heap_kinds_generated::{
    HeapLayoutPolicy, HeapShapePolicy, heap_layout_policy, heap_shape_policy,
};
use super::layout::{
    WrapperKind, classmethod_func_bits, property_getter_doc, property_name_bits,
    property_replace_doc_bits, property_replace_name_bits, wrapper_reference_bits,
};
use super::*;
use crate::{MoltObject, alloc_dict_with_pairs, alloc_string, builtin_classes, dec_ref_bits};

#[test]
fn wrapper_heap_kinds_and_builtin_classes_share_one_native_prefix_authority() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let classes = builtin_classes(py);
        for (kind, class_bits) in [
            (WrapperKind::Classmethod, classes.classmethod),
            (WrapperKind::Staticmethod, classes.staticmethod),
            (WrapperKind::Property, classes.property),
        ] {
            assert_eq!(
                heap_layout_policy(kind.type_id()),
                Some(HeapLayoutPolicy::Object)
            );
            assert_eq!(
                heap_shape_policy(kind.type_id()),
                Some(HeapShapePolicy::Class)
            );
            let class_ptr = obj_from_bits(class_bits).as_ptr().unwrap();
            assert_eq!(unsafe { class_instance_type_id(class_ptr) }, kind.type_id());
            assert_eq!(
                unsafe { layout::class_cached_layout_size(class_ptr) },
                Some(kind.prefix_size() + std::mem::size_of::<u64>())
            );
        }
    });
}

#[test]
fn exact_wrapper_lifecycle_composes_prefix_dictionary_and_class_edges() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let target = alloc_string(py, b"wrapper target");
        assert!(!target.is_null());
        let target_bits = MoltObject::from_ptr(target).bits();
        let property = builders::alloc_property_obj(py, target_bits, target_bits, target_bits);
        assert!(!property.is_null());
        assert!(crate::is_missing_bits(py, unsafe {
            property_name_bits(property)
        }));
        unsafe {
            assert!(property_replace_doc_bits(py, property, target_bits));
            assert!(property_replace_name_bits(py, property, target_bits));
        }
        dec_ref_bits(py, target_bits);

        let dictionary = alloc_dict_with_pairs(py, &[]);
        assert!(!dictionary.is_null());
        let dictionary_bits = MoltObject::from_ptr(dictionary).bits();
        unsafe { instance_set_dict_bits(py, property, dictionary_bits) };

        let class_ptr = obj_from_bits(builtin_classes(py).property)
            .as_ptr()
            .unwrap();
        let mut edges = Vec::new();
        unsafe { heap_lifecycle::visit_owned_edges(py, property, &mut |ptr| edges.push(ptr)) };
        assert_eq!(edges.iter().filter(|&&ptr| ptr == target).count(), 5);
        assert!(edges.contains(&dictionary));
        assert!(edges.contains(&class_ptr));
        assert!(!unsafe { property_getter_doc(property) });

        unsafe { heap_lifecycle::clear_cycle_edges(py, property) };
        for index in 0..4 {
            assert_eq!(
                unsafe { wrapper_reference_bits(property, index) },
                MoltObject::none().bits()
            );
        }
        assert!(crate::is_missing_bits(py, unsafe {
            property_name_bits(property)
        }));
        assert_eq!(unsafe { instance_dict_bits(property) }, 0);
        let mut remaining = Vec::new();
        unsafe {
            heap_lifecycle::visit_owned_edges(py, property, &mut |ptr| remaining.push(ptr));
        }
        assert_eq!(remaining, vec![class_ptr]);
        dec_ref_bits(py, MoltObject::from_ptr(property).bits());

        let raw_target = alloc_string(py, b"classmethod target");
        assert!(!raw_target.is_null());
        let raw_target_bits = MoltObject::from_ptr(raw_target).bits();
        let classmethod = builders::alloc_classmethod_obj(py, raw_target_bits);
        assert!(!classmethod.is_null());
        dec_ref_bits(py, raw_target_bits);
        unsafe { heap_lifecycle::clear_cycle_edges(py, classmethod) };
        assert!(crate::is_missing_bits(py, unsafe {
            classmethod_func_bits(classmethod)
        }));
        dec_ref_bits(py, MoltObject::from_ptr(classmethod).bits());
    });
}

#[test]
fn property_missing_name_is_distinct_from_explicit_none() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let property = builders::alloc_property_obj(
            py,
            MoltObject::none().bits(),
            MoltObject::none().bits(),
            MoltObject::none().bits(),
        );
        assert!(!property.is_null());
        assert!(crate::is_missing_bits(py, unsafe {
            property_name_bits(property)
        }));

        unsafe {
            assert!(property_replace_name_bits(
                py,
                property,
                MoltObject::none().bits()
            ));
        }
        assert_eq!(
            unsafe { property_name_bits(property) },
            MoltObject::none().bits()
        );

        dec_ref_bits(py, MoltObject::from_ptr(property).bits());
    });
}

#[test]
fn wrapper_missing_targets_are_distinct_from_explicit_none() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for (kind, object) in [
            (
                WrapperKind::Classmethod,
                builders::alloc_classmethod_obj(py, MoltObject::none().bits()),
            ),
            (
                WrapperKind::Staticmethod,
                builders::alloc_staticmethod_obj(py, MoltObject::none().bits()),
            ),
        ] {
            assert!(!object.is_null());
            assert_eq!(
                unsafe { wrapper_reference_bits(object, 0) },
                MoltObject::none().bits()
            );
            unsafe { heap_lifecycle::clear_cycle_edges(py, object) };
            assert!(crate::is_missing_bits(py, unsafe {
                wrapper_reference_bits(object, 0)
            }));
            assert_eq!(unsafe { object_type_id(object) }, kind.type_id());
            dec_ref_bits(py, MoltObject::from_ptr(object).bits());
        }
    });
}

#[test]
fn raw_zero_cannot_cross_a_wrapper_reference_boundary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        assert!(builders::alloc_classmethod_obj(py, 0).is_null());
        assert!(builders::alloc_staticmethod_obj(py, 0).is_null());
        assert!(
            builders::alloc_property_obj(
                py,
                0,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            )
            .is_null()
        );

        let target = alloc_string(py, b"typed replacement target");
        assert!(!target.is_null());
        let target_bits = MoltObject::from_ptr(target).bits();
        let staticmethod = builders::alloc_staticmethod_obj(py, target_bits);
        assert!(!staticmethod.is_null());
        assert!(!unsafe {
            property_replace_name_bits(py, staticmethod, MoltObject::none().bits())
        });
        assert_eq!(
            unsafe { wrapper_reference_bits(staticmethod, 0) },
            target_bits
        );
        dec_ref_bits(py, target_bits);
        dec_ref_bits(py, MoltObject::from_ptr(staticmethod).bits());
    });
}

#[test]
fn declared_fields_cannot_alias_a_native_wrapper_prefix() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let name = alloc_string(py, b"aliased");
        assert!(!name.is_null());
        let name_bits = MoltObject::from_ptr(name).bits();
        let offsets = alloc_dict_with_pairs(py, &[name_bits, MoltObject::from_int(0).bits()]);
        assert!(!offsets.is_null());
        dec_ref_bits(py, name_bits);

        assert!(
            unsafe {
                validate_class_field_offsets(
                    py,
                    offsets,
                    WrapperKind::Classmethod.prefix_size(),
                    2 * std::mem::size_of::<u64>(),
                )
            }
            .is_err()
        );
        assert!(crate::exception_pending(py));
        crate::clear_exception(py);
        dec_ref_bits(py, MoltObject::from_ptr(offsets).bits());
    });
}
