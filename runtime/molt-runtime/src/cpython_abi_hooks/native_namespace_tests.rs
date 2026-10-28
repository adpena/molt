//! Canonical native declarations across Python and C namespace consumers.
use super::*;
use molt_cpython_abi::api::{errors, mapping, refcount, strings, typeobj};

static IMPORT_PROBE_DICTIONARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static IMPORT_PROBE_KEY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static IMPORT_PROBE_REPLACE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
extern "C" fn import_probe_deletes_publication(_spec: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let dictionary = crate::ptr_from_bits(
                IMPORT_PROBE_DICTIONARY.load(std::sync::atomic::Ordering::SeqCst),
            );
            let key = IMPORT_PROBE_KEY.load(std::sync::atomic::Ordering::SeqCst);
            if IMPORT_PROBE_REPLACE.load(std::sync::atomic::Ordering::SeqCst) {
                assert!(crate::dict_set_in_place(
                    py,
                    dictionary,
                    key,
                    MoltObject::from_int(42).bits()
                ));
            } else {
                crate::dict_del_in_place(py, dictionary, key).unwrap();
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

#[test]
fn get_module_revalidates_callback_mutation_for_latest_313_through_315() {
    use crate::builtins::exceptions::ExceptionValue;
    use std::sync::atomic::Ordering;
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        let class_name = ExceptionValue::adopt(
            &py,
            crate::attr_name_bits_from_bytes(&py, b"DeletingImportProbe").unwrap(),
        );
        let class = ExceptionValue::adopt(&py, crate::molt_class_new(class_name.bits()));
        let base = crate::molt_class_set_base(
            class.bits(),
            crate::builtins::classes::builtin_classes(&py).object,
        );
        crate::call::discard_owned_call_result(&py, base);
        assert!(!crate::exception_pending(&py));
        let function = crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::builtins::functions::runtime_fn_addr(
                "import_probe_deletes_publication",
                import_probe_deletes_publication as *const (),
            ),
            1,
        );
        assert!(!function.is_null());
        let function = ExceptionValue::adopt(&py, MoltObject::from_ptr(function).bits());
        let property = crate::object::builders::alloc_property_obj(
            &py,
            function.bits(),
            MoltObject::none().bits(),
            MoltObject::none().bits(),
        );
        assert!(!property.is_null());
        let property = ExceptionValue::adopt(&py, MoltObject::from_ptr(property).bits());
        let flag = ExceptionValue::adopt(
            &py,
            crate::attr_name_bits_from_bytes(&py, b"_initializing").unwrap(),
        );
        crate::builtins::attributes::mutate_attr_name(
            &py,
            class.bits(),
            flag.bits(),
            crate::builtins::attr::DescriptorMutation::Set(property.bits()),
            molt_cpython_abi::hooks::AttributeMutation::Normal,
        )
        .unwrap();
        crate::object::class_finish_definition(&py, crate::ptr_from_bits(class.bits())).unwrap();
        let spec = ExceptionValue::adopt(&py, crate::call_callable0(&py, class.bits()));
        let name = ExceptionValue::adopt(
            &py,
            crate::attr_name_bits_from_bytes(&py, b"_molt_capi_probe_version").unwrap(),
        );
        let module = ExceptionValue::adopt(
            &py,
            MoltObject::from_ptr(crate::alloc_module_obj(&py, name.bits())).bits(),
        );
        let spec_key = ExceptionValue::adopt(
            &py,
            crate::attr_name_bits_from_bytes(&py, b"__spec__").unwrap(),
        );
        crate::builtins::attributes::mutate_attr_name(
            &py,
            module.bits(),
            spec_key.bits(),
            crate::builtins::attr::DescriptorMutation::Set(spec.bits()),
            molt_cpython_abi::hooks::AttributeMutation::Normal,
        )
        .unwrap();
        let dictionary = crate::builtins::module_table::module_dictionary(&py).unwrap();
        IMPORT_PROBE_DICTIONARY.store(dictionary, Ordering::SeqCst);
        IMPORT_PROBE_KEY.store(name.bits(), Ordering::SeqCst);
        let capi_name =
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name.bits());
        for minor in 12..=15 {
            for replace in [false, true] {
                IMPORT_PROBE_REPLACE.store(replace, Ordering::SeqCst);
                transaction.with_target_python_minor(&py, minor, || {
                    assert!(crate::dict_set_in_place(
                        &py,
                        crate::ptr_from_bits(dictionary),
                        name.bits(),
                        module.bits()
                    ));
                    let result = molt_cpython_abi::api::imports::PyImport_GetModule(capi_name);
                    if minor >= 13 {
                        assert!(
                            result.is_null(),
                            "latest 3.13+ returns cache miss after false-probe mutation"
                        );
                    } else {
                        assert!(!result.is_null());
                        assert_eq!(
                            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                                .observed_handle_for_pyobj(result)
                                .unwrap()
                                .bits(),
                            module.bits()
                        );
                        refcount::Py_DECREF(result);
                    }
                    assert!(
                        errors::PyErr_Occurred().is_null(),
                        "cache miss never starts loading or synthesizes an error"
                    );
                    assert!(!crate::exception_pending(&py));
                });
            }
        }
        crate::dict_del_in_place(&py, crate::ptr_from_bits(dictionary), name.bits()).unwrap();
        IMPORT_PROBE_REPLACE.store(false, Ordering::SeqCst);
        IMPORT_PROBE_DICTIONARY.store(0, Ordering::SeqCst);
        IMPORT_PROBE_KEY.store(0, Ordering::SeqCst);
        refcount::Py_DECREF(capi_name);
    });
}

#[test]
fn import_dictionary_is_available_before_sys_and_survives_attribute_rebinding() {
    use crate::builtins::exceptions::ExceptionValue;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        assert!(crate::builtins::modules::interpreter_sys_module(&py).is_none());
        let first = molt_cpython_abi::api::imports::PyImport_GetModuleDict();
        assert!(!first.is_null());
        let canonical = crate::builtins::module_table::module_dictionary(&py).unwrap();
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(first)
                .unwrap()
                .bits(),
            canonical
        );
        let imported =
            molt_cpython_abi::api::imports::PyImport_AddModule(c"_molt_presys_capi".as_ptr());
        assert!(!imported.is_null());
        let name = crate::attr_name_bits_from_bytes(&py, b"_molt_presys_capi").unwrap();
        // Replacing the public dictionary entry retires its module owner.
        // Keep this independent owner through every replacement and reinsert.
        let module =
            ExceptionValue::adopt(&py, crate::builtins::modules::molt_module_cache_get(name));
        let bits = module.bits();
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(imported)
                .unwrap()
                .bits(),
            bits
        );
        let capi_name = molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name);
        for value in [
            bits,
            MoltObject::none().bits(),
            MoltObject::from_int(42).bits(),
            0,
        ] {
            assert!(crate::dict_set_in_place(
                &py,
                crate::ptr_from_bits(canonical),
                name,
                value
            ));
            let result = molt_cpython_abi::api::imports::PyImport_GetModule(capi_name);
            assert!(!result.is_null(), "zero bits is a valid owned float result");
            let observed = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(result)
                .expect("public semantic carrier observation");
            assert_eq!(observed.bits(), value);
            refcount::Py_DECREF(result);
        }
        assert!(crate::dict_set_in_place(
            &py,
            crate::ptr_from_bits(canonical),
            name,
            bits
        ));

        let sys_name = crate::attr_name_bits_from_bytes(&py, b"sys").unwrap();
        let sys = MoltObject::from_ptr(crate::alloc_module_obj(&py, sys_name)).bits();
        crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
        let modules_name = crate::attr_name_bits_from_bytes(&py, b"modules").unwrap();
        let replacement = MoltObject::from_ptr(crate::alloc_dict_with_pairs(&py, &[])).bits();
        let namespace = crate::ptr_from_bits(crate::module_dict_bits(crate::ptr_from_bits(sys)));
        assert!(crate::dict_set_in_place(
            &py,
            namespace,
            modules_name,
            replacement
        ));
        let public = molt_cpython_abi::api::sys::PySys_GetObject(c"modules".as_ptr());
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(public)
                .unwrap()
                .bits(),
            replacement
        );
        assert_eq!(
            molt_cpython_abi::api::imports::PyImport_GetModuleDict(),
            first
        );
        let cached =
            ExceptionValue::adopt(&py, crate::builtins::modules::molt_module_cache_get(name));
        assert_eq!(cached.bits(), bits);
        drop(cached);
        assert!(crate::dict_del_in_place(&py, namespace, modules_name).unwrap());
        assert_eq!(
            molt_cpython_abi::api::imports::PyImport_GetModuleDict(),
            first
        );
        let found = molt_cpython_abi::api::imports::PyImport_GetModule(capi_name);
        assert!(!found.is_null());
        refcount::Py_DECREF(found);
        crate::builtins::modules::molt_module_cache_del(name);
        let missing = molt_cpython_abi::api::imports::PyImport_GetModule(capi_name);
        assert!(missing.is_null());
        assert!(errors::PyErr_Occurred().is_null());
        refcount::Py_DECREF(capi_name);
        for value in [name, sys_name, modules_name, replacement, sys] {
            crate::dec_ref_bits(&py, value);
        }
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn eval_builtins_interpreter_role_survives_module_unpublication_and_replacement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let name = crate::attr_name_bits_from_bytes(&py, b"builtins").unwrap();
        let module = MoltObject::from_ptr(crate::alloc_module_obj(&py, name)).bits();
        let dictionary = crate::module_dict_bits(crate::ptr_from_bits(module));
        crate::builtins::module_table::publish_interpreter_builtins_for_test(&py, module);
        crate::inc_ref_bits(&py, module);
        let previous = crate::builtins::module_table::replace_module_owned_for_test(
            &py,
            "builtins",
            Some(module),
        );
        let first = molt_cpython_abi::api::eval::PyEval_GetBuiltins();
        assert!(!first.is_null());
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(first)
                .unwrap()
                .bits(),
            dictionary
        );
        crate::builtins::modules::molt_module_cache_del(name);
        crate::dec_ref_bits(&py, module);
        // The module can be destroyed; its exact dictionary remains owned by
        // the interpreter role, not by a borrowed lookup's temporary pin.
        for replacement in [MoltObject::none().bits(), MoltObject::from_int(42).bits()] {
            let displaced = crate::builtins::module_table::replace_module_owned_for_test(
                &py,
                "builtins",
                Some(replacement),
            );
            if let Some(bits) = displaced {
                crate::dec_ref_bits(&py, bits);
            }
            assert_eq!(molt_cpython_abi::api::eval::PyEval_GetBuiltins(), first);
            assert_eq!(
                crate::builtins::module_table::interpreter_builtins_dictionary(&py),
                Some(dictionary)
            );
            assert_eq!(
                crate::builtins::frames::frame_effective_builtins_bits(
                    &py,
                    MoltObject::none().bits(),
                ),
                Ok(dictionary)
            );
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
        }
        let displaced =
            crate::builtins::module_table::replace_module_owned_for_test(&py, "builtins", previous);
        if let Some(bits) = displaced {
            crate::dec_ref_bits(&py, bits);
        }
        crate::dec_ref_bits(&py, name);
    });
}

#[test]
fn eval_builtins_borrows_the_exact_captured_object_without_mapping_validation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let dictionary = MoltObject::from_ptr(crate::alloc_dict_with_pairs(&py, &[])).bits();
        let mapping_candidate = MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits();
        for bits in [
            dictionary,
            mapping_candidate,
            MoltObject::none().bits(),
            MoltObject::from_int(42).bits(),
        ] {
            crate::inc_ref_bits(&py, bits);
            crate::builtins::frames::frame_stack_push_owned(&py, 0, 0, bits, 0);
            let view = molt_cpython_abi::api::eval::PyEval_GetBuiltins();
            assert!(!view.is_null());
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(view)
                    .unwrap()
                    .bits(),
                bits
            );
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
            crate::builtins::frames::frame_stack_pop(&py);
        }
        crate::dec_ref_bits(&py, mapping_candidate);
        crate::dec_ref_bits(&py, dictionary);
    });
}

#[test]
fn native_type_namespace_projects_the_owner_dictionary_and_preserves_edits() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let classes = crate::builtin_classes(&py);
        let class = crate::obj_from_bits(classes.list).as_ptr().unwrap();
        let dictionary = crate::class_dict_bits(class);
        let dict = crate::obj_from_bits(dictionary).as_ptr().unwrap();
        let append_name = crate::attr_name_bits_from_bytes(&py, b"append").unwrap();
        let reverse_name = crate::attr_name_bits_from_bytes(&py, b"reverse").unwrap();
        let previous_reverse = crate::dict_get_in_place(&py, dict, reverse_name).unwrap();
        let append = crate::builtins::attr::class_namespace_lookup_raw(&py, class, append_name)
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::dict_get_in_place(&py, dict, reverse_name).unwrap(),
            previous_reverse,
            "one ordinary lookup must not enumerate the owner's other declarations"
        );
        let c_name = strings::PyUnicode_FromString(c"append".as_ptr());
        let type_view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .handle_to_borrowed_pyobj(classes.list)
            .cast::<PyTypeObject>();
        let raw = typeobj::_PyType_Lookup(type_view, c_name);
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(raw)
                .unwrap()
                .bits(),
            append
        );
        assert_eq!(typeobj::PyType_Ready(type_view), 0);
        let namespace = (*type_view).tp_dict;
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .observed_handle_for_pyobj(namespace)
                .unwrap()
                .bits(),
            dictionary
        );
        assert_eq!(mapping::PyDict_GetItem(namespace, c_name), raw);
        // Keep the original descriptor while the admitted C dictionary drops
        // its owner; repeat readiness and enumeration must not recreate it.
        crate::inc_ref_bits(&py, append);
        assert_eq!(mapping::PyDict_DelItem(namespace, c_name), 0);
        assert_eq!(typeobj::PyType_Ready(type_view), 0);
        assert!(typeobj::_PyType_Lookup(type_view, c_name).is_null());
        assert!(errors::PyErr_Occurred().is_null());
        assert!(crate::builtins::methods::publish_builtin_class_methods(
            &py,
            classes.list
        ));
        assert!(
            crate::builtins::attr::class_namespace_lookup_raw(&py, class, append_name)
                .unwrap()
                .is_none()
        );
        assert!(crate::dict_set_in_place(&py, dict, append_name, append));
        crate::dec_ref_bits(&py, append);
        refcount::Py_DECREF(c_name);
        crate::dec_ref_bits(&py, append_name);
        crate::dec_ref_bits(&py, reverse_name);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn managed_type_projection_owns_one_mirrored_dictionary_edge() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        let name = crate::attr_name_bits_from_bytes(&py, b"NativeNamespaceOwner").unwrap();
        let class = crate::molt_class_new(name);
        crate::dec_ref_bits(&py, name);
        crate::molt_class_set_base(class, crate::builtin_classes(&py).object);
        crate::object::class_finish_definition(&py, crate::obj_from_bits(class).as_ptr().unwrap())
            .expect("seal the namespace class through its canonical layout owner");
        assert!(!crate::exception_pending(&py));
        let class_ptr = crate::obj_from_bits(class).as_ptr().unwrap();
        let namespace = crate::class_dict_bits(class_ptr);
        let dictionary_view =
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(namespace);
        let before = (*dictionary_view).ob_refcnt;
        let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .handle_to_borrowed_pyobj(class)
            .cast::<PyTypeObject>();
        assert!(!view.is_null());
        assert_eq!((*view).tp_dict, dictionary_view);
        assert_eq!((*dictionary_view).ob_refcnt, before + 1);
        assert!(!molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_direct_c_refs(namespace));
        assert_eq!(typeobj::PyType_Ready(view), 0);
        assert_eq!((*dictionary_view).ob_refcnt, before + 1);
        assert!(molt_cpython_abi::bridge::GLOBAL_BRIDGE.retire_runtime_type_views(&[class]));
        assert_eq!((*dictionary_view).ob_refcnt, before);
        assert!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .managed_handle_for_pyobj(view.cast())
                .is_none()
        );
        crate::dec_ref_bits(&py, class);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn gc_retires_type_mro_projection_cycle_before_releasing_candidate_pins() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        let name = crate::attr_name_bits_from_bytes(&py, b"CollectedProjectionOwner").unwrap();
        let class = crate::molt_class_new(name);
        crate::dec_ref_bits(&py, name);
        let result = crate::molt_class_set_base(class, crate::builtin_classes(&py).object);
        crate::dec_ref_bits(&py, result);
        crate::object::class_finish_definition(&py, crate::obj_from_bits(class).as_ptr().unwrap())
            .expect("seal class before publishing its recursive C projection");
        let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let view = bridge
            .handle_to_borrowed_pyobj(class)
            .cast::<PyTypeObject>();
        assert!(!view.is_null());
        let mro = (*view).tp_mro;
        assert_eq!(
            molt_cpython_abi::api::sequences::PyTuple_GetItem(mro, 0),
            view.cast()
        );
        assert_eq!(bridge.mirrored_c_refcount(view.addr()), 1);
        assert!(!bridge.has_direct_c_refs(class));
        // A live C alias must preserve the entire semantic/projection cycle.
        let alias = molt_cpython_abi::api::sequences::PyTuple_New(1);
        assert!(!alias.is_null());
        molt_cpython_abi::api::refcount::Py_INCREF(mro);
        assert_eq!(
            molt_cpython_abi::api::sequences::PyTuple_SetItem(alias, 0, mro),
            0
        );
        crate::dec_ref_bits(&py, class);
        let reachable = crate::object::gc::collect_cycles(&py);
        assert_eq!(
            reachable.status,
            crate::object::gc::GcCollectStatus::Completed
        );
        assert_eq!(bridge.managed_handle_for_pyobj(view.cast()), Some(class));
        assert!(bridge.managed_handle_for_pyobj(mro).is_some());
        molt_cpython_abi::api::refcount::Py_DECREF(alias);
        let result = crate::object::gc::collect_cycles(&py);
        assert_eq!(result.status, crate::object::gc::GcCollectStatus::Completed);
        for pointer in [view.cast(), mro] {
            assert!(bridge.managed_handle_for_pyobj(pointer).is_none());
            assert_eq!(bridge.mirrored_c_refcount(pointer.addr()), 0);
        }
        assert!(!crate::exception_pending(&py));
    });
}

mod readiness;
