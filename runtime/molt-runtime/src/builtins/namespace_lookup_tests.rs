use super::*;

const LOOKUP_HIT: u64 = 0;
const LOOKUP_KEY_ERROR: u64 = 1;
const LOOKUP_VALUE_ERROR: u64 = 2;

static LOOKUP_MODE: AtomicU64 = AtomicU64::new(LOOKUP_HIT);
static LOOKUP_RESULT: AtomicU64 = AtomicU64::new(0);
static LOOKUP_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn namespace_lookup_callback(_self: u64, _key: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        LOOKUP_CALLS.fetch_add(1, Ordering::SeqCst);
        match LOOKUP_MODE.load(Ordering::SeqCst) {
            LOOKUP_KEY_ERROR => raise_exception::<u64>(_py, "KeyError", "callback miss"),
            LOOKUP_VALUE_ERROR => {
                raise_exception::<u64>(_py, "ValueError", "callback lookup failed")
            }
            _ => {
                let result = LOOKUP_RESULT.load(Ordering::SeqCst);
                assert_ne!(result, 0, "lookup callback result was not installed");
                inc_ref_bits(_py, result);
                result
            }
        }
    })
}

fn refcount(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
    unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

fn callback_dict_subclass(_py: &PyToken<'_>) -> (u64, u64, u64) {
    let function = crate::builtins::functions::alloc_runtime_function_obj(
        _py,
        crate::builtins::functions::runtime_fn_addr(
            "namespace_lookup_callback",
            namespace_lookup_callback as *const (),
        ),
        2,
    );
    assert!(!function.is_null());
    let function = MoltObject::from_ptr(function).bits();
    let name = attr_name_bits_from_bytes(_py, b"LookupDict").unwrap();
    let slot = attr_name_bits_from_bytes(_py, b"__getitem__").unwrap();
    let namespace = alloc_dict_with_pairs(_py, &[slot, function]);
    let bases = alloc_tuple(_py, &[builtin_classes(_py).dict]);
    assert!(!namespace.is_null() && !bases.is_null());
    let namespace = MoltObject::from_ptr(namespace).bits();
    let bases = MoltObject::from_ptr(bases).bits();
    let class = crate::builtins::types::molt_type_new(
        builtin_classes(_py).type_obj,
        name,
        bases,
        namespace,
        MoltObject::none().bits(),
    );
    assert!(!exception_pending(_py));
    let class_ptr = obj_from_bits(class).as_ptr().expect("dict subclass");
    let instance = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
    let instance_ptr = obj_from_bits(instance)
        .as_ptr()
        .expect("dict subclass instance");
    assert_eq!(
        unsafe { object_type_id(instance_ptr) },
        crate::TYPE_ID_OBJECT
    );
    assert_eq!(unsafe { crate::object_class_bits(instance_ptr) }, class);
    assert!(!unsafe { crate::object_is_exact_builtin_dict(_py, instance_ptr) });
    let storage = unsafe { crate::object::ops::dict_like_bits_from_ptr(_py, instance_ptr) }
        .expect("dict subclass storage");
    assert_ne!(storage, instance);
    assert_eq!(
        unsafe { object_type_id(obj_from_bits(storage).as_ptr().unwrap()) },
        TYPE_ID_DICT
    );
    for bits in [namespace, bases, slot, name] {
        dec_ref_bits(_py, bits);
    }
    (instance, class, function)
}

fn assert_and_clear_exception(_py: &PyToken<'_>, expected: &str, message: &str) {
    assert!(exception_pending(_py));
    let exception = molt_exception_last_pending();
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        _py, exception, expected
    ));
    let rendered = obj_from_bits(exception)
        .as_ptr()
        .map(|ptr| format_exception_with_traceback(_py, ptr))
        .unwrap_or_default();
    assert!(
        rendered.contains(message),
        "unexpected exception: {rendered}"
    );
    clear_exception(_py);
    dec_ref_bits(_py, exception);
}

#[test]
fn relative_import_reads_subclass_globals_backing_without_mapping_callbacks() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let (globals, class, function) = callback_dict_subclass(_py);
        let package_key = attr_name_bits_from_bytes(_py, b"__package__").unwrap();
        let empty = attr_name_bits_from_bytes(_py, b"").unwrap();
        let storage = crate::builtins::frames::globals_namespace_storage_ptr(_py, globals)
            .expect("dict subclass backing");
        unsafe { dict_set_in_place(_py, storage, package_key, empty) };
        LOOKUP_MODE.store(LOOKUP_VALUE_ERROR, Ordering::SeqCst);
        LOOKUP_CALLS.store(0, Ordering::SeqCst);
        let result = crate::builtins::platform::molt_importlib_import_transaction(
            empty,
            globals,
            MoltObject::none().bits(),
            MoltObject::none().bits(),
            MoltObject::from_int(1).bits(),
        );
        assert!(obj_from_bits(result).is_none());
        assert_eq!(LOOKUP_CALLS.load(Ordering::SeqCst), 0);
        assert_and_clear_exception(_py, "ImportError", "no known parent package");
        for bits in [globals, class, function, package_key, empty] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn exact_dict_lookup_owns_hits_and_reports_clean_misses() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"bound").unwrap();
        let absent = attr_name_bits_from_bytes(_py, b"absent").unwrap();
        let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let namespace = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[key, value])).bits();
        let value_before = refcount(value);
        let namespace_before = refcount(namespace);

        let found = lookup_namespace_item(_py, namespace, key)
            .expect("exact dictionary lookup")
            .expect("exact dictionary hit");
        assert_eq!(found, value);
        assert_eq!(refcount(value), value_before + 1);
        assert_eq!(refcount(namespace), namespace_before);
        dec_ref_bits(_py, found);
        assert_eq!(refcount(value), value_before);

        assert_eq!(lookup_namespace_item(_py, namespace, absent), Ok(None));
        assert!(!exception_pending(_py));
        assert_eq!(refcount(namespace), namespace_before);
        for bits in [namespace, value, absent, key] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn namespace_get_dispatches_dict_subclass_getitem_and_preserves_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"bound").unwrap();
        let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let (namespace, class, function) = callback_dict_subclass(_py);
        let value_before = refcount(value);
        let namespace_before = refcount(namespace);
        LOOKUP_MODE.store(LOOKUP_HIT, Ordering::SeqCst);
        LOOKUP_RESULT.store(value, Ordering::SeqCst);
        LOOKUP_CALLS.store(0, Ordering::SeqCst);

        let found = molt_namespace_get(namespace, key, missing_bits(_py));
        assert_eq!(found, value);
        assert_eq!(LOOKUP_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(refcount(value), value_before + 1);
        assert_eq!(refcount(namespace), namespace_before);
        assert!(!exception_pending(_py));
        dec_ref_bits(_py, found);
        assert_eq!(refcount(value), value_before);

        LOOKUP_RESULT.store(0, Ordering::SeqCst);
        for bits in [namespace, class, function, value, key] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn framed_global_store_uses_dict_subclass_storage_without_replacing_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"stored").unwrap();
        let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let (globals, class, function) = callback_dict_subclass(_py);
        let globals_ptr = obj_from_bits(globals).as_ptr().unwrap();

        assert_eq!(crate::molt_dict_set(globals, key, value), globals);
        assert!(!exception_pending(_py));
        assert_eq!(
            unsafe { object_type_id(globals_ptr) },
            crate::TYPE_ID_OBJECT
        );
        assert_eq!(unsafe { crate::object_class_bits(globals_ptr) }, class);
        let storage_ptr = crate::builtins::frames::globals_namespace_storage_ptr(_py, globals)
            .expect("dict subclass globals storage");
        assert_eq!(
            unsafe { dict_get_in_place(_py, storage_ptr, key) },
            Some(value)
        );

        for bits in [globals, class, function, value, key] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn foreign_module_set_does_not_redirect_into_active_function_globals() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"foreign_value").unwrap();
        let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let module_name = attr_name_bits_from_bytes(_py, b"foreign_namespace_target").unwrap();
        let module = molt_module_new(module_name);
        let (globals, class, function) = callback_dict_subclass(_py);
        assert!(!obj_from_bits(module).is_none());

        inc_ref_bits(_py, globals);
        crate::builtins::frames::frame_stack_push_owned(_py, 0, globals, 0);
        let result = molt_module_set_attr(module, key, value);
        crate::builtins::frames::frame_stack_pop(_py);
        assert!(obj_from_bits(result).is_none());
        assert!(!exception_pending(_py));

        let globals_storage = crate::builtins::frames::globals_namespace_storage_ptr(_py, globals)
            .expect("dict subclass globals storage");
        assert_eq!(
            unsafe { dict_get_in_place(_py, globals_storage, key) },
            None
        );
        let found = molt_module_get_attr(module, key);
        assert_eq!(found, value);
        dec_ref_bits(_py, found);

        for bits in [globals, class, function, module, module_name, value, key] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn global_dict_subclass_key_error_falls_back_to_captured_builtins() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"bound").unwrap();
        let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let (globals, class, function) = callback_dict_subclass(_py);
        let builtins = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[key, value])).bits();
        let value_before = refcount(value);
        let globals_before = refcount(globals);
        let builtins_before = refcount(builtins);
        LOOKUP_MODE.store(LOOKUP_KEY_ERROR, Ordering::SeqCst);
        LOOKUP_RESULT.store(0, Ordering::SeqCst);
        LOOKUP_CALLS.store(0, Ordering::SeqCst);

        let found = lookup_global_namespace(_py, 0, "<test>", globals, builtins, key, "bound");
        assert_eq!(found, value);
        assert_eq!(LOOKUP_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(refcount(value), value_before + 1);
        assert_eq!(refcount(globals), globals_before);
        assert_eq!(refcount(builtins), builtins_before);
        assert!(!exception_pending(_py));
        dec_ref_bits(_py, found);

        for bits in [globals, class, function, builtins, value, key] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn global_dict_subclass_value_error_propagates_without_builtins_fallback() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"bound").unwrap();
        let fallback = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let (globals, class, function) = callback_dict_subclass(_py);
        let builtins = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[key, fallback])).bits();
        let fallback_before = refcount(fallback);
        let globals_before = refcount(globals);
        LOOKUP_MODE.store(LOOKUP_VALUE_ERROR, Ordering::SeqCst);
        LOOKUP_RESULT.store(0, Ordering::SeqCst);
        LOOKUP_CALLS.store(0, Ordering::SeqCst);

        let result = lookup_global_namespace(_py, 0, "<test>", globals, builtins, key, "bound");
        assert!(obj_from_bits(result).is_none());
        assert_eq!(LOOKUP_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(refcount(fallback), fallback_before);
        assert_eq!(refcount(globals), globals_before);
        assert_and_clear_exception(_py, "ValueError", "callback lookup failed");

        for bits in [globals, class, function, builtins, fallback, key] {
            dec_ref_bits(_py, bits);
        }
    });
}

#[test]
fn builtins_subclass_hit_miss_and_error_keep_bootstrap_fallback_ordered() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"len").unwrap();
        let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let globals = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[])).bits();
        let (builtins, class, function) = callback_dict_subclass(_py);
        let value_before = refcount(value);
        let builtins_before = refcount(builtins);
        LOOKUP_MODE.store(LOOKUP_HIT, Ordering::SeqCst);
        LOOKUP_RESULT.store(value, Ordering::SeqCst);
        LOOKUP_CALLS.store(0, Ordering::SeqCst);

        let found = lookup_global_namespace(_py, 0, "<test>", globals, builtins, key, "len");
        assert_eq!(found, value);
        assert_eq!(LOOKUP_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(refcount(value), value_before + 1);
        assert_eq!(refcount(builtins), builtins_before);
        dec_ref_bits(_py, found);

        LOOKUP_MODE.store(LOOKUP_KEY_ERROR, Ordering::SeqCst);
        let missing = lookup_global_namespace(_py, 0, "<test>", globals, builtins, key, "len");
        assert!(obj_from_bits(missing).is_none());
        assert_eq!(LOOKUP_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(refcount(value), value_before);
        assert_and_clear_exception(_py, "NameError", "name 'len' is not defined");

        LOOKUP_MODE.store(LOOKUP_VALUE_ERROR, Ordering::SeqCst);
        assert_eq!(
            lookup_builtin_global(_py, key, "len", globals, builtins),
            Err(())
        );
        assert_eq!(LOOKUP_CALLS.load(Ordering::SeqCst), 3);
        assert_eq!(refcount(value), value_before);
        assert_and_clear_exception(_py, "ValueError", "callback lookup failed");

        let bootstrap = lookup_builtin_global(_py, key, "len", globals, 0)
            .expect("bootstrap lookup")
            .expect("runtime builtin");
        assert_eq!(LOOKUP_CALLS.load(Ordering::SeqCst), 3);
        assert!(crate::builtins::callable::is_callable_impl(_py, bootstrap));
        dec_ref_bits(_py, bootstrap);

        LOOKUP_RESULT.store(0, Ordering::SeqCst);
        for bits in [globals, builtins, class, function, value, key] {
            dec_ref_bits(_py, bits);
        }
    });
}
