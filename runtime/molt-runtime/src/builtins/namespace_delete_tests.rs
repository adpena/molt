use super::*;

#[test]
fn global_delete_uses_active_globals_without_deleting_the_lexical_binding() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"bound").unwrap();
        let value = MoltObject::from_int(1).bits();
        let lexical = alloc_dict_with_pairs(_py, &[key, value]);
        let active = alloc_dict_with_pairs(_py, &[key, value]);
        assert!(!lexical.is_null() && !active.is_null());
        let lexical_bits = MoltObject::from_ptr(lexical).bits();
        let active_bits = MoltObject::from_ptr(active).bits();
        inc_ref_bits(_py, active_bits);
        crate::builtins::frames::frame_stack_push_owned(_py, 0, active_bits, 0);
        assert!(obj_from_bits(molt_module_del_global(lexical_bits, key)).is_none());
        assert_eq!(unsafe { dict_get_in_place(_py, active, key) }, None);
        assert_eq!(unsafe { dict_get_in_place(_py, lexical, key) }, Some(value));
        assert!(obj_from_bits(molt_module_del_global_if_present(lexical_bits, key)).is_none());
        assert!(!exception_pending(_py));
        molt_module_del_global(lexical_bits, key);
        let error = molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            error,
            "NameError"
        ));
        clear_exception(_py);
        dec_ref_bits(_py, error);
        crate::builtins::frames::frame_stack_pop(_py);
        for bits in [lexical_bits, active_bits, key] {
            dec_ref_bits(_py, bits);
        }
    });
}

static DELETE_RESULT: AtomicU64 = AtomicU64::new(0);
static DELETE_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn namespace_delete_callback(_self: u64, _key: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        DELETE_CALLS.fetch_add(1, Ordering::SeqCst);
        let result = DELETE_RESULT.load(Ordering::SeqCst);
        if result == 0 {
            return raise_exception::<u64>(_py, "ValueError", "custom deletion failed");
        }
        inc_ref_bits(_py, result);
        result
    })
}

fn callback_namespace(_py: &PyToken<'_>) -> (u64, u64, u64) {
    let function = crate::builtins::functions::alloc_runtime_function_obj(
        _py,
        crate::builtins::functions::runtime_fn_addr(
            "namespace_delete_callback",
            namespace_delete_callback as *const (),
        ),
        2,
    );
    assert!(!function.is_null());
    let function = MoltObject::from_ptr(function).bits();
    let name = attr_name_bits_from_bytes(_py, b"DeleteNamespace").unwrap();
    let slot = attr_name_bits_from_bytes(_py, b"__delitem__").unwrap();
    let dict = alloc_dict_with_pairs(_py, &[slot, function]);
    assert!(!dict.is_null());
    let dict = MoltObject::from_ptr(dict).bits();
    let class = crate::builtins::types::molt_type_new(
        builtin_classes(_py).type_obj,
        name,
        MoltObject::none().bits(),
        dict,
        MoltObject::none().bits(),
    );
    assert!(!exception_pending(_py));
    let namespace =
        unsafe { crate::alloc_instance_for_class(_py, obj_from_bits(class).as_ptr().unwrap()) };
    assert!(!obj_from_bits(namespace).is_none());
    dec_ref_bits(_py, dict);
    dec_ref_bits(_py, slot);
    dec_ref_bits(_py, name);
    (namespace, class, function)
}

#[test]
fn namespace_delete_dictionary_success_returns_none_and_keeps_receiver_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"bound").unwrap();
        let dict = alloc_dict_with_pairs(_py, &[key, MoltObject::from_int(1).bits()]);
        assert!(!dict.is_null());
        let namespace = MoltObject::from_ptr(dict).bits();
        let before = unsafe { (*crate::header_from_obj_ptr(dict)).ref_count_snapshot() };
        let result = molt_namespace_del(namespace, key);
        assert!(obj_from_bits(result).is_none());
        dec_ref_bits(_py, result);
        assert_eq!(
            unsafe { (*crate::header_from_obj_ptr(dict)).ref_count_snapshot() },
            before
        );
        assert_eq!(unsafe { dict_get_in_place(_py, dict, key) }, None);
        assert!(!exception_pending(_py));
        let _ = molt_namespace_del(namespace, key);
        let error = molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            error,
            "NameError"
        ));
        clear_exception(_py);
        dec_ref_bits(_py, error);
        dec_ref_bits(_py, namespace);
        dec_ref_bits(_py, key);
    });
}

#[test]
fn namespace_delete_custom_mapping_discards_result_and_preserves_pending_entry() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let key = attr_name_bits_from_bytes(_py, b"bound").unwrap();
        let result = alloc_list(_py, &[]);
        assert!(!result.is_null());
        let result_bits = MoltObject::from_ptr(result).bits();
        let (namespace, class, function) = callback_namespace(_py);
        let before = unsafe { (*crate::header_from_obj_ptr(result)).ref_count_snapshot() };
        DELETE_RESULT.store(result_bits, Ordering::SeqCst);
        DELETE_CALLS.store(0, Ordering::SeqCst);
        assert!(obj_from_bits(molt_namespace_del(namespace, key)).is_none());
        assert_eq!(DELETE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(
            unsafe { (*crate::header_from_obj_ptr(result)).ref_count_snapshot() },
            before
        );
        assert!(!exception_pending(_py));

        DELETE_RESULT.store(0, Ordering::SeqCst);
        let _ = molt_namespace_del(namespace, key);
        assert_eq!(DELETE_CALLS.load(Ordering::SeqCst), 2);
        let error = molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            _py,
            error,
            "NameError"
        ));
        let _ = molt_namespace_del(namespace, key);
        assert_eq!(DELETE_CALLS.load(Ordering::SeqCst), 2);
        let retained = molt_exception_last_pending();
        assert_eq!(error, retained);
        clear_exception(_py);
        dec_ref_bits(_py, retained);
        dec_ref_bits(_py, error);
        dec_ref_bits(_py, namespace);
        dec_ref_bits(_py, class);
        dec_ref_bits(_py, function);
        dec_ref_bits(_py, result_bits);
        dec_ref_bits(_py, key);
    });
}
