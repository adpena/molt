//! Exact native alias identity survives C3 inheritance and repeated exposure.
use super::*;

#[test]
fn native_alias_diamond_preserves_physical_mro_and_canonical_projection() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = snapshot_class(py, b"PhysicalAliasDiamond");
            let key = crate::attr_name_bits_from_bytes(py, b"__len__").unwrap();
            let seventeen = function(py, length_seventeen as *const (), 1);
            let twenty_three = function(py, length_twenty_three as *const (), 1);
            crate::molt_set_attr_name(class, key, seventeen);
            let canonical =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            assert!(
                !canonical.as_ptr().is_null(),
                "{}",
                native_error_description()
            );
            let canonical_type = canonical.as_ptr().cast::<PyTypeObject>();
            let canonical_mro = OwnedPyObject::from_borrowed((*canonical_type).tp_mro);
            let canonical_instance =
                OwnedPyObject::from_owned(object::PyObject_CallNoArgs(canonical.as_ptr()));
            assert!(
                !canonical_instance.as_ptr().is_null(),
                "{}",
                native_error_description()
            );
            let object_type = &raw mut PyBaseObject_Type;

            let mut sequence: PySequenceMethods = std::mem::zeroed();
            sequence.sq_length = length_three as *const () as *mut std::ffi::c_void;
            let mut alias =
                NativeType::<PyTypeObject>::subtype(object_type, c"hierarchy.PhysicalAlias");
            alias.tp_as_sequence = (&raw mut sequence).cast();
            let alias_type = &raw mut *alias;
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(alias_type.cast(), class, false)
                .unwrap();
            assert_eq!(alias.ready(), 0, "{}", native_error_description());
            assert_physical_mro(alias_type, &[alias_type, object_type]);
            assert_eq!(alias.tp_base, object_type);
            assert_eq!(
                sequences::PyTuple_GetItem(alias.tp_bases, 0),
                object_type.cast()
            );

            let mut left_sequence: PySequenceMethods = std::mem::zeroed();
            let mut right_sequence: PySequenceMethods = std::mem::zeroed();
            let mut diamond_sequence: PySequenceMethods = std::mem::zeroed();
            let mut left = NativeType::<PyTypeObject>::subtype(alias_type, c"hierarchy.AliasLeft");
            left.tp_as_sequence = (&raw mut left_sequence).cast();
            assert_eq!(left.ready(), 0, "{}", native_error_description());
            let left_type = &raw mut *left;
            let mut right =
                NativeType::<PyTypeObject>::subtype(alias_type, c"hierarchy.AliasRight");
            right.tp_as_sequence = (&raw mut right_sequence).cast();
            assert_eq!(right.ready(), 0, "{}", native_error_description());
            let right_type = &raw mut *right;
            let direct = [left_type.cast::<PyObject>(), right_type.cast()];
            let bases = OwnedPyObject::from_owned(sequences::PyTuple_FromArray(direct.as_ptr(), 2));
            assert!(!bases.as_ptr().is_null(), "{}", native_error_description());
            let mut diamond =
                NativeType::<PyTypeObject>::subtype(left_type, c"hierarchy.AliasDiamond");
            diamond.tp_bases = bases.into_ptr();
            diamond.tp_as_sequence = (&raw mut diamond_sequence).cast();
            assert_eq!(diamond.ready(), 0, "{}", native_error_description());
            let diamond_type = &raw mut *diamond;
            assert_eq!((*left_type).tp_base, alias_type);
            assert_eq!((*right_type).tp_base, alias_type);
            assert_eq!((*diamond_type).tp_base, left_type);
            for (index, entry) in direct.into_iter().enumerate() {
                assert_eq!(
                    sequences::PyTuple_GetItem((*diamond_type).tp_bases, index as isize),
                    entry
                );
            }
            assert_physical_mro(left_type, &[left_type, alias_type, object_type]);
            assert_physical_mro(right_type, &[right_type, alias_type, object_type]);
            assert_physical_mro(
                diamond_type,
                &[diamond_type, left_type, right_type, alias_type, object_type],
            );
            for native in [alias_type, left_type, right_type, diamond_type] {
                assert_eq!(
                    native_sequence_size(native),
                    3,
                    "{}",
                    native_error_description()
                );
            }
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Size(
                    canonical_instance.as_ptr()
                ),
                17
            );

            // Re-exposure replaces owned roots without changing physical self,
            // the runtime's original tuple, or any descendant C3 order.
            assert_eq!(alias.ready(), 0, "{}", native_error_description());
            assert_physical_mro(alias_type, &[alias_type, object_type]);
            assert_physical_mro(
                diamond_type,
                &[diamond_type, left_type, right_type, alias_type, object_type],
            );
            assert_eq!((*canonical_type).tp_mro, canonical_mro.as_ptr());
            assert_physical_mro(canonical_type, &[canonical_type, object_type]);
            let projected =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            assert_eq!(projected.as_ptr(), canonical.as_ptr());

            crate::molt_set_attr_name(class, key, twenty_three);
            assert!(
                !crate::exception_pending(py),
                "{}",
                native_error_description()
            );
            for native in [alias_type, left_type, right_type, diamond_type] {
                assert_eq!(
                    native_sequence_size(native),
                    23,
                    "{}",
                    native_error_description()
                );
            }
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Size(
                    canonical_instance.as_ptr()
                ),
                23
            );
            drop(diamond);
            drop(right);
            drop(left);
            assert!(
                GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(alias_type.cast(), class)
            );
            drop(alias);
            // Exact unbinding retires only this alias; the canonical allocation
            // and semantic MRO still name the same live runtime class.
            let after_unbind =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            assert_eq!(after_unbind.as_ptr(), canonical.as_ptr());
            assert_eq!((*canonical_type).tp_mro, canonical_mro.as_ptr());
            for bits in [twenty_three, seventeen, key, class] {
                dec_ref_bits(py, bits);
            }
            assert!(
                errors::PyErr_Occurred().is_null(),
                "{}",
                native_error_description()
            );
        }
    });
}
