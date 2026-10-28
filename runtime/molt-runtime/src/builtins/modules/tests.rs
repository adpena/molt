use super::*;
use std::sync::atomic::Ordering;

#[test]
fn module_trace_modes_share_one_grammar_and_verbose_traces_every_access() {
    assert_eq!(trace_mode(None), TraceMode::Off);
    assert_eq!(trace_mode(Some("0")), TraceMode::Off);
    assert_eq!(trace_mode(Some("1")), TraceMode::Filtered);
    assert_eq!(trace_mode(Some("all")), TraceMode::Verbose);
    assert_eq!(trace_mode(Some("verbose")), TraceMode::Verbose);
    // Verbose includes the filtered accesses; module stores were silent
    // under it while the setter required the `1` spelling as well.
    for (module, attr) in [("app", "x"), ("app", "_sys"), ("importlib._bootstrap", "x")] {
        assert!(module_attr_trace_selected(TraceMode::Verbose, module, attr));
        assert!(!module_attr_trace_selected(TraceMode::Off, module, attr));
    }
    assert!(!module_attr_trace_selected(TraceMode::Filtered, "app", "x"));
    assert!(module_attr_trace_selected(
        TraceMode::Filtered,
        "app",
        "_sys"
    ));
    assert!(module_attr_trace_selected(
        TraceMode::Filtered,
        "importlib._bootstrap",
        "x"
    ));
}

#[test]
fn copyreg_reads_foreign_integer_storage_without_losing_range_policy() {
    use molt_cpython_abi::abi_types::{
        PyExc_AssertionError, PyExc_ValueError, PyLong_Type, PyLongObject,
    };
    use molt_cpython_abi::api::{errors, object, refcount::OwnedPyObject, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for value in [5000u32, 0] {
                let native = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                    &raw mut PyLong_Type,
                    1,
                ));
                assert!(!native.as_ptr().is_null());
                assert!(
                    GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(native.as_ptr())
                        .is_none()
                );
                let integer = native.as_ptr().cast::<PyLongObject>();
                (*integer).long_value.lv_tag = if value == 0 { 1 } else { 1 << 3 };
                (*integer).long_value.ob_digit[0] = value;
                let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap();
                let _owner = crate::PtrDropGuard::preserving(ptr_from_bits(bits));
                assert_eq!(
                    crate::object::object_type_id(ptr_from_bits(bits)),
                    crate::TYPE_ID_FOREIGN
                );
                let refs = (*native.as_ptr()).ob_refcnt;
                let converted = copyreg_add_extension_code_int(py, bits);
                if value == 0 {
                    let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(converted.is_err());
                    assert!(!error.as_ptr().is_null());
                    assert_eq!(object::Py_TYPE(error.as_ptr()), &raw mut PyExc_ValueError);
                } else {
                    let converted = converted.unwrap();
                    let converted_owner = obj_from_bits(converted)
                        .as_ptr()
                        .map(crate::PtrDropGuard::preserving);
                    assert!(!exception_pending(py));
                    // Public int() retains an exact native integer; range admission
                    // does not require a managed scalar projection.
                    assert_eq!(converted, bits);
                    drop(converted_owner);
                    let result = molt_copyreg_reduce_ex(MoltObject::none().bits(), bits);
                    let _result_owner = obj_from_bits(result)
                        .as_ptr()
                        .map(crate::PtrDropGuard::preserving);
                    let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(!error.as_ptr().is_null());
                    // Correct extraction reaches the documented proto >= 2 guard.
                    assert_eq!(
                        object::Py_TYPE(error.as_ptr()),
                        &raw mut PyExc_AssertionError
                    );
                }
                assert!(!exception_pending(py));
                assert_eq!((*native.as_ptr()).ob_refcnt, refs);
            }
        }
    });
}

#[test]
fn namespace_del_normalizes_non_key_mapping_failures() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = MoltObject::from_ptr(alloc_string(_py, b"absent")).bits();
        // A TypeError from the receiver is normalized, not just KeyError.
        let _ = molt_namespace_del(MoltObject::none().bits(), key);
        assert!(exception_pending(_py));
        let error = molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            error,
            "NameError"
        ));
        clear_exception(_py);
        dec_ref_bits(_py, error);
        dec_ref_bits(_py, key);
    });
}

#[test]
fn namespace_get_preserves_values_absence_and_non_key_errors() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = MoltObject::from_ptr(alloc_string(_py, b"bound")).bits();
        let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let ns = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[key, value])).bits();
        let missing = crate::missing_bits(_py);
        let loaded = molt_namespace_get(ns, key, missing);
        assert_eq!(loaded, value);
        assert!(!exception_pending(_py));
        dec_ref_bits(_py, loaded);

        let absent = MoltObject::from_ptr(alloc_string(_py, b"absent")).bits();
        assert_eq!(molt_namespace_get(ns, absent, missing), missing);
        assert!(!exception_pending(_py));

        // Only a KeyError from the namespace means fall through. TypeError
        // from a non-mapping (or a bad hash) must remain pending.
        let _ = molt_namespace_get(MoltObject::none().bits(), key, missing);
        assert!(exception_pending(_py));
        let error = molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            error,
            "TypeError"
        ));
        let _ = molt_namespace_get(ns, key, missing);
        let retained = molt_exception_last_pending();
        assert_eq!(retained, error, "a pending exception must not be consumed");
        clear_exception(_py);
        dec_ref_bits(_py, retained);
        dec_ref_bits(_py, error);
        dec_ref_bits(_py, absent);
        dec_ref_bits(_py, ns);
        dec_ref_bits(_py, value);
        dec_ref_bits(_py, key);
    });
}

struct ModuleCacheRestore {
    name_bits: u64,
    previous_bits: Option<u64>,
}

impl ModuleCacheRestore {
    fn new(_py: &PyToken<'_>, name_bits: u64) -> Self {
        let name = string_obj_to_owned(obj_from_bits(name_bits)).unwrap();
        let previous_bits = crate::builtins::module_table::module_lookup(_py, &name)
            .expect("snapshot public module membership");
        let _ = crate::molt_exception_clear();
        let _ = molt_module_cache_del(name_bits);
        let _ = crate::molt_exception_clear();
        Self {
            name_bits,
            previous_bits,
        }
    }

    fn name_bits(&self) -> u64 {
        self.name_bits
    }
}

impl Drop for ModuleCacheRestore {
    fn drop(&mut self) {
        crate::with_gil_entry_nopanic!(_py, {
            let _ = crate::molt_exception_clear();
            let _ = molt_module_cache_del(self.name_bits);
            let _ = crate::molt_exception_clear();
            let name = string_obj_to_owned(obj_from_bits(self.name_bits)).unwrap();
            // Public membership includes None and arbitrary objects. The
            // compiler namespace publisher deliberately rejects those.
            let displaced = crate::builtins::module_table::replace_module_owned_for_test(
                _py,
                &name,
                self.previous_bits.take(),
            );
            if let Some(bits) = displaced {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, self.name_bits);
        });
    }
}

#[test]
fn module_cache_restore_preserves_exact_public_membership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let name = attr_name_bits_from_bytes(py, b"_molt_cache_restore_membership").unwrap();
        let _restore = ModuleCacheRestore::new(py, name);
        let dictionary = crate::builtins::module_table::module_dictionary(py).unwrap();
        let pointer = ptr_from_bits(dictionary);
        let string = MoltObject::from_ptr(alloc_string(py, b"retained public value")).bits();
        for previous in [
            None,
            Some(MoltObject::none().bits()),
            Some(MoltObject::from_int(42).bits()),
            Some(string),
        ] {
            if let Some(bits) = previous {
                assert!(unsafe { dict_set_in_place(py, pointer, name, bits) });
            } else {
                unsafe { dict_del_in_place(py, pointer, name).unwrap() };
            }
            inc_ref_bits(py, name);
            {
                let _scoped = ModuleCacheRestore::new(py, name);
                assert_eq!(
                    unsafe { dict_get_in_place(py, pointer, name).unwrap() },
                    None
                );
                assert!(unsafe {
                    dict_set_in_place(py, pointer, name, MoltObject::from_int(99).bits())
                });
            }
            assert_eq!(
                unsafe { dict_get_in_place(py, pointer, name).unwrap() },
                previous,
                "restore must distinguish absent, None and arbitrary public values"
            );
            assert!(!exception_pending(py));
        }
        dec_ref_bits(py, string);
    });
}

fn assert_pending_exception_class(_py: &PyToken<'_>, expected: &str) {
    assert!(exception_pending(_py));
    let exc_bits = molt_exception_last_pending();
    assert!(!obj_from_bits(exc_bits).is_none());
    assert!(
        crate::builtins::exceptions::exception_matches_builtin_name(_py, exc_bits, expected),
        "expected pending exception to be {expected}"
    );
    dec_ref_bits(_py, exc_bits);
    let _ = crate::molt_exception_clear();
    assert!(!exception_pending(_py));
}

fn assert_pending_module_not_found_metadata(
    py: &PyToken<'_>,
    expected_name: &str,
    expected_name_identity: Option<u64>,
    expected_message: &str,
) {
    assert!(exception_pending(py));
    let exception = ExceptionValue::adopt(py, molt_exception_last_pending());
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        py,
        exception.bits(),
        "ModuleNotFoundError"
    ));
    clear_exception(py);
    let read = |field: &[u8]| {
        let key = attr_name_bits_from_bytes(py, field).unwrap();
        let value = crate::builtins::attributes::get_attr_name(py, exception.bits(), key)
            .expect("public exception attribute");
        dec_ref_bits(py, key);
        ExceptionValue::adopt(py, value)
    };
    let name = read(b"name");
    assert_eq!(
        string_obj_to_owned(obj_from_bits(name.bits())).as_deref(),
        Some(expected_name)
    );
    if let Some(expected) = expected_name_identity {
        assert_eq!(
            name.bits(),
            expected,
            "preserve the supplied Python name object"
        );
    }
    for field in [b"path".as_slice(), b"name_from"] {
        assert!(obj_from_bits(read(field).bits()).is_none());
    }
    let message = read(b"msg");
    assert_eq!(
        string_obj_to_owned(obj_from_bits(message.bits())).as_deref(),
        Some(expected_message)
    );
    let args = read(b"args");
    assert_eq!(
        unsafe { object_type_id(ptr_from_bits(args.bits())) },
        crate::TYPE_ID_TUPLE
    );
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr_from_bits(args.bits()), |args| {
            assert_eq!(args, &[message.bits()], "one original message argument");
        })
    }
    .expect("exception args tuple");
    drop(exception);
    // The public name result owns its reference independently of the now
    // retired raised state and exception, rather than borrowing a field.
    assert_eq!(
        string_obj_to_owned(obj_from_bits(name.bits())).as_deref(),
        Some(expected_name)
    );
    assert!(!exception_pending(py));
}

#[test]
fn modules_runtime_state_is_owned_and_clearable() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        let dispatch_bits = copyreg_dict_slot_bits(_py, &state.modules.copyreg_dispatch_table_bits);
        assert_ne!(dispatch_bits, 0);
        assert_ne!(
            state
                .modules
                .copyreg_dispatch_table_bits
                .load(Ordering::Acquire),
            0
        );
        let constructors_bits =
            copyreg_set_slot_bits(_py, &state.modules.copyreg_constructor_registry_bits);
        assert_ne!(constructors_bits, 0);
        assert_ne!(
            state
                .modules
                .copyreg_constructor_registry_bits
                .load(Ordering::Acquire),
            0
        );
        let import_name =
            intern_static_name(_py, &state.modules.runpy_import_dunder_name, b"__import__");
        assert_ne!(import_name, 0);
        assert_ne!(
            state
                .modules
                .runpy_import_dunder_name
                .load(Ordering::Acquire),
            0
        );

        modules_clear_runtime_state(_py, state);

        for slot in state.modules.object_slots() {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    });
}

#[test]
fn import_from_publishes_owned_metadata_and_versioned_origin() {
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let string = |value: &[u8]| {
            let ptr = alloc_string(py, value);
            assert!(!ptr.is_null());
            ExceptionValue::adopt(py, MoltObject::from_ptr(ptr).bits())
        };
        let name = string(b"owned_import_receiver");
        let module_ptr = alloc_module_obj(py, name.bits());
        assert!(!module_ptr.is_null());
        let module = ExceptionValue::adopt(py, MoltObject::from_ptr(module_ptr).bits());
        let spec_ptr = alloc_module_obj(py, name.bits());
        assert!(!spec_ptr.is_null());
        let spec = ExceptionValue::adopt(py, MoltObject::from_ptr(spec_ptr).bits());
        let file = string(b"/import-receiver/file.py");
        let origin = string(b"/import-receiver/spec.py");
        let set = |receiver: *mut u8, key: &[u8], value: u64| unsafe {
            let key = string(key);
            let dict = ptr_from_bits(module_dict_bits(receiver));
            assert!(dict_set_in_place(py, dict, key.bits(), value));
            assert!(!exception_pending(py));
        };
        set(module_ptr, b"__file__", file.bits());
        set(module_ptr, b"__spec__", spec.bits());
        set(spec_ptr, b"origin", origin.bits());
        set(
            spec_ptr,
            b"has_location",
            MoltObject::from_bool(true).bits(),
        );
        set(
            spec_ptr,
            b"_initializing",
            MoltObject::from_bool(false).bits(),
        );
        for minor in [12, 13, 14] {
            transaction.with_target_python_minor(py, minor, || {
                for (member, expected_message) in [
                    (
                        b"first".as_slice(),
                        "cannot import name 'first' from 'owned_import_receiver'",
                    ),
                    (
                        b"second".as_slice(),
                        "cannot import name 'second' from 'owned_import_receiver'",
                    ),
                ] {
                    let attr = string(member);
                    let result = molt_module_import_from(module.bits(), attr.bits());
                    assert!(obj_from_bits(result).is_none());
                    assert!(exception_pending(py));
                    let exception = ExceptionValue::adopt(py, molt_exception_last_pending());
                    clear_exception(py);
                    let expected_path = if minor == 12 {
                        file.bits()
                    } else {
                        origin.bits()
                    };
                    for (field, expected) in [
                        (b"name".as_slice(), name.bits()),
                        (b"path".as_slice(), expected_path),
                        (b"name_from".as_slice(), attr.bits()),
                    ] {
                        let field = string(field);
                        let actual = ExceptionValue::adopt(
                            py,
                            molt_getattr_builtin(exception.bits(), field.bits(), missing_bits(py)),
                        );
                        assert_eq!(
                            actual.bits(),
                            expected,
                            "typed import metadata keeps identity"
                        );
                        assert!(!exception_pending(py));
                    }
                    let expected = format!(
                        "{expected_message} ({})",
                        if minor == 12 {
                            "/import-receiver/file.py"
                        } else {
                            "/import-receiver/spec.py"
                        }
                    );
                    assert_eq!(
                        format_obj_str(py, obj_from_bits(exception.bits())),
                        expected
                    );
                    assert!(!exception_pending(py));
                }
            });
        }
        // Existing attribute success bypasses all failure metadata work.
        set(module_ptr, b"first", MoltObject::from_int(73).bits());
        let first = string(b"first");
        assert_eq!(
            molt_module_import_from(module.bits(), first.bits()),
            MoltObject::from_int(73).bits()
        );
        assert!(!exception_pending(py));
    });
}

#[test]
fn import_error_allocation_denial_never_publishes_partial_fields() {
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    struct RestoreBudget;
    impl Drop for RestoreBudget {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        let none = MoltObject::none().bits();
        for kind in ["ImportError", "ModuleNotFoundError"] {
            let raise = |message: &[u8]| {
                if kind == "ImportError" {
                    raise_import_error(py, message, none, none, none)
                } else {
                    crate::builtins::exceptions::raise_module_not_found(py, message, none)
                }
            };
            // Warm each concrete class, then deny the real allocator.
            let _ = raise(b"warm");
            clear_exception(py);
            let budget = RestoreBudget;
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            assert_eq!(raise(b"denied"), none);
            crate::test_support::assert_and_clear_emergency_memory_error(py);
            drop(budget);
            let _ = raise(b"recovered");
            let recovered = ExceptionValue::adopt(py, molt_exception_last_pending());
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                recovered.bits(),
                kind
            ));
            clear_exception(py);
        }
    });
}

#[test]
fn raw_module_allocation_seeds_public_name_metadata() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let name_ptr = alloc_string(_py, b"synthetic_runtime_module");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let module_ptr = alloc_module_obj(_py, name_bits);
            assert!(!module_ptr.is_null());
            dec_ref_bits(_py, name_bits);
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let name_key_ptr = alloc_string(_py, b"__name__");
            assert!(!name_key_ptr.is_null());
            let name_key_bits = MoltObject::from_ptr(name_key_ptr).bits();
            let found_name_bits = module_attr_lookup(_py, module_ptr, name_key_bits)
                .expect("module name lookup succeeds")
                .expect("module __name__ exists");
            let found_name = string_obj_to_owned(obj_from_bits(found_name_bits));
            assert_eq!(found_name.as_deref(), Some("synthetic_runtime_module"));
            dec_ref_bits(_py, found_name_bits);
            dec_ref_bits(_py, name_key_bits);

            let missing_key_ptr = alloc_string(_py, b"tolist");
            assert!(!missing_key_ptr.is_null());
            let missing_key_bits = MoltObject::from_ptr(missing_key_ptr).bits();
            let has_bits = crate::molt_has_attr_name(module_bits, missing_key_bits);
            assert_eq!(has_bits, MoltObject::from_bool(false).bits());
            assert!(
                !exception_pending(_py),
                "hasattr(module, missing) must clear AttributeError"
            );
            dec_ref_bits(_py, missing_key_bits);
            dec_ref_bits(_py, module_bits);
        }
    });
}

#[test]
fn generated_native_providers_own_self_and_public_aliases_own_lookup() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let builtins = crate::test_support::NativeProviderTestNamespace::new(py, "builtins");
            let io = crate::test_support::NativeProviderTestNamespace::new(py, "_io");
            let builtin_dict = obj_from_bits(module_dict_bits(
                obj_from_bits(builtins.bits()).as_ptr().unwrap(),
            ))
            .as_ptr()
            .unwrap();
            let io_dict =
                obj_from_bits(module_dict_bits(obj_from_bits(io.bits()).as_ptr().unwrap()))
                    .as_ptr()
                    .unwrap();
            let open_name = attr_name_bits_from_bytes(py, b"open").unwrap();
            let self_name = attr_name_bits_from_bytes(py, b"__self__").unwrap();
            assert!(
                dict_get_in_place(py, builtin_dict, open_name)
                    .expect("dictionary lookup succeeds")
                    .is_none()
            );
            let open = dict_get_in_place(py, io_dict, open_name)
                .expect("dictionary lookup succeeds")
                .unwrap();
            let owner = crate::molt_get_attr_name(open, self_name);
            assert_eq!(owner, io.bits());
            dec_ref_bits(py, owner);
            // Model the runtime initializer's exact alias publication;
            // this unit fixture deliberately has no generated module table.
            assert!(dict_set_in_place(py, builtin_dict, open_name, open));
            let alias = crate::builtins::functions::lookup_builtin_name(py, "open")
                .expect("operation succeeds")
                .unwrap();
            assert_eq!(alias, open);
            dec_ref_bits(py, alias);
            assert!(dict_set_in_place(
                py,
                builtin_dict,
                open_name,
                MoltObject::from_int(37).bits()
            ));
            assert_eq!(
                crate::builtins::functions::lookup_builtin_name(py, "open")
                    .expect("operation succeeds"),
                Some(MoltObject::from_int(37).bits())
            );
            crate::dict_del_in_place(py, builtin_dict, open_name).expect("operation succeeds");
            assert_eq!(
                crate::builtins::functions::lookup_builtin_name(py, "open")
                    .expect("operation succeeds"),
                None
            );
            assert_eq!(
                dict_get_in_place(py, io_dict, open_name).expect("dictionary lookup succeeds"),
                Some(open)
            );
            // A compiled named materializer cannot use its raw target to
            // refill a deleted known public builtin.
            let absent = crate::molt_func_new_builtin_named(
                open_name,
                fn_addr!(crate::molt_open_builtin),
                0,
                8,
            );
            assert!(exception_pending(py));
            assert_pending_exception_class(py, "NameError");
            dec_ref_bits(py, absent);
            for bits in [open_name, self_name] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn module_get_global_uses_captured_builtins_for_hits_misses_and_suggestions() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let string = |value: &[u8]| MoltObject::from_ptr(alloc_string(py, value)).bits();
        let builtins_name = string(b"builtins");
        let cache_restore = ModuleCacheRestore::new(py, builtins_name);
        let cached_module = alloc_module_obj(py, cache_restore.name_bits());
        let cached_bits = MoltObject::from_ptr(cached_module).bits();
        molt_module_cache_set(cache_restore.name_bits(), cached_bits);
        crate::builtins::module_table::publish_interpreter_builtins_for_test(py, cached_bits);
        let cached_dict = unsafe { module_dict_bits(cached_module) };
        let captured_ptr = alloc_dict_with_pairs(py, &[]);
        let captured = MoltObject::from_ptr(captured_ptr).bits();
        let globals_ptr = alloc_dict_with_pairs(py, &[]);
        let globals = MoltObject::from_ptr(globals_ptr).bits();
        let answer = string(b"answer");
        let len = string(b"len");
        let custom = string(b"custom_token");
        let runtime_only = string(b"runtime_only");
        let builtins_key = string(b"__builtins__");
        let module_name = string(b"unrelated_module");
        let module_ptr = alloc_module_obj(py, module_name);
        let module = MoltObject::from_ptr(module_ptr).bits();
        unsafe {
            assert!(dict_set_in_place(
                py,
                globals_ptr,
                answer,
                MoltObject::from_int(42).bits()
            ));
            assert!(dict_set_in_place(
                py,
                captured_ptr,
                len,
                MoltObject::from_int(11).bits()
            ));
            assert!(dict_set_in_place(
                py,
                captured_ptr,
                custom,
                MoltObject::from_int(1).bits()
            ));
            let cached_ptr = obj_from_bits(cached_dict).as_ptr().unwrap();
            assert!(dict_set_in_place(
                py,
                cached_ptr,
                len,
                MoltObject::from_int(33).bits()
            ));
            assert!(dict_set_in_place(
                py,
                cached_ptr,
                runtime_only,
                MoltObject::from_int(1).bits()
            ));
            let unrelated = obj_from_bits(module_dict_bits(module_ptr))
                .as_ptr()
                .unwrap();
            assert!(dict_set_in_place(
                py,
                unrelated,
                answer,
                MoltObject::from_int(99).bits()
            ));
            // A replaced __builtins__ entry must not reinterpret this activation.
            assert!(dict_set_in_place(
                py,
                globals_ptr,
                builtins_key,
                cached_dict
            ));
        }
        inc_ref_bits(py, globals);
        inc_ref_bits(py, captured);
        crate::builtins::frames::frame_stack_push_owned(py, 0, globals, captured, 0);
        assert_eq!(
            molt_module_get_global(module, answer),
            MoltObject::from_int(42).bits()
        );
        assert_eq!(
            molt_module_get_global(module, len),
            MoltObject::from_int(11).bits()
        );
        // Explicit module attribute access is not Python LOAD_GLOBAL.
        assert_eq!(
            molt_module_get_attr(module, answer),
            MoltObject::from_int(99).bits()
        );
        assert!(unsafe { dict_del_in_place(py, captured_ptr, len).expect("operation succeeds") });
        assert!(obj_from_bits(molt_module_get_global(module, len)).is_none());
        assert_pending_exception_class(py, "NameError");
        assert!(obj_from_bits(molt_module_get_global(module, runtime_only)).is_none());
        assert_pending_exception_class(py, "NameError");

        for (typo, expected) in [
            (b"custom_toke".as_slice(), Some("custom_token")),
            (b"runtime_onl".as_slice(), None),
        ] {
            let typo = string(typo);
            let missing = molt_module_get_global(module, typo);
            assert!(obj_from_bits(missing).is_none());
            let exception = molt_exception_last_pending();
            let ptr = obj_from_bits(exception).as_ptr().unwrap();
            let message = format_exception_with_traceback(py, ptr);
            match expected {
                Some(candidate) => {
                    assert!(message.contains(&format!("Did you mean: '{candidate}'?")))
                }
                None => assert!(!message.contains("Did you mean:")),
            }
            dec_ref_bits(py, exception);
            assert_pending_exception_class(py, "NameError");
            dec_ref_bits(py, typo);
        }
        assert!(unsafe {
            dict_del_in_place(py, captured_ptr, custom).expect("operation succeeds")
        });
        assert!(obj_from_bits(molt_module_get_global(module, len)).is_none());
        assert_pending_exception_class(py, "NameError");
        crate::builtins::frames::frame_stack_pop(py);
        assert_eq!(
            molt_module_get_global(module, answer),
            MoltObject::from_int(99).bits()
        );
        assert_eq!(
            molt_module_get_global(module, len),
            MoltObject::from_int(33).bits()
        );
        for bits in [
            module,
            module_name,
            globals,
            captured,
            cached_bits,
            answer,
            len,
            custom,
            runtime_only,
            builtins_key,
        ] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn module_get_global_captured_unavailable_builtins_does_not_switch_to_cache() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let builtins_name = MoltObject::from_ptr(alloc_string(py, b"builtins")).bits();
        let cache_restore = ModuleCacheRestore::new(py, builtins_name);
        let globals = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
        inc_ref_bits(py, globals);
        crate::builtins::frames::frame_stack_push_owned(py, 0, globals, 0, 0);
        let module_ptr = alloc_module_obj(py, cache_restore.name_bits());
        let module = MoltObject::from_ptr(module_ptr).bits();
        molt_module_cache_set(cache_restore.name_bits(), module);
        assert!(!exception_pending(py));
        assert_eq!(
            crate::builtins::module_table::interpreter_builtins_dictionary(py),
            None,
            "public module publication alone does not select interpreter builtins"
        );
        crate::builtins::module_table::publish_interpreter_builtins_for_test(py, module);
        assert_eq!(
            crate::builtins::frames::frame_stack_active_builtins(),
            Some(0)
        );
        assert_eq!(
            crate::builtins::frames::frame_effective_builtins_bits(py, globals)
                .expect("builtins namespace lookup succeeds"),
            0
        );
        for spelling in ["len", "list", "ValueError", "molt_len"] {
            let name = MoltObject::from_ptr(alloc_string(py, spelling.as_bytes())).bits();
            unsafe {
                let dictionary = obj_from_bits(module_dict_bits(module_ptr))
                    .as_ptr()
                    .unwrap();
                assert!(dict_set_in_place(
                    py,
                    dictionary,
                    name,
                    MoltObject::from_int(73).bits()
                ));
            }
            let found = molt_module_get_global(MoltObject::none().bits(), name);
            assert!(obj_from_bits(found).is_none());
            assert!(exception_pending(py));
            let exception = molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                exception,
                "NameError"
            ));
            clear_exception(py);
            dec_ref_bits(py, exception);
            dec_ref_bits(py, name);
        }
        assert_eq!(
            crate::builtins::functions::lookup_builtin_name(py, "len").expect("operation succeeds"),
            None
        );
        assert!(!exception_pending(py));
        unsafe {
            assert!(molt_cpython_abi::api::eval::PyEval_GetBuiltins().is_null());
            assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::errors::PyErr_Clear();
        }
        clear_exception(py);
        crate::builtins::frames::frame_stack_pop(py);
        assert_eq!(crate::builtins::frames::frame_stack_active_builtins(), None);
        assert_eq!(
            crate::builtins::frames::frame_effective_builtins_bits(py, globals)
                .expect("builtins namespace lookup succeeds"),
            unsafe { module_dict_bits(module_ptr) }
        );
        assert_eq!(
            crate::builtins::functions::lookup_builtin_name(py, "len").expect("operation succeeds"),
            Some(MoltObject::from_int(73).bits())
        );
        unsafe {
            let view = molt_cpython_abi::api::eval::PyEval_GetBuiltins();
            assert!(!view.is_null());
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(view)
                    .unwrap()
                    .bits(),
                module_dict_bits(module_ptr)
            );
        }
        for bits in [globals, module] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn module_get_global_respects_present_builtins_dict_miss() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins_name_ptr = alloc_string(_py, b"builtins");
        assert!(!builtins_name_ptr.is_null());
        let builtins_name_bits = MoltObject::from_ptr(builtins_name_ptr).bits();
        let cache_restore = ModuleCacheRestore::new(_py, builtins_name_bits);

        let builtins_module_ptr = alloc_module_obj(_py, cache_restore.name_bits());
        assert!(!builtins_module_ptr.is_null());
        let builtins_module_bits = MoltObject::from_ptr(builtins_module_ptr).bits();
        let set_result = molt_module_cache_set(cache_restore.name_bits(), builtins_module_bits);
        assert!(obj_from_bits(set_result).is_none());
        assert!(!exception_pending(_py));
        crate::builtins::module_table::publish_interpreter_builtins_for_test(
            _py,
            builtins_module_bits,
        );

        let module_name_ptr = alloc_string(_py, b"empty_builtins_lookup_module");
        assert!(!module_name_ptr.is_null());
        let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
        let module_ptr = alloc_module_obj(_py, module_name_bits);
        dec_ref_bits(_py, module_name_bits);
        assert!(!module_ptr.is_null());
        let module_bits = MoltObject::from_ptr(module_ptr).bits();

        let builtins_dict_bits = unsafe { module_dict_bits(builtins_module_ptr) };
        let builtins_dict_ptr = obj_from_bits(builtins_dict_bits)
            .as_ptr()
            .expect("builtins module dictionary");
        assert_eq!(unsafe { object_type_id(builtins_dict_ptr) }, TYPE_ID_DICT);

        for (index, builtin_name) in ["len", "globals", "locals", "vars", "__import__"]
            .into_iter()
            .enumerate()
        {
            let name_ptr = alloc_string(_py, builtin_name.as_bytes());
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let published_bits = MoltObject::from_int(index as i64 + 1).bits();
            unsafe {
                assert!(dict_set_in_place(
                    _py,
                    builtins_dict_ptr,
                    name_bits,
                    published_bits
                ));
            }
            assert!(!exception_pending(_py));

            let loaded_bits = molt_module_get_global(module_bits, name_bits);
            assert_eq!(
                loaded_bits, published_bits,
                "published builtins.{builtin_name} must win over runtime synthesis"
            );
            assert!(!exception_pending(_py));
            dec_ref_bits(_py, loaded_bits);

            assert!(unsafe {
                crate::object::ops::dict_del_in_place(_py, builtins_dict_ptr, name_bits)
                    .expect("operation succeeds")
            });
            assert!(!exception_pending(_py));
            let missing_bits = molt_module_get_global(module_bits, name_bits);
            assert!(
                obj_from_bits(missing_bits).is_none(),
                "deleting builtins.{builtin_name} from a published dictionary must be authoritative"
            );
            assert_pending_exception_class(_py, "NameError");
            dec_ref_bits(_py, name_bits);
        }

        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, builtins_module_bits);
    });
}

#[test]
fn module_cache_publication_preserves_duplicate_and_aliased_owners() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let refcount = |bits| {
            let ptr = obj_from_bits(bits).as_ptr().expect("module object");
            unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
        };
        // Exercise both bootstrap publication without sys.modules and the
        // ordinary publication after sys bootstrap.
        for publish_sys in [false, true] {
            let sys_name = MoltObject::from_ptr(alloc_string(py, b"sys")).bits();
            let sys_restore = ModuleCacheRestore::new(py, sys_name);
            let sys_bits = if publish_sys {
                let bits = molt_module_new(sys_restore.name_bits());
                assert!(!obj_from_bits(bits).is_none());
                assert!(
                    obj_from_bits(
                        crate::builtins::module_table::publish_interpreter_sys_for_test(py, bits)
                    )
                    .is_none()
                );
                bits
            } else {
                MoltObject::none().bits()
            };
            let name =
                MoltObject::from_ptr(alloc_string(py, b"_molt_publication_ownership")).bits();
            let cache_restore = ModuleCacheRestore::new(py, name);
            // This proves cache ownership, including a sole-owner
            // borrowed replacement. molt_module_new can additionally
            // anchor the first module as the runtime intrinsic registry.
            let first_ptr = alloc_module_obj(py, name);
            let duplicate_ptr = alloc_module_obj(py, name);
            assert!(!first_ptr.is_null());
            assert!(!duplicate_ptr.is_null());
            let first = MoltObject::from_ptr(first_ptr).bits();
            let duplicate = MoltObject::from_ptr(duplicate_ptr).bits();
            assert_ne!(first, duplicate);
            assert_eq!(refcount(first), 1, "fixture has exactly one caller owner");
            assert_eq!(
                refcount(duplicate),
                1,
                "fixture has exactly one caller owner"
            );
            assert!(obj_from_bits(molt_module_cache_set(name, first)).is_none());
            assert!(!exception_pending(py));
            let first_owners = refcount(first);
            let duplicate_owners = refcount(duplicate);

            for _ in 0..3 {
                let result = molt_module_cache_set(name, duplicate);
                assert!(
                    obj_from_bits(result).is_none(),
                    "duplicate publication must not return a borrowed cache owner"
                );
                // Model an unbound owned-result sink, as used by WASM.
                dec_ref_bits(py, result);
                assert!(!exception_pending(py));
                assert_eq!(refcount(first), first_owners - 1);
                assert_eq!(refcount(duplicate), duplicate_owners + 1);
                let cached = molt_module_cache_get(name);
                assert_eq!(
                    cached, duplicate,
                    "publication installs the exact supplied namespace"
                );
                dec_ref_bits(py, cached);
                if publish_sys {
                    let modules =
                        sys_modules_dict_bits(py, sys_bits).expect("published sys.modules");
                    let dict = obj_from_bits(modules).as_ptr().unwrap();
                    let _modules_owner = crate::PtrDropGuard::new(dict);
                    assert_eq!(
                        unsafe {
                            dict_get_in_place(py, dict, name).expect("dictionary lookup succeeds")
                        },
                        Some(duplicate)
                    );
                }
            }

            dec_ref_bits(py, duplicate);
            dec_ref_bits(py, first);
            let cache_owners = refcount(duplicate);
            if !publish_sys {
                assert_eq!(cache_owners, 1, "the cache is now the sole owner");
            }
            // The argument is borrowed from the live cache entry. A
            // release-before-retain replacement would destroy it here.
            for _ in 0..3 {
                let result = molt_module_cache_set(name, duplicate);
                assert!(obj_from_bits(result).is_none());
                dec_ref_bits(py, result);
                assert!(!exception_pending(py));
                assert_eq!(refcount(duplicate), cache_owners);
            }
            drop(cache_restore);
            dec_ref_bits(py, sys_bits);
            drop(sys_restore);
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn sys_module_cache_set_does_not_leave_pending_exception() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let name_ptr = alloc_string(_py, b"sys");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let cache_restore = ModuleCacheRestore::new(_py, name_bits);
        let module_ptr = alloc_module_obj(_py, cache_restore.name_bits());
        assert!(!module_ptr.is_null());
        let module_bits = MoltObject::from_ptr(module_ptr).bits();

        let result_bits = molt_module_cache_set(cache_restore.name_bits(), module_bits);
        // Publication is a mutator, never a borrowed module handoff.
        assert!(obj_from_bits(result_bits).is_none());
        assert!(
            !exception_pending(_py),
            "sys module registration must not leave a pending exception"
        );

        dec_ref_bits(_py, result_bits);
        dec_ref_bits(_py, module_bits);
    });
}

#[test]
fn import_outcome_uses_single_dictionary_before_and_after_sys_bootstrap() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let sys_name = attr_name_bits_from_bytes(py, b"sys").unwrap();
        let _sys_restore = ModuleCacheRestore::new(py, sys_name);
        let text = "_molt_public_cache_outcome_probe";
        let name = attr_name_bits_from_bytes(py, text.as_bytes()).unwrap();
        let _name_restore = ModuleCacheRestore::new(py, name);
        let bootstrap = molt_module_new(name);
        molt_module_cache_set(name, bootstrap);
        let before_sys = crate::builtins::module_table::module_dictionary(py).unwrap();
        let cached = molt_module_cache_get(name);
        assert_eq!(
            cached, bootstrap,
            "cache is authoritative before sys publication"
        );
        dec_ref_bits(py, cached);
        let sys = molt_module_new(sys_name);
        crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys);
        let _resolver = crate::test_support::EmptyImportResolver::new(py);
        let modules_bits = sys_modules_dict_bits(py, sys).unwrap();
        assert_eq!(
            modules_bits, before_sys,
            "sys exposes the existing dictionary"
        );
        let modules = obj_from_bits(modules_bits).as_ptr().unwrap();
        let _modules_owner = crate::PtrDropGuard::new(modules);
        let module = molt_module_new(name);
        let dictionary = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
        for value in [MoltObject::from_int(42).bits(), module, dictionary] {
            unsafe { assert!(dict_set_in_place(py, modules, name, value)) };
            let cached = molt_module_cache_get(name);
            assert_eq!(cached, value);
            dec_ref_bits(py, cached);
            let imported = match module_import_attempt(name).unwrap() {
                ModuleImportOutcome::Imported(bits) => bits,
                ModuleImportOutcome::Missing { .. } => panic!("visible cache hit lost"),
            };
            assert_eq!(imported, value);
            dec_ref_bits(py, imported);
            let stored = crate::builtins::module_table::module_lookup(py, text).unwrap();
            assert_eq!(stored, Some(value));
            dec_ref_bits(py, stored.unwrap());
            assert!(unsafe { dict_del_in_place(py, modules, name).expect("operation succeeds") });
            assert!(obj_from_bits(molt_module_cache_get(name)).is_none());
            assert!(matches!(module_import_attempt(name),
                Ok(ModuleImportOutcome::Missing { diagnostic_name }) if diagnostic_name == text));
            assert!(obj_from_bits(molt_module_import(name)).is_none());
            assert_pending_module_not_found_metadata(
                py,
                text,
                None,
                &format!("No module named '{text}'"),
            );
        }
        unsafe {
            assert!(dict_set_in_place(
                py,
                modules,
                name,
                MoltObject::none().bits()
            ))
        };
        let refs_before =
            unsafe { (*crate::header_from_obj_ptr(ptr_from_bits(name))).ref_count_snapshot() };
        assert!(obj_from_bits(molt_module_import(name)).is_none());
        assert_pending_module_not_found_metadata(
            py,
            text,
            Some(name),
            &format!("import of {text} halted; None in sys.modules"),
        );
        assert_eq!(
            unsafe { (*crate::header_from_obj_ptr(ptr_from_bits(name))).ref_count_snapshot() },
            refs_before,
            "error metadata releases its name edge"
        );
        for bits in [module, dictionary, bootstrap, sys] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn catalog_lookup_uses_actual_image_membership_and_public_cache_precedence() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        // This name can designate a user backport. A name-only stdlib
        // availability table cannot turn an unregistered image miss into
        // a different provider/dependency failure before execution.
        let name = attr_name_bits_from_bytes(py, b"msilib.schema").unwrap();
        let _restore = ModuleCacheRestore::new(py, name);
        assert!(matches!(catalog_import_attempt(name),
            Ok(ModuleImportOutcome::Missing { diagnostic_name }) if diagnostic_name == "msilib.schema"));
        assert!(!exception_pending(py));
        let dictionary = crate::builtins::module_table::module_dictionary(py).unwrap();
        // Arbitrary public cache values are dictionary mutations, not
        // compiler namespace publications through molt_module_cache_set.
        assert!(unsafe {
            dict_set_in_place(
                py,
                ptr_from_bits(dictionary),
                name,
                MoltObject::from_int(42).bits(),
            )
        });
        assert!(!exception_pending(py));
        let imported = match catalog_import_attempt(name).unwrap() {
            ModuleImportOutcome::Imported(bits) => bits,
            ModuleImportOutcome::Missing { .. } => panic!("public cache hit lost"),
        };
        assert_eq!(imported, MoltObject::from_int(42).bits());
        dec_ref_bits(py, imported);
        assert!(!exception_pending(py));
    });
}

#[test]
fn import_attempt_preserves_pending_failure_even_when_its_text_names_the_target() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let name = attr_name_bits_from_bytes(_py, b"pkg.child").unwrap();
        crate::builtins::exceptions::exception_stack_push();
        let raised =
            raise_exception::<u64>(_py, "ModuleNotFoundError", "No module named 'pkg.child'");
        dec_ref_bits(_py, raised);
        let original = molt_exception_last_pending();
        assert!(module_import_attempt(name).is_err());
        let observed = molt_exception_last_pending();
        assert_eq!(observed, original);
        assert!(exception_pending(_py));
        dec_ref_bits(_py, observed);
        dec_ref_bits(_py, original);
        clear_exception(_py);
        crate::builtins::exceptions::exception_stack_pop(_py);
        dec_ref_bits(_py, name);
    });
}

#[test]
fn canonical_module_dictionary_preserves_none_and_arbitrary_public_values() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        crate::with_gil_entry_nopanic!(py, {
            assert!(
                interpreter_sys_module(py).is_none(),
                "exercise cold resolver setup"
            );
            let previous_sys = crate::builtins::module_table::module_lookup(py, "sys").unwrap();
            let resolver = crate::test_support::EmptyImportResolver::new(py);
            let text = "_molt_single_store_arbitrary_values";
            let name = attr_name_bits_from_bytes(py, text.as_bytes()).unwrap();
            let _restore = ModuleCacheRestore::new(py, name);
            let dictionary = crate::builtins::module_table::module_dictionary(py).unwrap();
            let pointer = ptr_from_bits(dictionary);
            assert!(unsafe { dict_set_in_place(py, pointer, name, MoltObject::none().bits()) });
            assert!(module_import_attempt(name).is_err());
            assert_pending_exception_class(py, "ModuleNotFoundError");
            let string = MoltObject::from_ptr(alloc_string(py, b"arbitrary public value")).bits();
            assert!(unsafe { dict_set_in_place(py, pointer, name, string) });
            assert!(
                matches!(module_import_attempt(name), Ok(ModuleImportOutcome::Imported(bits)) if { assert_eq!(bits, string); dec_ref_bits(py, bits); true })
            );
            assert!(unsafe { dict_del_in_place(py, pointer, name).unwrap() });
            assert!(matches!(
                module_import_attempt(name),
                Ok(ModuleImportOutcome::Missing { diagnostic_name }) if diagnostic_name == text
            ));
            assert!(!exception_pending(py));
            dec_ref_bits(py, string);
            drop(_restore);
            drop(resolver);
            assert!(
                interpreter_sys_module(py).is_none(),
                "fixture retires its sys role"
            );
            let restored_sys = crate::builtins::module_table::module_lookup(py, "sys").unwrap();
            assert_eq!(
                restored_sys, previous_sys,
                "fixture restores public sys membership"
            );
            for bits in [previous_sys, restored_sys].into_iter().flatten() {
                dec_ref_bits(py, bits);
            }
        });
    });
}

#[test]
fn prepare_from_import_child_preserves_existing_package_attribute() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let module_name_ptr = alloc_string(_py, b"pkg");
            assert!(!module_name_ptr.is_null());
            let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
            let module_ptr = alloc_module_obj(_py, module_name_bits);
            assert!(!module_ptr.is_null());
            dec_ref_bits(_py, module_name_bits);
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let existing_ptr = alloc_string(_py, b"class-export");
            assert!(!existing_ptr.is_null());
            let existing_bits = MoltObject::from_ptr(existing_ptr).bits();
            let module_dict = module_dict_bits(module_ptr);
            let module_dict_ptr = obj_from_bits(module_dict)
                .as_ptr()
                .expect("module dict pointer");
            assert_eq!(object_type_id(module_dict_ptr), TYPE_ID_DICT);
            dict_set_str_key_bits(_py, module_dict_ptr, "Tensor", existing_bits)
                .expect("set exported Tensor");

            let attr_ptr = alloc_string(_py, b"Tensor");
            assert!(!attr_ptr.is_null());
            let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
            let child_ptr = alloc_string(_py, b"pkg.Tensor");
            assert!(!child_ptr.is_null());
            let child_bits = MoltObject::from_ptr(child_ptr).bits();

            let result = prepare_from_import_child(_py, module_bits, attr_bits, child_bits);
            assert!(
                !exception_pending(_py),
                "existing package attr prepare path must not leave an exception"
            );
            assert!(result.is_ok());

            let found_bits = dict_get_in_place(_py, module_dict_ptr, attr_bits)
                .expect("dictionary lookup succeeds")
                .expect("Tensor attr");
            assert_eq!(found_bits, existing_bits);

            dec_ref_bits(_py, child_bits);
            dec_ref_bits(_py, attr_bits);
            dec_ref_bits(_py, existing_bits);
            dec_ref_bits(_py, module_bits);
        }
    });
}

#[test]
fn sys_module_cache_set_populates_bootstrap_metadata() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let name_ptr = alloc_string(_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let cache_restore = ModuleCacheRestore::new(_py, name_bits);
            let module_ptr = alloc_module_obj(_py, cache_restore.name_bits());
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let result_bits =
                crate::builtins::module_table::publish_interpreter_sys_for_test(_py, module_bits);
            assert!(
                !exception_pending(_py),
                "sys module registration must not leave a pending exception"
            );

            let dict_bits = module_dict_bits(module_ptr);
            let dict_ptr = obj_from_bits(dict_bits)
                .as_ptr()
                .expect("sys module dict pointer");
            assert_eq!(object_type_id(dict_ptr), TYPE_ID_DICT);

            for key in [
                "platform",
                "version",
                "version_info",
                "hexversion",
                "api_version",
                "implementation",
                "maxsize",
                "maxunicode",
                "byteorder",
                "prefix",
                "exec_prefix",
                "base_prefix",
                "base_exec_prefix",
                "platlibdir",
                "path",
                "orig_argv",
                "copyright",
                "stdlib_module_names",
                "builtin_module_names",
                "meta_path",
                "path_hooks",
                "path_importer_cache",
            ] {
                let key_ptr = alloc_string(_py, key.as_bytes());
                assert!(!key_ptr.is_null());
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let value_bits = dict_get_in_place(_py, dict_ptr, key_bits)
                    .expect("dictionary lookup succeeds")
                    .unwrap_or_else(|| panic!("missing sys.{key}"));
                assert!(
                    !obj_from_bits(value_bits).is_none(),
                    "sys.{key} must not be None"
                );
                dec_ref_bits(_py, key_bits);
            }

            let platform_key_ptr = alloc_string(_py, b"platform");
            assert!(!platform_key_ptr.is_null());
            let platform_key_bits = MoltObject::from_ptr(platform_key_ptr).bits();
            let platform_bits = dict_get_in_place(_py, dict_ptr, platform_key_bits)
                .expect("dictionary lookup succeeds")
                .expect("sys.platform");
            let platform_text = string_obj_to_owned(obj_from_bits(platform_bits));
            assert!(
                platform_text
                    .as_ref()
                    .is_some_and(|value| !value.is_empty()),
                "sys.platform must be a non-empty string"
            );
            let abiflags_key = crate::attr_name_bits_from_bytes(_py, b"abiflags").unwrap();
            let abiflags =
                dict_get_in_place(_py, dict_ptr, abiflags_key).expect("dictionary lookup succeeds");
            assert_eq!(
                abiflags.is_some(),
                !platform_text.as_ref().unwrap().starts_with("win"),
                "native bootstrap owns platform-specific abiflags presence"
            );
            if let Some(bits) = abiflags {
                assert!(string_obj_to_owned(obj_from_bits(bits)).is_some());
            }
            dec_ref_bits(_py, abiflags_key);
            dec_ref_bits(_py, platform_key_bits);

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, module_bits);
        }
    });
}
