use super::{
    ATTR_NAME_INLINE_CAP, AttrNameCacheKey, set_attribute_error_members,
    with_class_descriptor_snapshot,
};
use crate::{MoltObject, alloc_string, dec_ref_bits, obj_from_bits};

fn heap_refcount(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().expect("expected heap bits");
    unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

#[test]
fn ordinary_missing_observation_needs_no_allocation() {
    use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let name = crate::NameId::DunderDeepcopy.bits(py);
        let class_name = crate::dynamic_attr_name_bits(py, b"AllocationFreeLookup").unwrap();
        let class = crate::molt_class_new_attached_body(py, class_name);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        unsafe { crate::object::class_finish_definition(py, class_ptr).unwrap() };
        let instance = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
        let module = MoltObject::from_ptr(crate::alloc_module_obj(py, class_name)).bits();
        for receiver in [instance, class, module] {
            // Warm lazy namespace publication, independently of the allocation
            // obligation. An exception on *each* miss still cannot pass below.
            assert_eq!(
                super::lookup_attribute(py, receiver, name),
                super::AttributeLookup::Missing
            );
            assert!(!crate::exception_pending(py));
        }
        #[cfg(feature = "stdlib_http")]
        assert_eq!(
            molt_runtime_http::bridge::attr_optional(
                py.core_token(),
                instance,
                b"absent_http_lookup"
            ),
            Ok(None)
        );
        let reset = crate::test_support::restore_unlimited_tracker_on_drop();
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_allocations: Some(0),
            ..ResourceLimits::default()
        })));
        for receiver in [instance, class, module] {
            assert_eq!(
                super::lookup_attribute(py, receiver, name),
                super::AttributeLookup::Missing
            );
            assert!(!crate::exception_pending(py));
            #[cfg(feature = "stdlib_http")]
            {
                assert_eq!(
                    crate::http_bridge::__molt_http_lookup_attribute(
                        py.attached_context(),
                        receiver,
                        name
                    ),
                    super::AttributeLookup::Missing
                );
                assert_eq!(
                    molt_runtime_http::bridge::attr_optional(
                        py.core_token(),
                        instance,
                        b"absent_http_lookup"
                    ),
                    Ok(None)
                );
                assert!(!crate::exception_pending(py));
            }
            #[cfg(feature = "molt_gpu_primitives")]
            {
                assert_eq!(
                    crate::gpu_bridge::__molt_gpu_lookup_attribute(
                        py.attached_context(),
                        receiver,
                        name
                    ),
                    super::AttributeLookup::Missing
                );
                assert!(!crate::exception_pending(py));
            }
        }
        drop(reset);
        for bits in [instance, class, module, class_name] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn physical_slot_declaration_survives_public_rebind_mutation_and_iterator_exhaustion() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for kind in 0..4 {
            let name = crate::dynamic_attr_name_bits(_py, b"SealedSlots").unwrap();
            let field = crate::dynamic_attr_name_bits(_py, b"field").unwrap();
            let dict = crate::dynamic_attr_name_bits(_py, b"__dict__").unwrap();
            let weakref = crate::dynamic_attr_name_bits(_py, b"__weakref__").unwrap();
            let slots = crate::dynamic_attr_name_bits(_py, b"__slots__").unwrap();
            let class = crate::molt_class_new_attached_body(_py, name);
            let ptr = obj_from_bits(class).as_ptr().unwrap();
            let declaration = match kind {
                0 => {
                    crate::inc_ref_bits(_py, field);
                    field
                }
                1 => MoltObject::from_ptr(crate::alloc_tuple(_py, &[field, dict, weakref])).bits(),
                2 => MoltObject::from_ptr(crate::alloc_list(_py, &[field, dict, weakref])).bits(),
                _ => {
                    let tuple =
                        MoltObject::from_ptr(crate::alloc_tuple(_py, &[field, dict, weakref]))
                            .bits();
                    let iterator = crate::molt_iter_attached_body(_py, tuple);
                    dec_ref_bits(_py, tuple);
                    iterator
                }
            };
            crate::molt_set_attr_name_attached_body(_py, class, slots, declaration);
            unsafe {
                crate::object::class_finish_definition(_py, ptr).expect("seal valid slots");
                let offset = super::class_own_slot_field_offset(_py, ptr, field).unwrap();
                let captured = crate::object::layout::class_slot_declaration_bits(ptr);
                assert_ne!(captured, declaration, "the layout tuple must be private");
                assert!(
                    crate::object::object_payload_size(ptr)
                        >= crate::object::layout::CLASS_PAYLOAD_WORDS * std::mem::size_of::<u64>()
                );
                let mut edges = Vec::new();
                crate::object::heap_lifecycle::visit_owned_values(_py, ptr, &mut |edge| {
                    edges.push(edge)
                });
                assert!(
                    edges.contains(&captured),
                    "declaration is a traced class owner"
                );
                // A second layout pass must not consume an iterator again.
                crate::object::class_finish_definition(_py, ptr).expect("seal valid slots");
                crate::object::class_finish_definition(_py, ptr).expect("seal valid slots");
                if kind == 2 {
                    crate::molt_list_clear_attached_body(_py, declaration);
                }
                let empty = MoltObject::from_ptr(crate::alloc_tuple(_py, &[])).bits();
                crate::molt_set_attr_name_attached_body(_py, class, slots, empty);
                dec_ref_bits(_py, empty);
                assert_eq!(
                    crate::object::layout::class_slot_declaration_bits(ptr),
                    captured
                );
                assert_eq!(
                    super::class_own_slot_field_offset(_py, ptr, field),
                    Some(offset)
                );
                let info = super::class_slots_info(_py, ptr).unwrap();
                assert_eq!(info.allows_dict, kind != 0);
                assert_eq!(info.allows_weakref, kind != 0);
            }
            assert!(!crate::exception_pending(_py));
            for bits in [name, field, dict, weakref, slots, class, declaration] {
                dec_ref_bits(_py, bits);
            }
        }
    });
}

#[test]
fn absence_of_slots_is_sealed_and_invalid_declarations_do_not_publish_provenance() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let name = crate::dynamic_attr_name_bits(_py, b"Unslotted").unwrap();
        let slots = crate::dynamic_attr_name_bits(_py, b"__slots__").unwrap();
        let empty = MoltObject::from_ptr(crate::alloc_tuple(_py, &[])).bits();
        let class = crate::molt_class_new_attached_body(_py, name);
        let ptr = obj_from_bits(class).as_ptr().unwrap();
        unsafe {
            crate::object::class_finish_definition(_py, ptr).expect("seal unslotted class");
        }
        crate::molt_set_attr_name_attached_body(_py, class, slots, empty);
        let policy = unsafe { super::class_slots_info(_py, ptr) }.unwrap();
        assert!(policy.allows_dict && policy.allows_weakref);
        assert!(matches!(
            unsafe { crate::object::layout::class_slot_declaration(ptr) },
            crate::object::layout::ClassSlotDeclaration::Absent
        ));

        let invalid_class = crate::molt_class_new_attached_body(_py, name);
        let invalid_ptr = obj_from_bits(invalid_class).as_ptr().unwrap();
        let invalid =
            MoltObject::from_ptr(crate::alloc_tuple(_py, &[MoltObject::from_int(1).bits()])).bits();
        crate::molt_set_attr_name_attached_body(_py, invalid_class, slots, invalid);
        assert!(unsafe { crate::object::class_finish_definition(_py, invalid_ptr) }.is_err());
        assert!(crate::exception_pending(_py));
        assert!(!unsafe { crate::object::class_definition_is_finished(invalid_ptr) });
        assert!(unsafe { crate::object::layout::class_cached_layout_size(invalid_ptr) }.is_none());
        crate::molt_exception_clear_attached_body(_py);
        assert!(matches!(
            unsafe { crate::object::layout::class_slot_declaration(invalid_ptr) },
            crate::object::layout::ClassSlotDeclaration::Uninitialized
        ));
        crate::molt_set_attr_name_attached_body(_py, invalid_class, slots, empty);
        unsafe { crate::object::class_finish_definition(_py, invalid_ptr) }
            .expect("retry valid declaration");
        assert!(
            !unsafe { super::class_slots_info(_py, invalid_ptr) }
                .unwrap()
                .allows_dict
        );
        assert!(!crate::exception_pending(_py));
        for bits in [name, slots, empty, class, invalid_class, invalid] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn sealed_field_offsets_retain_exact_map_and_ignore_namespace_rebinding() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let name = crate::dynamic_attr_name_bits(_py, b"SealedFieldOffsets").unwrap();
        let field = crate::dynamic_attr_name_bits(_py, b"field").unwrap();
        let offsets_name = crate::dynamic_attr_name_bits(_py, b"__molt_field_offsets__").unwrap();
        let size_name = crate::dynamic_attr_name_bits(_py, b"__molt_layout_size__").unwrap();
        let class = crate::molt_class_new_attached_body(_py, name);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        let offsets_ptr =
            crate::alloc_dict_with_pairs(_py, &[field, MoltObject::from_int(0).bits()]);
        let offsets = MoltObject::from_ptr(offsets_ptr).bits();
        crate::molt_set_attr_name_attached_body(_py, class, offsets_name, offsets);
        crate::molt_set_attr_name_attached_body(
            _py,
            class,
            size_name,
            MoltObject::from_int(16).bits(),
        );
        unsafe { crate::object::class_finish_definition(_py, class_ptr) }
            .expect("seal exact field map");
        assert_eq!(
            unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
            offsets,
        );
        let namespace = obj_from_bits(unsafe { crate::class_dict_bits(class_ptr) })
            .as_ptr()
            .unwrap();
        assert!(
            unsafe {
                crate::dict_get_in_place(_py, namespace, offsets_name)
                    .expect("dictionary lookup succeeds")
            }
            .is_none()
        );
        assert!(
            unsafe {
                crate::dict_get_in_place(_py, namespace, size_name)
                    .expect("dictionary lookup succeeds")
            }
            .is_none()
        );

        let rebound_ptr =
            crate::alloc_dict_with_pairs(_py, &[field, MoltObject::from_int(8).bits()]);
        let rebound = MoltObject::from_ptr(rebound_ptr).bits();
        crate::molt_set_attr_name_attached_body(_py, class, offsets_name, rebound);
        assert!(crate::exception_pending(_py));
        let exception = crate::molt_exception_last_attached_body(_py);
        assert!(crate::builtins::exceptions::exception_matches_kind(
            _py,
            exception,
            crate::BuiltinExc::TypeError,
        ));
        crate::molt_exception_clear_attached_body(_py);
        dec_ref_bits(_py, exception);
        assert_eq!(
            unsafe { super::class_field_offset(_py, class_ptr, field) },
            Some(0)
        );
        crate::molt_del_attr_name_attached_body(_py, class, offsets_name);
        assert!(crate::exception_pending(_py));
        let exception = crate::molt_exception_last_attached_body(_py);
        assert!(crate::builtins::exceptions::exception_matches_kind(
            _py,
            exception,
            crate::BuiltinExc::TypeError,
        ));
        crate::molt_exception_clear_attached_body(_py);
        dec_ref_bits(_py, exception);
        assert_eq!(
            unsafe { super::class_field_offset(_py, class_ptr, field) },
            Some(0)
        );

        let namespace = obj_from_bits(unsafe { crate::class_dict_bits(class_ptr) })
            .as_ptr()
            .unwrap();
        unsafe {
            assert!(crate::dict_set_in_place(
                _py,
                namespace,
                offsets_name,
                rebound
            ))
        };
        assert_eq!(
            unsafe { super::class_field_offset(_py, class_ptr, field) },
            Some(0)
        );
        assert!(
            unsafe { crate::dict_del_in_place(_py, namespace, offsets_name) }
                .expect("namespace deletion succeeds")
        );
        assert_eq!(
            unsafe { super::class_field_offset(_py, class_ptr, field) },
            Some(0)
        );
        assert_eq!(
            unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
            offsets,
        );

        let mut edges = Vec::new();
        unsafe {
            crate::object::heap_lifecycle::visit_owned_values(_py, class_ptr, &mut |bits| {
                edges.push(bits)
            });
        }
        let record = unsafe { crate::object::layout::class_field_layout_bits(class_ptr) };
        assert_eq!(edges.iter().filter(|&&bits| bits == record).count(), 1);
        assert!(
            !edges.contains(&offsets),
            "the record owns the map projection"
        );
        let mut record_edges = Vec::new();
        unsafe {
            crate::object::heap_lifecycle::visit_owned_values(
                _py,
                obj_from_bits(record).as_ptr().unwrap(),
                &mut |bits| record_edges.push(bits),
            );
        }
        assert_eq!(
            record_edges.iter().filter(|&&bits| bits == offsets).count(),
            1
        );
        assert_eq!(heap_refcount(offsets), 2, "local plus private record owner");
        for _ in 0..2 {
            unsafe { crate::object::heap_lifecycle::clear_cycle_edges(_py, class_ptr) };
            assert_eq!(
                unsafe { crate::object::layout::class_field_layout_bits(class_ptr) },
                record,
                "cycle clear must retain the physical layout identity"
            );
            assert_eq!(
                unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
                offsets,
                "live instances still require the exact physical field map"
            );
            assert_eq!(
                heap_refcount(offsets),
                2,
                "cycle clear must preserve the local and private-record owners"
            );
        }
        dec_ref_bits(_py, class);
        assert_eq!(
            heap_refcount(offsets),
            1,
            "terminal class release must discharge the private map edge exactly once"
        );

        for bits in [name, field, offsets_name, size_name, offsets, rebound] {
            dec_ref_bits(_py, bits);
        }
        assert!(!crate::exception_pending(_py));
    });
}

#[test]
fn malformed_field_offsets_fail_before_private_publication() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let name = crate::dynamic_attr_name_bits(_py, b"InvalidFieldOffsets").unwrap();
        let field = crate::dynamic_attr_name_bits(_py, b"field").unwrap();
        let other = crate::dynamic_attr_name_bits(_py, b"other").unwrap();
        let offsets_name = crate::dynamic_attr_name_bits(_py, b"__molt_field_offsets__").unwrap();
        let size_name = crate::dynamic_attr_name_bits(_py, b"__molt_layout_size__").unwrap();
        let cases = [
            (
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(0).bits(),
                16,
            ),
            (field, MoltObject::from_bool(false).bits(), 16),
            (field, MoltObject::from_int(-8).bits(), 16),
            (field, MoltObject::from_int(4).bits(), 24),
            (field, MoltObject::from_int(8).bits(), 16),
        ];
        for (key, offset, size) in cases {
            let class = crate::molt_class_new_attached_body(_py, name);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let offsets_ptr = crate::alloc_dict_with_pairs(_py, &[key, offset]);
            let offsets = MoltObject::from_ptr(offsets_ptr).bits();
            crate::molt_set_attr_name_attached_body(_py, class, offsets_name, offsets);
            crate::molt_set_attr_name_attached_body(
                _py,
                class,
                size_name,
                MoltObject::from_int(size).bits(),
            );
            assert!(unsafe { crate::object::class_finish_definition(_py, class_ptr) }.is_err());
            assert!(crate::exception_pending(_py));
            assert_eq!(
                unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
                0,
            );
            assert!(
                unsafe { crate::object::layout::class_cached_layout_size(class_ptr) }.is_none()
            );
            assert!(!unsafe { crate::object::class_definition_is_finished(class_ptr) });
            crate::molt_exception_clear_attached_body(_py);
            dec_ref_bits(_py, offsets);
            dec_ref_bits(_py, class);
        }

        let class = crate::molt_class_new_attached_body(_py, name);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        let offsets_ptr = crate::alloc_dict_with_pairs(
            _py,
            &[
                field,
                MoltObject::from_int(0).bits(),
                other,
                MoltObject::from_int(0).bits(),
            ],
        );
        let offsets = MoltObject::from_ptr(offsets_ptr).bits();
        crate::molt_set_attr_name_attached_body(_py, class, offsets_name, offsets);
        crate::molt_set_attr_name_attached_body(
            _py,
            class,
            size_name,
            MoltObject::from_int(16).bits(),
        );
        assert!(unsafe { crate::object::class_finish_definition(_py, class_ptr) }.is_err());
        assert_eq!(
            unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
            0,
        );
        crate::molt_exception_clear_attached_body(_py);
        dec_ref_bits(_py, offsets);
        dec_ref_bits(_py, class);

        for bits in [name, field, other, offsets_name, size_name] {
            dec_ref_bits(_py, bits);
        }
        assert!(!crate::exception_pending(_py));
    });
}

#[test]
fn attr_name_cache_key_inlines_common_attr_names() {
    let key = AttrNameCacheKey::new(b"__molt_arg_names__");

    assert!(matches!(key, AttrNameCacheKey::Inline { .. }));
    assert_eq!(key.as_slice(), b"__molt_arg_names__");
}

#[test]
fn attr_name_cache_key_preserves_long_names() {
    let bytes = vec![b'x'; ATTR_NAME_INLINE_CAP + 1];
    let key = AttrNameCacheKey::new(&bytes);

    assert!(matches!(key, AttrNameCacheKey::Heap(_)));
    assert_eq!(key.as_slice(), bytes.as_slice());
}

#[test]
fn attribute_error_members_use_canonical_typed_fields_without_overwriting_args() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let obj_ptr = alloc_string(_py, b"attribute-owner");
        assert!(!obj_ptr.is_null());
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let obj_refcount = heap_refcount(obj_bits);
        let exc_ptr = crate::builtins::exceptions::alloc_exception(
            _py,
            crate::BuiltinExc::AttributeError,
            "missing attribute",
        );
        assert!(!exc_ptr.is_null());
        let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
        let args_bits = unsafe { crate::builtins::exceptions::exception_args_bits(exc_ptr) };
        let args_refcount = heap_refcount(args_bits);

        set_attribute_error_members(_py, exc_bits, "missing", obj_bits)
            .expect("publish AttributeError typed members");

        assert_eq!(
            unsafe { crate::builtins::exceptions::exception_args_bits(exc_ptr) },
            args_bits,
            "AttributeError member publication must not overwrite args"
        );
        assert_eq!(heap_refcount(args_bits), args_refcount);
        let name_bits = crate::builtins::exceptions::exception_typed_field_get(
            _py,
            exc_ptr,
            molt_obj_model::ExceptionTypedField::AttributeErrorName,
        )
        .expect("AttributeError.name descriptor")
        .expect("AttributeError.name value");
        let member_obj_bits = crate::builtins::exceptions::exception_typed_field_get(
            _py,
            exc_ptr,
            molt_obj_model::ExceptionTypedField::AttributeErrorObject,
        )
        .expect("AttributeError.obj descriptor")
        .expect("AttributeError.obj value");
        assert_eq!(
            crate::string_obj_to_owned(obj_from_bits(name_bits)).as_deref(),
            Some("missing")
        );
        assert_eq!(member_obj_bits, obj_bits);
        assert_eq!(heap_refcount(obj_bits), obj_refcount + 2);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, member_obj_bits);
        assert_eq!(heap_refcount(obj_bits), obj_refcount + 1);

        dec_ref_bits(_py, exc_bits);
        assert_eq!(heap_refcount(obj_bits), obj_refcount);
        dec_ref_bits(_py, obj_bits);
    });
}

#[test]
fn cached_namespace_borrows_values_but_active_snapshot_owns_selection() {
    assert_cached_namespace_snapshot_ownership(
        molt_cpython_abi::hooks::AttributeMutation::TypeDefault,
    );
}

#[test]
fn raw_generic_namespace_mutation_invalidates_borrowed_runtime_observations() {
    assert_cached_namespace_snapshot_ownership(molt_cpython_abi::hooks::AttributeMutation::Generic);
}

fn assert_cached_namespace_snapshot_ownership(access: molt_cpython_abi::hooks::AttributeMutation) {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let class = super::descriptor_tests::test_class_bits(_py, b"SnapshotOwner", &[], &[]);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        assert!(unsafe { crate::class_mro_pinned(_py, class_ptr) }.is_some());
        let attr = MoltObject::from_ptr(alloc_string(_py, b"cached attr")).bits();
        let first = MoltObject::from_ptr(crate::alloc_dict_with_pairs(_py, &[])).bits();
        let second = MoltObject::from_ptr(crate::alloc_dict_with_pairs(_py, &[])).bits();
        crate::builtins::attributes::mutate_attr_name(
            _py,
            class,
            attr,
            super::DescriptorMutation::Set(first),
            access,
        )
        .unwrap();
        let class_before = heap_refcount(class);
        let first_before = heap_refcount(first);
        for _ in 0..2 {
            unsafe {
                with_class_descriptor_snapshot(_py, class, attr, |snapshot| {
                    assert_eq!(snapshot.unwrap().class_attr_bits, Some(first));
                    Ok(())
                })
            }
            .unwrap();
        }
        assert_eq!(
            unsafe { super::class_namespace_lookup_mro(_py, class_ptr, attr) },
            Ok(Some(first))
        );
        let key = unsafe { super::class_cache::LookupKey::new(_py, class_ptr, attr) }.unwrap();
        assert_eq!(
            crate::runtime_state(_py)
                .attributes
                .class_lookup
                .lookup(key)
                .unwrap()
                .value,
            Some(first)
        );
        assert_eq!(heap_refcount(class), class_before);
        assert_eq!(heap_refcount(first), first_before);
        unsafe {
            with_class_descriptor_snapshot(_py, class, attr, |snapshot| {
                let snapshot = snapshot.unwrap();
                assert_eq!(heap_refcount(class), class_before + 1);
                assert_eq!(heap_refcount(first), first_before + 1);
                crate::builtins::attributes::mutate_attr_name(
                    _py,
                    class,
                    attr,
                    super::DescriptorMutation::Set(second),
                    access,
                )
                .unwrap();
                with_class_descriptor_snapshot(_py, class, attr, |inner| {
                    assert_eq!(inner.unwrap().class_attr_bits, Some(second));
                    Ok(())
                })
                .unwrap();
                assert_eq!(snapshot.class_attr_bits, Some(first));
                assert_eq!(heap_refcount(first), first_before);
                Ok(())
            })
        }
        .unwrap();
        assert_eq!(heap_refcount(first), first_before - 1);
        crate::builtins::attributes::mutate_attr_name(
            _py,
            class,
            attr,
            super::DescriptorMutation::Delete,
            access,
        )
        .unwrap();
        // Deletion releases the namespace's value before another lookup.
        assert_eq!(heap_refcount(second), 1);
        assert_eq!(heap_refcount(class), class_before);
        crate::runtime_state(_py).attributes.class_lookup.clear(_py);
        for bits in [second, first, attr, class] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn synthetic_function_descriptor_root_retirement_cannot_leave_a_cached_value() {
    use std::sync::atomic::Ordering;
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let state = crate::runtime_state(py);
        let class = obj_from_bits(crate::builtin_classes(py).function)
            .as_ptr()
            .unwrap();
        for (name, slot) in [
            (
                crate::NameId::DunderCode,
                &state.special_cache.function_code_descriptor,
            ),
            (
                crate::NameId::DunderGlobals,
                &state.special_cache.function_globals_descriptor,
            ),
        ] {
            let key = name.bits(py);
            let first = unsafe { super::class_namespace_lookup_mro(py, class, key) }
                .unwrap()
                .unwrap();
            assert_eq!(first, slot.load(Ordering::Acquire));
            crate::inc_ref_bits(py, first);
            crate::state::cache::clear_atomic_bits(py, slot);
            let second = unsafe { super::class_namespace_lookup_mro(py, class, key) }
                .unwrap()
                .unwrap();
            assert_eq!(second, slot.load(Ordering::Acquire));
            assert_ne!(
                first, second,
                "the retired root must be recreated rather than read from the namespace cache"
            );
            dec_ref_bits(py, first);
        }
    });
}
#[test]
fn c_attribute_mutation_invalidates_warm_runtime_type_lookup_with_or_without_a_native_tag() {
    use molt_cpython_abi::abi_types::PyTypeObject;
    use molt_cpython_abi::api::{numbers, object, refcount::OwnedPyObject, strings, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        unsafe {
            let class =
                super::descriptor_tests::test_class_bits(py, b"CNamespaceCacheOwner", &[], &[]);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let view = GLOBAL_BRIDGE
                .handle_to_borrowed_pyobj(class)
                .cast::<PyTypeObject>();
            assert!(!view.is_null());
            assert_eq!(typeobj::PyType_Ready(view), 0);
            let key =
                OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"c namespace".as_ptr()));
            let query = MoltObject::from_ptr(alloc_string(py, b"c namespace")).bits();
            for (native_lookup, first, second) in [(false, 101, 102), (true, 201, 202)] {
                let first_c = OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(first));
                let second_c = OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(second));
                assert_eq!(
                    object::PyObject_SetAttr(view.cast(), key.as_ptr(), first_c.as_ptr()),
                    0
                );
                typeobj::PyType_Modified(view);
                assert_eq!(
                    super::class_namespace_lookup_mro(py, class_ptr, query),
                    Ok(Some(MoltObject::from_int(first).bits()))
                );
                if native_lookup {
                    assert!(!typeobj::_PyType_Lookup(view, key.as_ptr()).is_null());
                }
                assert_eq!(
                    object::PyObject_SetAttr(view.cast(), key.as_ptr(), second_c.as_ptr()),
                    0
                );
                typeobj::PyType_Modified(view);
                assert_eq!(
                    super::class_namespace_lookup_mro(py, class_ptr, query),
                    Ok(Some(MoltObject::from_int(second).bits()))
                );
                assert!(!crate::exception_pending(py));
            }
            dec_ref_bits(py, query);
            drop(key);
            dec_ref_bits(py, class);
        }
    });
}

#[test]
fn c_type_modified_invalidates_runtime_namespace_hits_and_misses_without_native_tags() {
    use molt_cpython_abi::abi_types::{Py_TPFLAGS_VALID_VERSION_TAG, PyTypeObject};
    use molt_cpython_abi::api::{mapping, numbers, refcount::OwnedPyObject, strings, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        unsafe {
            for query in [
                crate::NameId::Append.owned_bits(py).unwrap(),
                MoltObject::from_ptr(alloc_string(py, b"manual C namespace")).bits(),
            ] {
                let owner =
                    super::descriptor_tests::test_class_bits(py, b"ManualNamespaceOwner", &[], &[]);
                let child = super::descriptor_tests::test_class_bits(
                    py,
                    b"ManualNamespaceChild",
                    &[owner],
                    &[],
                );
                let owner_view = GLOBAL_BRIDGE
                    .handle_to_borrowed_pyobj(owner)
                    .cast::<PyTypeObject>();
                assert_eq!(typeobj::PyType_Ready(owner_view), 0);
                let key = OwnedPyObject::from_owned(strings::PyUnicode_FromStringAndSize(
                    crate::string_bytes(obj_from_bits(query).as_ptr().unwrap()).cast(),
                    crate::string_len(obj_from_bits(query).as_ptr().unwrap()) as isize,
                ));
                for target in [owner, child] {
                    let class = obj_from_bits(target).as_ptr().unwrap();
                    let view = GLOBAL_BRIDGE
                        .handle_to_borrowed_pyobj(target)
                        .cast::<PyTypeObject>();
                    assert_eq!(typeobj::PyType_Ready(view), 0);
                    for native_lookup in [false, true] {
                        let mut expected = None;
                        // The C small-integer cache shares these immediate
                        // payloads; larger C integers have distinct carriers.
                        for next in [Some(101), Some(202), None, Some(203), None] {
                            // Force the same query to warm a hit or a miss after
                            // readiness, including through an inheriting class.
                            typeobj::PyType_Modified(owner_view);
                            assert_eq!(
                                super::class_namespace_lookup_mro(py, class, query),
                                Ok(expected.map(|value| MoltObject::from_int(value).bits()))
                            );
                            if native_lookup {
                                let found = typeobj::_PyType_Lookup(view, key.as_ptr());
                                assert_eq!(found.is_null(), expected.is_none());
                            } else {
                                assert_eq!(
                                    (*owner_view).tp_flags & Py_TPFLAGS_VALID_VERSION_TAG,
                                    0
                                );
                                assert_eq!((*view).tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
                            }
                            if let Some(value) = next {
                                let value =
                                    OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(value));
                                assert_eq!(
                                    mapping::PyDict_SetItem(
                                        (*owner_view).tp_dict,
                                        key.as_ptr(),
                                        value.as_ptr()
                                    ),
                                    0
                                );
                            } else {
                                assert_eq!(
                                    mapping::PyDict_DelItem((*owner_view).tp_dict, key.as_ptr()),
                                    0
                                );
                            }
                            typeobj::PyType_Modified(owner_view);
                            assert_eq!(
                                super::class_namespace_lookup_mro(py, class, query),
                                Ok(next.map(|value| MoltObject::from_int(value).bits())),
                                "manual dictionary edits must invalidate the owner and inherited lookups: target={target}, native_lookup={native_lookup}"
                            );
                            let found = typeobj::_PyType_Lookup(view, key.as_ptr());
                            if let Some(value) = next {
                                assert_eq!(
                                    GLOBAL_BRIDGE
                                        .observed_handle_for_pyobj(found)
                                        .unwrap()
                                        .bits(),
                                    MoltObject::from_int(value).bits()
                                );
                            } else {
                                assert!(found.is_null());
                            }
                            expected = next;
                        }
                    }
                }
                for bits in [child, owner, query] {
                    dec_ref_bits(py, bits);
                }
                assert!(!crate::exception_pending(py));
            }
        }
    });
}

#[test]
fn completed_native_absence_is_reusable_without_preparing_other_declarations() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = obj_from_bits(crate::builtin_classes(py).function)
            .as_ptr()
            .unwrap();
        for identity in [crate::NameId::DunderSet, crate::NameId::DunderDelete] {
            let name = identity.bits(py);
            assert_eq!(
                unsafe { super::class_namespace_lookup_mro(py, class, name) },
                Ok(None)
            );
            assert_eq!(
                unsafe {
                    super::class_namespace_lookup_mro_pure(
                        py,
                        class,
                        name,
                        None,
                        super::NamespacePreparation::Unresolved,
                    )
                },
                Some(None),
                "a pure client can reuse completed native absence without declaration preparation"
            );
            let key = unsafe { super::class_cache::LookupKey::new(py, class, name) }.unwrap();
            assert_eq!(
                crate::runtime_state(py)
                    .attributes
                    .class_lookup
                    .lookup(key)
                    .unwrap()
                    .value,
                None
            );
        }
    });
}

#[test]
fn native_default_missing_observation_needs_no_allocation_or_error_replacement() {
    use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
    use molt_cpython_abi::abi_types::{Py_TPFLAGS_READY, PyObject, PyType_Type, PyTypeObject};
    use molt_cpython_abi::api::{errors, mapping, object, refcount};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    // These physical declarations outlive the runtime transaction and every
    // native namespace/reference it retires. Their initial references are ours.
    let mut native_type: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    let mut native_object: Box<PyObject> = Box::new(unsafe { std::mem::zeroed() });
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            native_type.ob_base.ob_base.ob_refcnt = 1;
            native_type.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            native_type.tp_name = c"ForeignLookup".as_ptr();
            native_type.tp_flags = Py_TPFLAGS_READY;
            native_type.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
            native_type.tp_getattro = Some(object::PyObject_GenericGetAttr);
            native_type.tp_dict = mapping::PyDict_New();
            assert!(!native_type.tp_dict.is_null());
            native_object.ob_refcnt = 1;
            native_object.ob_type = &raw mut *native_type;
            let receiver = GLOBAL_BRIDGE
                .molt_value_for_pyobj(&raw mut *native_object)
                .unwrap();
            let name = crate::NameId::DunderDeepcopy.bits(py);
            let native_name = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(name);
            assert!(!native_name.is_null());
            let mut output = std::ptr::null_mut();
            // Publish all lazy namespace/projection prerequisites before bounding
            // allocations. Both the foreign runtime and native C observers then
            // have to report a miss without manufacturing AttributeError.
            assert_eq!(
                super::lookup_attribute(py, receiver, name),
                super::AttributeLookup::Missing
            );
            assert_eq!(
                object::PyObject_GetOptionalAttr(
                    &raw mut *native_object,
                    native_name,
                    &raw mut output
                ),
                0
            );
            assert!(output.is_null());
            assert_eq!(
                object::PyObject_GetOptionalAttr(
                    (&raw mut *native_type).cast(),
                    native_name,
                    &raw mut output
                ),
                0
            );
            assert!(output.is_null());
            assert!(errors::PyErr_Occurred().is_null());
            let reset = crate::test_support::restore_unlimited_tracker_on_drop();
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_allocations: Some(0),
                ..ResourceLimits::default()
            })));
            assert_eq!(
                super::lookup_attribute(py, receiver, name),
                super::AttributeLookup::Missing
            );
            assert_eq!(
                object::PyObject_GetOptionalAttr(
                    &raw mut *native_object,
                    native_name,
                    &raw mut output
                ),
                0
            );
            assert!(output.is_null());
            assert_eq!(
                object::PyObject_GetOptionalAttr(
                    (&raw mut *native_type).cast(),
                    native_name,
                    &raw mut output
                ),
                0
            );
            assert!(output.is_null());
            assert!(!crate::exception_pending(py));
            drop(reset);

            crate::raise_exception::<()>(
                py,
                crate::BuiltinExc::ValueError,
                "incoming lookup error",
            );
            let incoming = crate::exception_last_bits_noinc(py).unwrap();
            assert_eq!(
                super::lookup_attribute(py, receiver, name),
                super::AttributeLookup::Missing
            );
            assert_eq!(crate::exception_last_bits_noinc(py), Some(incoming));
            assert_eq!(
                object::PyObject_HasAttr(&raw mut *native_object, native_name),
                0
            );
            assert!(
                !crate::exception_pending(py),
                "legacy C hasattr clears incoming errors"
            );
            crate::clear_exception(py);
            assert!(errors::PyErr_Occurred().is_null());

            // The required boundary still raises, while neither optional probe did.
            let value = object::PyObject_GetAttr(&raw mut *native_object, native_name);
            assert!(value.is_null());
            assert_ne!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_AttributeError).cast()
                ),
                0
            );
            errors::PyErr_Clear();
            crate::clear_exception(py);
            dec_ref_bits(py, receiver);
            refcount::Py_DECREF(native_type.tp_dict);
            native_type.tp_dict = std::ptr::null_mut();
            assert_eq!(native_object.ob_refcnt, 1);
        }
    });
}
