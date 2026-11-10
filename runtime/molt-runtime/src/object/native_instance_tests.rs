use super::*;

fn class(py: &PyToken<'_>, name: &[u8], base: u64, slots: bool) -> u64 {
    let name = MoltObject::from_ptr(alloc_string(py, name)).bits();
    let class = crate::molt_class_new_attached_body(py, name);
    dec_ref_bits(py, name);
    crate::molt_class_set_base_attached_body(py, class, base);
    if slots {
        let field = dynamic_attr_name_bits(py, b"field").unwrap();
        let dict = dynamic_attr_name_bits(py, b"__dict__").unwrap();
        let inherited_dictionary = unsafe {
            crate::object::layout::class_slot_policy(obj_from_bits(base).as_ptr().unwrap())
                .unwrap()
                .allows_dict
        };
        let fields = [field, dict];
        let names = MoltObject::from_ptr(alloc_tuple(
            py,
            &fields[..if inherited_dictionary { 1 } else { 2 }],
        ))
        .bits();
        let key = dynamic_attr_name_bits(py, b"__slots__").unwrap();
        crate::molt_set_attr_name_attached_body(py, class, key, names);
        for bits in [field, dict, names, key] {
            dec_ref_bits(py, bits);
        }
    }
    let ptr = obj_from_bits(class).as_ptr().unwrap();
    unsafe { crate::object::class_finish_definition(py, ptr) }.unwrap();
    assert_eq!(
        unsafe { crate::object::class_instance_type_id(ptr) },
        unsafe { crate::object::class_instance_type_id(obj_from_bits(base).as_ptr().unwrap()) }
    );
    assert!(!exception_pending(py));
    class
}

fn value(py: &PyToken<'_>, kind: NativePayload, length: usize) -> u64 {
    let bytes = vec![b'x'; length];
    match kind {
        NativePayload::String => MoltObject::from_ptr(alloc_string(py, &bytes)).bits(),
        NativePayload::Bytes | NativePayload::Bytearray => {
            MoltObject::from_ptr(alloc_bytes(py, &bytes)).bits()
        }
        NativePayload::Complex => MoltObject::from_int(7).bits(),
        _ => MoltObject::from_ptr(alloc_tuple(py, &[MoltObject::from_int(7).bits()])).bits(),
    }
}

fn construct(py: &PyToken<'_>, class: u64, input: u64) -> u64 {
    let result =
        unsafe { crate::call::bind::call_bind_borrowed(py, class, None, &[input], &[], &[]) };
    assert!(!exception_pending(py));
    assert_eq!(type_of_bits(py, result), class);
    result
}

#[test]
fn native_subtype_constructor_family_tracks_cycles_and_retires_membership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        for kind in [
            NativePayload::Tuple,
            NativePayload::String,
            NativePayload::Bytes,
            NativePayload::Bytearray,
            NativePayload::Set,
            NativePayload::Frozenset,
            NativePayload::Complex,
        ] {
            let class = class(py, b"NativeCycle", kind.owner(py).unwrap(), false);
            let input = value(py, kind, 9);
            let object = construct(py, class, input);
            let ptr = obj_from_bits(object).as_ptr().unwrap();
            assert_eq!(unsafe { object_type_id(ptr) }, kind.type_id());
            assert!(unsafe { has_fields(ptr) });
            assert!(unsafe { crate::object::gc::gc_is_tracked(ptr) });
            let key = dynamic_attr_name_bits(py, b"self_cycle").unwrap();
            crate::molt_set_attr_name_attached_body(py, object, key, object);
            assert!(!exception_pending(py));
            assert_ne!(unsafe { crate::object::instance_dict_bits(ptr) }, 0);
            dec_ref_bits(py, key);
            dec_ref_bits(py, object);
            let _ = unsafe { crate::object::gc::collect_cycles(py) };
            // Registry lookup treats the old pointer as an opaque key.
            assert!(!unsafe { crate::object::gc::gc_is_tracked(ptr) });
            dec_ref_bits(py, input);
            dec_ref_bits(py, class);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn native_subtype_slots_align_and_survive_dictionary_and_class_changes() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for kind in [
            NativePayload::String,
            NativePayload::Bytearray,
            NativePayload::Set,
            NativePayload::Frozenset,
            NativePayload::Complex,
        ] {
            let first = class(py, b"NativeFirst", kind.owner(py).unwrap(), true);
            let second = class(py, b"NativeSecond", kind.owner(py).unwrap(), true);
            for length in [0, 1, 4, 7, 8, 9] {
                let input = value(py, kind, length);
                let object = construct(py, first, input);
                let ptr = obj_from_bits(object).as_ptr().unwrap();
                let base = unsafe { field_base(ptr) };
                assert_eq!(base as usize % WORD, 0);
                assert!(base as usize > ptr as usize);
                let field = dynamic_attr_name_bits(py, b"field").unwrap();
                let target = MoltObject::from_ptr(alloc_list(py, &[])).bits();
                crate::molt_set_attr_name_attached_body(py, object, field, target);
                let extra = dynamic_attr_name_bits(py, b"extra").unwrap();
                crate::molt_set_attr_name_attached_body(py, object, extra, target);
                let class_key = dynamic_attr_name_bits(py, b"__class__").unwrap();
                crate::molt_set_attr_name_attached_body(py, object, class_key, second);
                assert!(!exception_pending(py));
                assert_eq!(type_of_bits(py, object), second);
                assert_eq!(unsafe { field_base(ptr) }, base);
                let loaded = crate::molt_get_attr_name_attached_body(py, object, field);
                assert_eq!(loaded, target);
                dec_ref_bits(py, loaded);
                let dictionary = unsafe { crate::object::instance_dict_bits(ptr) };
                let mut edges = Vec::new();
                unsafe {
                    crate::object::heap_lifecycle::visit_owned_values(py, ptr, &mut |bits| {
                        edges.push(bits)
                    })
                };
                assert!(edges.contains(&second));
                assert!(edges.contains(&target));
                assert!(edges.contains(&dictionary));
                unsafe { crate::object::heap_lifecycle::clear_cycle_edges(py, ptr) };
                assert_eq!(unsafe { crate::object::instance_dict_bits(ptr) }, 0);
                assert_eq!(unsafe { *base.cast::<u64>() }, missing_bits(py));
                for bits in [field, extra, class_key, target, object, input] {
                    dec_ref_bits(py, bits);
                }
                assert!(!unsafe { crate::object::gc::gc_is_tracked(ptr) });
            }
            dec_ref_bits(py, second);
            dec_ref_bits(py, first);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn native_inline_terminator_precedes_slots_and_the_dictionary_through_c_abi_access() {
    use molt_cpython_abi::api::{refcount, strings};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for kind in [NativePayload::String, NativePayload::Bytes] {
            // Bytes subclasses inherit a dictionary but reject nonempty slots.
            let has_slot = kind == NativePayload::String;
            let class = class(py, b"TerminatedNative", kind.owner(py).unwrap(), has_slot);
            let key =
                dynamic_attr_name_bits(py, if has_slot { b"field" } else { b"extra" }).unwrap();
            for length in [0, 1, 7, 8, 9, 15, 16, 17] {
                let input = value(py, kind, length);
                let object = construct(py, class, input);
                let ptr = obj_from_bits(object).as_ptr().unwrap();
                unsafe {
                    let data = string_bytes(ptr);
                    assert!(field_base(ptr) as usize > data.add(length) as usize);
                    assert_eq!(*data.add(length), 0);
                }
                crate::molt_set_attr_name_attached_body(
                    py,
                    object,
                    key,
                    MoltObject::from_int(0x41).bits(),
                );
                assert!(!exception_pending(py));
                unsafe {
                    assert_eq!(*string_bytes(ptr).add(length), 0);
                    if kind == NativePayload::Bytes {
                        let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                            .borrowed_handle_to_new_pyobj(object);
                        assert!(!view.is_null());
                        let mut data = std::ptr::null_mut();
                        let mut c_length = -1;
                        assert_eq!(
                            strings::PyBytes_AsStringAndSize(
                                view,
                                &raw mut data,
                                &raw mut c_length
                            ),
                            0
                        );
                        assert_eq!(c_length, length as isize);
                        assert_eq!(data.cast::<u8>().cast_const(), bytes_data(ptr));
                        assert_eq!(strings::PyBytes_AS_STRING(view), data);
                        assert_eq!(*data.add(length), 0);
                        refcount::Py_DECREF(view);
                    }
                }
                dec_ref_bits(py, object);
                dec_ref_bits(py, input);
            }
            dec_ref_bits(py, key);
            dec_ref_bits(py, class);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn native_subtype_rejects_generic_allocation_and_overflow_without_class_attachment() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for kind in [
            NativePayload::Tuple,
            NativePayload::String,
            NativePayload::Bytes,
            NativePayload::Bytearray,
            NativePayload::Set,
            NativePayload::Frozenset,
            NativePayload::Complex,
        ] {
            let class = class(py, b"NativeAdmission", kind.owner(py).unwrap(), false);
            let ptr = obj_from_bits(class).as_ptr().unwrap();
            assert!(obj_from_bits(crate::molt_object_new_bound_attached_body(py, class)).is_none());
            assert!(exception_pending(py));
            crate::molt_exception_clear_attached_body(py);
            let extent = unsafe { crate::object::layout::class_cached_layout_size(ptr) }.unwrap();
            assert!(
                obj_from_bits(crate::object::builders::alloc_class_instance(
                    py, extent, class
                ))
                .is_none()
            );
            assert!(exception_pending(py));
            crate::molt_exception_clear_attached_body(py);
            if matches!(
                kind,
                NativePayload::Tuple | NativePayload::String | NativePayload::Bytes
            ) {
                let refs = unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() };
                assert!(unsafe { alloc_unpublished(py, class, kind, usize::MAX) }.is_null());
                assert!(exception_pending(py));
                crate::molt_exception_clear_attached_body(py);
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() },
                    refs
                );
            }
            dec_ref_bits(py, class);
        }
        for kind in [
            NativePayload::String,
            NativePayload::Bytes,
            NativePayload::Bytearray,
            NativePayload::Complex,
        ] {
            let input = value(py, kind, 4);
            let exact = construct(py, kind.owner(py).unwrap(), input);
            let ptr = obj_from_bits(exact).as_ptr().unwrap();
            assert!(!unsafe { has_fields(ptr) });
            assert!(!unsafe { crate::object::gc::gc_is_tracked(ptr) });
            dec_ref_bits(py, exact);
            dec_ref_bits(py, input);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn native_io_subtype_slots_preserve_payload_dictionary_and_cycle_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        let classes = builtin_classes(py);
        for root in [
            classes.file,
            classes.file_io,
            classes.buffered_reader,
            classes.buffered_writer,
            classes.buffered_random,
            classes.text_io_wrapper,
            classes.bytes_io,
            classes.string_io,
        ] {
            assert_eq!(
                unsafe {
                    crate::object::class_instance_type_id(obj_from_bits(root).as_ptr().unwrap())
                },
                TYPE_ID_FILE_HANDLE
            );
        }
        for (root, text) in [(classes.bytes_io, false), (classes.string_io, true)] {
            let input = MoltObject::from_ptr(if text {
                alloc_string(py, b"payload")
            } else {
                alloc_bytes(py, b"payload")
            })
            .bits();
            let exact = construct(py, root, input);
            let exact_ptr = obj_from_bits(exact).as_ptr().unwrap();
            assert!(!unsafe { has_fields(exact_ptr) });
            assert_eq!(
                unsafe { object_payload_size(exact_ptr) },
                WORD // A 32-bit host pointer is padded to the existing allocation word.
            );
            assert_eq!(unsafe { object_class_bits(exact_ptr) }, root);
            dec_ref_bits(py, exact);

            let subclass = class(py, b"SlottedIO", root, true);
            let object = construct(py, subclass, input);
            let ptr = obj_from_bits(object).as_ptr().unwrap();
            let native = unsafe { file_handle_ptr(ptr) };
            assert!(!native.is_null());
            let state = unsafe { std::sync::Arc::downgrade((*native).state.as_ref().unwrap()) };
            let field = dynamic_attr_name_bits(py, b"field").unwrap();
            let extra = dynamic_attr_name_bits(py, b"extra").unwrap();
            let marker = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            crate::molt_set_attr_name_attached_body(py, object, field, marker);
            crate::molt_set_attr_name_attached_body(py, object, extra, marker);
            assert!(!exception_pending(py));
            assert_eq!(unsafe { file_handle_ptr(ptr) }, native);
            assert!(
                unsafe { field_base(ptr) } as usize
                    >= ptr as usize + size_of::<*mut MoltFileHandle>()
            );
            let loaded = crate::molt_get_attr_name_attached_body(py, object, field);
            assert_eq!(loaded, marker);
            dec_ref_bits(py, loaded);
            let output = crate::builtins::io::molt_file_getvalue_attached_body(py, object);
            assert!(!exception_pending(py));
            if text {
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(output)).as_deref(),
                    Some("payload")
                );
            } else {
                let bytes = obj_from_bits(output).as_ptr().unwrap();
                assert_eq!(unsafe { object_type_id(bytes) }, TYPE_ID_BYTES);
                assert_eq!(
                    unsafe { std::slice::from_raw_parts(bytes_data(bytes), bytes_len(bytes)) },
                    b"payload"
                );
            }
            dec_ref_bits(py, output);
            let dictionary = unsafe { crate::object::instance_dict_bits(ptr) };
            assert_eq!(dictionary, unsafe { (*native).dict_bits });
            let mut edges = Vec::new();
            unsafe {
                crate::object::heap_lifecycle::visit_owned_values(py, ptr, &mut |bits| {
                    edges.push(bits)
                })
            };
            assert_eq!(edges.iter().filter(|&&bits| bits == dictionary).count(), 1);
            assert_eq!(edges.iter().filter(|&&bits| bits == marker).count(), 1);
            assert_eq!(edges.iter().filter(|&&bits| bits == subclass).count(), 1);
            // Both real owners become cycles. Collection must retire the native
            // resource after clearing the slot and the sole public dictionary.
            crate::molt_set_attr_name_attached_body(py, object, field, object);
            crate::molt_set_attr_name_attached_body(py, object, extra, object);
            assert!(!exception_pending(py));
            assert_eq!(
                unsafe {
                    (*header_from_obj_ptr(obj_from_bits(marker).as_ptr().unwrap()))
                        .ref_count_snapshot()
                },
                1
            );
            for bits in [marker, field, extra, object] {
                dec_ref_bits(py, bits);
            }
            let _ = unsafe { crate::object::gc::collect_cycles(py) };
            assert!(!unsafe { crate::object::gc::gc_is_tracked(ptr) });
            assert!(state.upgrade().is_none());
            dec_ref_bits(py, input);
            dec_ref_bits(py, subclass);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn native_declared_slots_share_allocation_projection_and_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        for kind in [
            NativePayload::Tuple,
            NativePayload::String,
            NativePayload::Bytes,
            NativePayload::Bytearray,
            NativePayload::Set,
            NativePayload::Frozenset,
            NativePayload::Complex,
        ] {
            let roots: Vec<_> = builtin_classes(py)
                .anchors()
                .into_iter()
                .filter(|&bits| unsafe {
                    crate::object::class_instance_type_id(obj_from_bits(bits).as_ptr().unwrap())
                        == kind.type_id()
                })
                .collect();
            assert_eq!(roots, vec![kind.owner(py).unwrap()]);
            let name = dynamic_attr_name_bits(py, b"DeclaredNativeTail").unwrap();
            let field = dynamic_attr_name_bits(py, b"retained").unwrap();
            let class = crate::molt_class_new_attached_body(py, name);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let policy = crate::object::class_storage::ClassSlotPolicy {
                variable_sized: matches!(
                    kind,
                    NativePayload::Tuple | NativePayload::String | NativePayload::Bytes
                ),
                ..Default::default()
            };
            unsafe {
                crate::object::class_storage::class_declare_native_slots(class_ptr, policy);
            }
            crate::molt_class_set_base_attached_body(py, class, kind.owner(py).unwrap());
            let names = MoltObject::from_ptr(alloc_tuple(py, &[field])).bits();
            let declaration =
                MoltObject::from_ptr(alloc_tuple(py, &[names, policy.encode()])).bits();
            unsafe {
                crate::object::layout::class_set_slot_declaration_owned(class_ptr, declaration);
                crate::object::class_finish_definition(py, class_ptr).unwrap();
            }
            dec_ref_bits(py, names);
            for length in [0, 9] {
                let input = value(py, kind, length);
                let object = construct(py, class, input);
                let ptr = obj_from_bits(object).as_ptr().unwrap();
                // NativeSlotLayout describes this declaration, not a root.
                // Fail before any field write if the read projection disagrees.
                assert!(unsafe { has_fields(ptr) });
                assert!(unsafe { crate::object::gc::gc_is_tracked(ptr) });
                let base = unsafe { field_base(ptr) };
                assert!(base as usize > ptr as usize);
                let native_bytes = unsafe {
                    std::slice::from_raw_parts(ptr, base as usize - ptr as usize).to_vec()
                };
                let marker = MoltObject::from_ptr(alloc_list(py, &[])).bits();
                let marker_ptr = obj_from_bits(marker).as_ptr().unwrap();
                let refs = unsafe { (*header_from_obj_ptr(marker_ptr)).ref_count_snapshot() };
                crate::molt_set_attr_name_attached_body(py, object, field, marker);
                assert!(!exception_pending(py));
                let selected = crate::molt_get_attr_name_attached_body(py, object, field);
                assert_eq!(selected, marker);
                dec_ref_bits(py, selected);
                assert_eq!(
                    unsafe { std::slice::from_raw_parts(ptr, native_bytes.len()) },
                    native_bytes.as_slice(),
                    "declared slots must not overwrite the native payload",
                );
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(marker_ptr)).ref_count_snapshot() },
                    refs + 1,
                );
                dec_ref_bits(py, object);
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(marker_ptr)).ref_count_snapshot() },
                    refs,
                    "physical field retirement releases its owner",
                );
                dec_ref_bits(py, marker);

                let cycle = construct(py, class, input);
                let cycle_ptr = obj_from_bits(cycle).as_ptr().unwrap();
                crate::molt_set_attr_name_attached_body(py, cycle, field, cycle);
                assert!(!exception_pending(py));
                dec_ref_bits(py, cycle);
                let _ = unsafe { crate::object::gc::collect_cycles(py) };
                assert!(!unsafe { crate::object::gc::gc_is_tracked(cycle_ptr) });
                dec_ref_bits(py, input);
            }
            for bits in [field, name, class] {
                dec_ref_bits(py, bits);
            }
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn native_class_edge_publication_refuses_an_unallocated_tail_atomically() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let name = dynamic_attr_name_bits(py, b"NativePublicationExtent").unwrap();
        let class = crate::molt_class_new_attached_body(py, name);
        dec_ref_bits(py, name);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        let policy = crate::object::class_storage::ClassSlotPolicy {
            variable_sized: true,
            ..Default::default()
        };
        unsafe { crate::object::class_storage::class_declare_native_slots(class_ptr, policy) };
        crate::molt_class_set_base_attached_body(py, class, builtin_classes(py).tuple);
        let mut names = Vec::new();
        for index in 0..32 {
            names.push(dynamic_attr_name_bits(py, format!("field{index}").as_bytes()).unwrap());
        }
        let slots = MoltObject::from_ptr(alloc_tuple(py, &names)).bits();
        let declaration = MoltObject::from_ptr(alloc_tuple(py, &[slots, policy.encode()])).bits();
        unsafe {
            crate::object::layout::class_set_slot_declaration_owned(class_ptr, declaration);
            crate::object::class_finish_definition(py, class_ptr).unwrap();
        }
        for bits in names.into_iter().chain([slots]) {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
        let item = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        let exact = crate::object::builders::alloc_tuple_unpublished(py, &[item], false, None);
        assert!(!exact.is_null());
        let refs = unsafe { (*header_from_obj_ptr(class_ptr)).ref_count_snapshot() };
        unsafe {
            assert!(!crate::object::object_init_class_edge_unpublished(
                py,
                exact,
                class,
                crate::object::ClassEdgeOwnership::Owned,
            ));
            assert_eq!(object_class_bits(exact), 0);
            assert_eq!((*header_from_obj_ptr(class_ptr)).ref_count_snapshot(), refs);
            // The rejected publication did not touch payload ownership. Exact
            // root identity still publishes, then rejects the same bad change.
            assert!(crate::object::object_init_class_edge_unpublished(
                py,
                exact,
                builtin_classes(py).tuple,
                crate::object::ClassEdgeOwnership::Borrowed,
            ));
            crate::object::gc::gc_publish_initialized(py, exact);
            assert!(!crate::object::object_replace_class_edge(
                py,
                exact,
                class,
                crate::object::ClassEdgeOwnership::Owned,
            ));
            assert_eq!(object_class_bits(exact), builtin_classes(py).tuple);
            assert_eq!((*header_from_obj_ptr(class_ptr)).ref_count_snapshot(), refs);
            assert_eq!(crate::object::seq_access::item(exact, 0), Some(item));
        }
        let correct = unsafe { crate::object::builders::alloc_tuple_subclass(py, class, &[item]) };
        assert!(!exception_pending(py));
        assert_eq!(type_of_bits(py, correct), class);
        dec_ref_bits(py, correct);
        dec_ref_bits(py, MoltObject::from_ptr(exact).bits());
        dec_ref_bits(py, item);
        dec_ref_bits(py, class);
    });
}

#[test]
fn code_replacement_metadata_subclasses_form_collectible_cycles() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        let text_class = class(py, b"CodeMetadataText", builtin_classes(py).str, false);
        let input = MoltObject::from_ptr(alloc_string(py, b"metadata")).bits();
        let names = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
        for field in [
            crate::NameId::CoName,
            crate::NameId::CoFilename,
            crate::NameId::CoQualname,
        ] {
            let source = crate::alloc_code_obj(
                py,
                input,
                input,
                1,
                MoltObject::none().bits(),
                names,
                names,
                0,
                0,
                0,
            );
            assert!(!source.is_null());
            assert!(
                !unsafe { crate::object::gc::gc_is_tracked(source) },
                "exact immutable metadata must retain the untracked path"
            );
            let source_bits = MoltObject::from_ptr(source).bits();
            let text = construct(py, text_class, input);
            let text_ptr = obj_from_bits(text).as_ptr().unwrap();
            let key = field.owned_bits(py).unwrap();
            let args = MoltObject::from_ptr(alloc_tuple(py, &[source_bits])).bits();
            let kwargs =
                MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[key, text])).bits();
            let replaced =
                crate::builtins::functions::molt_code_replace_attached_body(py, args, kwargs);
            assert!(!exception_pending(py));
            let replaced_ptr = obj_from_bits(replaced).as_ptr().unwrap();
            assert!(
                unsafe { crate::object::gc::gc_is_tracked(replaced_ptr) },
                "a metadata subclass with mutable fields must promote code tracking"
            );
            let actual = crate::molt_get_attr_name_attached_body(py, replaced, key);
            assert_eq!(actual, text, "replacement must retain subclass identity");
            dec_ref_bits(py, actual);
            let backref = dynamic_attr_name_bits(py, b"code_backref").unwrap();
            crate::molt_set_attr_name_attached_body(py, text, backref, replaced);
            assert!(!exception_pending(py));
            for bits in [backref, args, kwargs, key, source_bits, replaced, text] {
                dec_ref_bits(py, bits);
            }
            let _ = unsafe { crate::object::gc::collect_cycles(py) };
            // Registry lookups treat retired pointers only as opaque keys.
            assert!(!unsafe { crate::object::gc::gc_is_tracked(replaced_ptr) });
            assert!(!unsafe { crate::object::gc::gc_is_tracked(text_ptr) });
            assert!(!exception_pending(py));
        }
        for bits in [names, input, text_class] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn code_metadata_publication_keeps_earlier_container_owners_collectible() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        let text_class = class(py, b"LateCodeMetadataText", builtin_classes(py).str, false);
        let input = MoltObject::from_ptr(alloc_string(py, b"metadata")).bits();
        let names = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
        let code = crate::alloc_code_obj(
            py,
            input,
            input,
            1,
            MoltObject::none().bits(),
            names,
            names,
            0,
            0,
            0,
        );
        let code_bits = MoltObject::from_ptr(code).bits();
        assert!(!unsafe { crate::object::gc::gc_is_tracked(code) });
        let owner = alloc_tuple(py, &[code_bits]);
        let owner_bits = MoltObject::from_ptr(owner).bits();
        assert!(
            unsafe { crate::object::gc::gc_is_tracked(owner) },
            "an earlier owner must account for pending metadata publication"
        );
        let text = construct(py, text_class, input);
        let text_ptr = obj_from_bits(text).as_ptr().unwrap();
        unsafe {
            crate::object::layout::code_publish_declaration_metadata(
                py,
                code,
                text,
                MoltObject::none().bits(),
            )
        }
        .unwrap();
        assert!(unsafe { crate::object::gc::gc_is_tracked(code) });
        let key = dynamic_attr_name_bits(py, b"owner_backref").unwrap();
        crate::molt_set_attr_name_attached_body(py, text, key, owner_bits);
        assert!(!exception_pending(py));
        for bits in [key, code_bits, owner_bits, text] {
            dec_ref_bits(py, bits);
        }
        let _ = unsafe { crate::object::gc::collect_cycles(py) };
        for retired in [code, owner, text_ptr] {
            assert!(!unsafe { crate::object::gc::gc_is_tracked(retired) });
        }
        for bits in [input, names, text_class] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn code_sealing_preserves_atomic_container_fast_paths() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(py, {
        let input = MoltObject::from_ptr(alloc_string(py, b"sealed_metadata")).bits();
        let names = MoltObject::from_ptr(alloc_tuple(py, &[input])).bits();
        assert!(unsafe {
            crate::object::gc::gc_is_tracked(obj_from_bits(names).as_ptr().unwrap())
        });
        let code = crate::alloc_code_obj(
            py,
            input,
            input,
            1,
            MoltObject::none().bits(),
            names,
            names,
            0,
            0,
            0,
        );
        let code_bits = MoltObject::from_ptr(code).bits();
        assert!(
            !unsafe { crate::object::gc::gc_is_tracked(code) },
            "a conservatively tracked tuple of atomic names must not enroll code"
        );
        unsafe {
            crate::object::layout::code_publish_declaration_metadata(
                py,
                code,
                input,
                MoltObject::none().bits(),
            )
            .unwrap();
            crate::object::layout::code_publish_policy(
                code,
                crate::object::layout::CodeExecutionKind::Direct,
                0,
            )
            .unwrap();
        }
        let tuple = alloc_tuple(py, &[code_bits]);
        let _ = unsafe { crate::object::gc::collect_cycles(py) };
        assert!(
            !unsafe { crate::object::gc::gc_is_tracked(tuple) },
            "sealed atomic code must permit immutable container demotion"
        );
        let dictionary = crate::alloc_dict_with_pairs(py, &[input, code_bits]);
        if crate::object::ops_sys::runtime_target_minor(py) < 14 {
            assert!(!unsafe { crate::object::gc::gc_is_tracked(dictionary) });
        }
        // A late compiler descriptor can add immutable bytes, never a mutable
        // subclass that invalidates the earlier owner's atomicity proof.
        let text_class = class(py, b"LateGpuText", builtin_classes(py).str, false);
        let text = construct(py, text_class, input);
        assert!(!unsafe { crate::object::layout::code_publish_gpu_descriptor(py, code, text) });
        assert!(exception_pending(py));
        crate::clear_exception(py);
        assert!(unsafe { crate::object::layout::code_publish_gpu_descriptor(py, code, input) });
        let mutable_names = MoltObject::from_ptr(alloc_tuple(py, &[text])).bits();
        let mutable_code = crate::alloc_code_obj(
            py,
            input,
            input,
            1,
            MoltObject::none().bits(),
            mutable_names,
            names,
            0,
            0,
            0,
        );
        assert!(
            unsafe { crate::object::gc::gc_is_tracked(mutable_code) },
            "string-subclass children in name tuples must still enroll code"
        );
        let mutable_code_bits = MoltObject::from_ptr(mutable_code).bits();
        let text_ptr = obj_from_bits(text).as_ptr().unwrap();
        let backref = dynamic_attr_name_bits(py, b"name_tuple_backref").unwrap();
        crate::molt_set_attr_name_attached_body(py, text, backref, mutable_code_bits);
        for bits in [
            MoltObject::from_ptr(tuple).bits(),
            MoltObject::from_ptr(dictionary).bits(),
            code_bits,
            text,
            text_class,
            input,
            names,
            mutable_names,
            mutable_code_bits,
            backref,
        ] {
            dec_ref_bits(py, bits);
        }
        let _ = unsafe { crate::object::gc::collect_cycles(py) };
        assert!(!unsafe { crate::object::gc::gc_is_tracked(mutable_code) });
        assert!(!unsafe { crate::object::gc::gc_is_tracked(text_ptr) });
        assert!(!exception_pending(py));
    });
}
