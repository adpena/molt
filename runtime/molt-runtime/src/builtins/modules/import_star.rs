//! CPython IMPORT_STAR uses indexed reads, not the iterator protocol.
use crate::*;

pub(super) fn import_star(py: &PyToken<'_>, source: u64, destination: u64) -> u64 {
    let none = MoltObject::none().bits();
    let Some(src) = obj_from_bits(source).as_ptr() else {
        return raise_exception::<_>(py, "TypeError", "module import expects module");
    };
    let Some(dst) = obj_from_bits(destination).as_ptr() else {
        return raise_exception::<_>(py, "TypeError", "module import expects module");
    };
    unsafe {
        if object_type_id(src) != TYPE_ID_MODULE || object_type_id(dst) != TYPE_ID_MODULE {
            return raise_exception::<_>(py, "TypeError", "module import expects module");
        }
        inc_ref_bits(py, source);
        let _src_owner = PtrDropGuard::new(src);
        inc_ref_bits(py, destination);
        let _dst_owner = PtrDropGuard::new(dst);
        let source_dict = module_dict_bits(src);
        let destination_dict = module_dict_bits(dst);
        let Some(src_dict) = obj_from_bits(source_dict)
            .as_ptr()
            .filter(|&p| object_type_id(p) == TYPE_ID_DICT)
        else {
            return raise_exception::<_>(py, "TypeError", "module dict missing");
        };
        let Some(dst_dict) = obj_from_bits(destination_dict)
            .as_ptr()
            .filter(|&p| object_type_id(p) == TYPE_ID_DICT)
        else {
            return raise_exception::<_>(py, "TypeError", "module dict missing");
        };
        inc_ref_bits(py, source_dict);
        let _src_dict_owner = PtrDropGuard::new(src_dict);
        inc_ref_bits(py, destination_dict);
        let _dst_dict_owner = PtrDropGuard::new(dst_dict);
        let all_name = intern_static_name(py, &runtime_state(py).interned.all_name, b"__all__");
        let explicit = crate::builtins::attr::attr_lookup_ptr_allow_missing(py, src, all_name);
        if exception_pending(py) {
            return none;
        }
        let skip_private = explicit.is_none();
        let names = if let Some(names) = explicit {
            names
        } else {
            // Snapshot keys before invoking arbitrary attribute callbacks. This
            // also preserves source==destination and partial namespace writes.
            let Some(keys) = crate::object::ops_dict::dict_snapshot(
                py,
                src_dict,
                crate::object::ops_dict::DictSnapshotKind::Keys,
            ) else {
                return none;
            };
            let ptr = alloc_tuple(py, &keys);
            if ptr.is_null() {
                return none;
            }
            MoltObject::from_ptr(ptr).bits()
        };
        let _names_owner = obj_from_bits(names).as_ptr().map(PtrDropGuard::new);
        let mut index = 0i64;
        loop {
            // Exhaustion is an implicit handler. Keep any caller's handled
            // exception context alive while consuming our own IndexError.
            let read_scope = crate::builtins::exceptions::ExceptionStackScope::push(py);
            let name = crate::object::sequence_index::sequence_item_at_index(py, names, index);
            let _name_owner = obj_from_bits(name).as_ptr().map(PtrDropGuard::new);
            if exception_pending(py) {
                let error = molt_exception_last_pending();
                let exhausted = crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "IndexError",
                );
                dec_ref_bits(py, error);
                if exhausted {
                    clear_exception(py);
                }
                return none;
            }
            drop(read_scope);
            let Some(text) = string_obj_to_owned(obj_from_bits(name)) else {
                // CPython fetches __name__ only on the invalid-name path; this
                // lookup is observable and may itself fail or return non-str.
                let Some(name_key) =
                    crate::builtins::attr::attr_name_bits_from_bytes(py, b"__name__")
                else {
                    return none;
                };
                let _name_key_owner = obj_from_bits(name_key).as_ptr().map(PtrDropGuard::new);
                let module_name_bits = molt_get_attr_name(source, name_key);
                let _module_name_owner = obj_from_bits(module_name_bits)
                    .as_ptr()
                    .map(PtrDropGuard::new);
                if exception_pending(py) {
                    return none;
                }
                let Some(module_name) = string_obj_to_owned(obj_from_bits(module_name_bits)) else {
                    let name_kind = class_name_for_error(type_of_bits(py, module_name_bits));
                    return raise_exception::<_>(
                        py,
                        "TypeError",
                        &format!("module __name__ must be a string, not {name_kind}"),
                    );
                };
                let kind = class_name_for_error(type_of_bits(py, name));
                let location = if skip_private { "Key" } else { "Item" };
                let attribute = if skip_private { "__dict__" } else { "__all__" };
                return raise_exception::<_>(
                    py,
                    "TypeError",
                    &format!("{location} in {module_name}.{attribute} must be str, not {kind}"),
                );
            };
            if !(skip_private && text.starts_with('_')) {
                let value = molt_get_attr_name(source, name);
                let _value_owner = obj_from_bits(value).as_ptr().map(PtrDropGuard::new);
                if exception_pending(py) {
                    return none;
                }
                dict_set_in_place(py, dst_dict, name, value);
                if exception_pending(py) {
                    return none;
                }
            }
            let Some(next) = index.checked_add(1) else {
                return raise_exception::<_>(
                    py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            };
            index = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_error(py: &PyToken<'_>, expected: &str) {
        assert!(exception_pending(py));
        let error = molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            py, error, expected
        ));
        clear_exception(py);
        dec_ref_bits(py, error);
    }

    extern "C" fn dynamic_module_attribute(name: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            match string_obj_to_owned(obj_from_bits(name)).as_deref() {
                Some("__all__") => {
                    let dynamic = MoltObject::from_ptr(alloc_string(py, b"dynamic")).bits();
                    let hidden = MoltObject::from_ptr(alloc_string(py, b"_hidden")).bits();
                    let names = MoltObject::from_ptr(alloc_list(py, &[dynamic, hidden])).bits();
                    dec_ref_bits(py, dynamic);
                    dec_ref_bits(py, hidden);
                    names
                }
                Some("dynamic") => MoltObject::from_int(99).bits(),
                _ => raise_exception::<_>(py, "AttributeError", "dynamic attribute missing"),
            }
        })
    }

    #[test]
    fn star_observes_dynamic_all_and_dynamic_export_attributes() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let module_name = MoltObject::from_ptr(alloc_string(py, b"star_dynamic")).bits();
                let src = alloc_module_obj(py, module_name);
                let dst = alloc_module_obj(py, module_name);
                let _src_owner = PtrDropGuard::new(src);
                let _dst_owner = PtrDropGuard::new(dst);
                let callback = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::builtins::functions::runtime_fn_addr(
                        "star_dynamic_module_attribute",
                        dynamic_module_attribute as *const (),
                    ),
                    1,
                );
                let callback = MoltObject::from_ptr(callback).bits();
                let getattr_name =
                    crate::builtins::attr::attr_name_bits_from_bytes(py, b"__getattr__").unwrap();
                let dynamic_name =
                    crate::builtins::attr::attr_name_bits_from_bytes(py, b"dynamic").unwrap();
                let hidden_name =
                    crate::builtins::attr::attr_name_bits_from_bytes(py, b"_hidden").unwrap();
                let src_dict = obj_from_bits(module_dict_bits(src)).as_ptr().unwrap();
                let dst_dict = obj_from_bits(module_dict_bits(dst)).as_ptr().unwrap();
                dict_set_in_place(py, src_dict, getattr_name, callback);
                dict_set_in_place(py, src_dict, hidden_name, MoltObject::from_int(11).bits());
                import_star(
                    py,
                    MoltObject::from_ptr(src).bits(),
                    MoltObject::from_ptr(dst).bits(),
                );
                assert!(!exception_pending(py));
                assert_eq!(
                    dict_get_in_place(py, dst_dict, dynamic_name),
                    Some(MoltObject::from_int(99).bits())
                );
                assert_eq!(
                    dict_get_in_place(py, dst_dict, hidden_name),
                    Some(MoltObject::from_int(11).bits())
                );
                for bits in [
                    module_name,
                    callback,
                    getattr_name,
                    dynamic_name,
                    hidden_name,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    static INDEX_MODE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    extern "C" fn indexed_name(_self: u64, index: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let mode = INDEX_MODE.load(std::sync::atomic::Ordering::SeqCst);
            if mode == 2 {
                return raise_exception::<_>(py, "StopIteration", "not sequence exhaustion");
            }
            if to_i64(obj_from_bits(index)) == Some(0) {
                return MoltObject::from_ptr(alloc_string(py, b"visible")).bits();
            }
            let kind = if mode == 1 {
                "ValueError"
            } else {
                "IndexError"
            };
            raise_exception::<_>(py, kind, "after first")
        })
    }

    extern "C" fn forbidden_iterator(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            raise_exception::<_>(py, "RuntimeError", "IMPORT_STAR must not call __iter__")
        })
    }

    fn indexed_names(py: &PyToken<'_>) -> u64 {
        let getitem = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "star_indexed_name",
                indexed_name as *const (),
            ),
            2,
        );
        let iter = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "star_forbidden_iterator",
                forbidden_iterator as *const (),
            ),
            1,
        );
        let getitem = MoltObject::from_ptr(getitem).bits();
        let iter = MoltObject::from_ptr(iter).bits();
        let getitem_name =
            crate::builtins::attr::attr_name_bits_from_bytes(py, b"__getitem__").unwrap();
        let iter_name = crate::builtins::attr::attr_name_bits_from_bytes(py, b"__iter__").unwrap();
        let class_name =
            crate::builtins::attr::attr_name_bits_from_bytes(py, b"IndexedNames").unwrap();
        let namespace = MoltObject::from_ptr(alloc_dict_with_pairs(
            py,
            &[getitem_name, getitem, iter_name, iter],
        ))
        .bits();
        let classes = crate::builtins::classes::builtin_classes(py);
        let bases = MoltObject::from_ptr(alloc_tuple(py, &[classes.object])).bits();
        let class = crate::builtins::types::molt_type_new(
            classes.type_obj,
            class_name,
            bases,
            namespace,
            MoltObject::none().bits(),
        );
        assert!(!exception_pending(py));
        let instance =
            unsafe { crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap()) };
        for bits in [
            getitem,
            iter,
            getitem_name,
            iter_name,
            class_name,
            namespace,
            bases,
            class,
        ] {
            dec_ref_bits(py, bits);
        }
        instance
    }

    #[test]
    fn star_uses_indexed_slots_and_only_index_error_exhaustion() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let _outer_scope = crate::builtins::exceptions::ExceptionStackScope::push(py);
                let outer = alloc_exception(py, "RuntimeError", "outer handled context");
                let outer_bits = MoltObject::from_ptr(outer).bits();
                crate::builtins::exceptions::exception_context_set(py, outer_bits);
                dec_ref_bits(py, outer_bits);
                for mode in 0..3 {
                    INDEX_MODE.store(mode, std::sync::atomic::Ordering::SeqCst);
                    let module_name =
                        MoltObject::from_ptr(alloc_string(py, b"star_indexed")).bits();
                    let src = alloc_module_obj(py, module_name);
                    let dst = alloc_module_obj(py, module_name);
                    let _src_owner = PtrDropGuard::new(src);
                    let _dst_owner = PtrDropGuard::new(dst);
                    let key = MoltObject::from_ptr(alloc_string(py, b"visible")).bits();
                    let names = indexed_names(py);
                    let _names_owner = obj_from_bits(names).as_ptr().map(PtrDropGuard::new);
                    let all_key =
                        intern_static_name(py, &runtime_state(py).interned.all_name, b"__all__");
                    let src_dict = obj_from_bits(module_dict_bits(src)).as_ptr().unwrap();
                    let dst_dict = obj_from_bits(module_dict_bits(dst)).as_ptr().unwrap();
                    dict_set_in_place(py, src_dict, key, MoltObject::from_int(7).bits());
                    dict_set_in_place(py, src_dict, all_key, names);
                    let depth = crate::builtins::exceptions::exception_stack_depth();
                    import_star(
                        py,
                        MoltObject::from_ptr(src).bits(),
                        MoltObject::from_ptr(dst).bits(),
                    );
                    assert_eq!(crate::builtins::exceptions::exception_stack_depth(), depth);
                    assert_eq!(
                        crate::builtins::exceptions::exception_context_active_bits(),
                        Some(outer_bits)
                    );
                    if mode == 0 {
                        assert!(!exception_pending(py));
                    } else {
                        assert_error(
                            py,
                            if mode == 1 {
                                "ValueError"
                            } else {
                                "StopIteration"
                            },
                        );
                    }
                    assert_eq!(
                        dict_get_in_place(py, dst_dict, key),
                        if mode == 2 {
                            None
                        } else {
                            Some(MoltObject::from_int(7).bits())
                        }
                    );
                    dec_ref_bits(py, key);
                    dec_ref_bits(py, module_name);
                }
            }
        });
    }

    #[test]
    fn star_preserves_prior_writes_when_all_contains_invalid_item() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let source_name = MoltObject::from_ptr(alloc_string(py, b"star_partial")).bits();
                let destination_name =
                    MoltObject::from_ptr(alloc_string(py, b"star_destination")).bits();
                let src = alloc_module_obj(py, source_name);
                let dst = alloc_module_obj(py, destination_name);
                let _src_owner = PtrDropGuard::new(src);
                let _dst_owner = PtrDropGuard::new(dst);
                let key = MoltObject::from_ptr(alloc_string(py, b"visible")).bits();
                let names = alloc_list(py, &[key, MoltObject::from_int(3).bits()]);
                let _names_owner = PtrDropGuard::new(names);
                let all_key =
                    intern_static_name(py, &runtime_state(py).interned.all_name, b"__all__");
                let src_dict = obj_from_bits(module_dict_bits(src)).as_ptr().unwrap();
                let dst_dict = obj_from_bits(module_dict_bits(dst)).as_ptr().unwrap();
                dict_set_in_place(py, src_dict, key, MoltObject::from_int(7).bits());
                dict_set_in_place(py, src_dict, all_key, MoltObject::from_ptr(names).bits());
                import_star(
                    py,
                    MoltObject::from_ptr(src).bits(),
                    MoltObject::from_ptr(dst).bits(),
                );
                assert_error(py, "TypeError");
                assert_eq!(
                    dict_get_in_place(py, dst_dict, key),
                    Some(MoltObject::from_int(7).bits())
                );
                for bits in [key, source_name, destination_name] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn indexed_read_rejects_mapping_only_and_normalizes_builtin_negative_index() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let zero = MoltObject::from_int(0).bits();
            let value = MoltObject::from_int(17).bits();
            let mapping = alloc_dict_with_pairs(py, &[zero, value]);
            let _mapping_owner = PtrDropGuard::new(mapping);
            let _ = crate::object::sequence_index::sequence_item_at_index(
                py,
                MoltObject::from_ptr(mapping).bits(),
                0,
            );
            assert_error(py, "TypeError");
            let mapping_bits = MoltObject::from_ptr(mapping).bits();
            let _ = crate::c_api::molt_sequence_getitem(mapping_bits, zero);
            assert_error(py, "TypeError");
            assert_eq!(crate::c_api::PySequence_GetItem(mapping_bits, 0), 0);
            assert_error(py, "TypeError");
            let list = alloc_list(py, &[zero, value]);
            let _list_owner = PtrDropGuard::new(list);
            let read = crate::object::sequence_index::sequence_item_at_index(
                py,
                MoltObject::from_ptr(list).bits(),
                -1,
            );
            assert!(!exception_pending(py));
            assert_eq!(read, value);
            dec_ref_bits(py, read);
            let list_bits = MoltObject::from_ptr(list).bits();
            let api_read =
                crate::c_api::molt_sequence_getitem(list_bits, MoltObject::from_int(-1).bits());
            assert!(!exception_pending(py));
            assert_eq!(api_read, value);
            dec_ref_bits(py, api_read);
            let compat_read = crate::c_api::PySequence_GetItem(list_bits, -1);
            assert!(!exception_pending(py));
            assert_eq!(compat_read, value);
            dec_ref_bits(py, compat_read);
            let _ = crate::object::sequence_index::sequence_item_at_index(
                py,
                MoltObject::from_ptr(list).bits(),
                -3,
            );
            assert_error(py, "IndexError");
        });
    }
}
