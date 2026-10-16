use super::*;

fn class(py: &PyToken<'_>, name: &[u8], base: u64, slots: bool) -> u64 {
    let name = MoltObject::from_ptr(alloc_string(py, name)).bits();
    let class = crate::molt_class_new(name);
    dec_ref_bits(py, name);
    crate::molt_class_set_base(class, base);
    if slots {
        let field = attr_name_bits_from_bytes(py, b"field").unwrap();
        let dict = attr_name_bits_from_bytes(py, b"__dict__").unwrap();
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
        let key = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
        crate::molt_set_attr_name(class, key, names);
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
            let key = attr_name_bits_from_bytes(py, b"self_cycle").unwrap();
            crate::molt_set_attr_name(object, key, object);
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
                let field = attr_name_bits_from_bytes(py, b"field").unwrap();
                let target = MoltObject::from_ptr(alloc_list(py, &[])).bits();
                crate::molt_set_attr_name(object, field, target);
                let extra = attr_name_bits_from_bytes(py, b"extra").unwrap();
                crate::molt_set_attr_name(object, extra, target);
                let class_key = attr_name_bits_from_bytes(py, b"__class__").unwrap();
                crate::molt_set_attr_name(object, class_key, second);
                assert!(!exception_pending(py));
                assert_eq!(type_of_bits(py, object), second);
                assert_eq!(unsafe { field_base(ptr) }, base);
                let loaded = crate::molt_get_attr_name(object, field);
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
                attr_name_bits_from_bytes(py, if has_slot { b"field" } else { b"extra" }).unwrap();
            for length in [0, 1, 7, 8, 9, 15, 16, 17] {
                let input = value(py, kind, length);
                let object = construct(py, class, input);
                let ptr = obj_from_bits(object).as_ptr().unwrap();
                unsafe {
                    let data = string_bytes(ptr);
                    assert!(field_base(ptr) as usize > data.add(length) as usize);
                    assert_eq!(*data.add(length), 0);
                }
                crate::molt_set_attr_name(object, key, MoltObject::from_int(0x41).bits());
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
            assert!(obj_from_bits(crate::molt_object_new_bound(class)).is_none());
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            let extent = unsafe { crate::object::layout::class_cached_layout_size(ptr) }.unwrap();
            assert!(
                obj_from_bits(crate::object::builders::alloc_class_instance(
                    py, extent, class
                ))
                .is_none()
            );
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            if matches!(
                kind,
                NativePayload::Tuple | NativePayload::String | NativePayload::Bytes
            ) {
                let refs = unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() };
                assert!(unsafe { alloc_unpublished(py, class, kind, usize::MAX) }.is_null());
                assert!(exception_pending(py));
                crate::molt_exception_clear();
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
            let field = attr_name_bits_from_bytes(py, b"field").unwrap();
            let extra = attr_name_bits_from_bytes(py, b"extra").unwrap();
            let marker = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            crate::molt_set_attr_name(object, field, marker);
            crate::molt_set_attr_name(object, extra, marker);
            assert!(!exception_pending(py));
            assert_eq!(unsafe { file_handle_ptr(ptr) }, native);
            assert!(
                unsafe { field_base(ptr) } as usize
                    >= ptr as usize + size_of::<*mut MoltFileHandle>()
            );
            let loaded = crate::molt_get_attr_name(object, field);
            assert_eq!(loaded, marker);
            dec_ref_bits(py, loaded);
            let output = crate::builtins::io::molt_file_getvalue(object);
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
            crate::molt_set_attr_name(object, field, object);
            crate::molt_set_attr_name(object, extra, object);
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
