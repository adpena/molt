use super::*;

#[test]
fn false_fromlist_selects_request_component_and_balances_alias_owners() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let mut keys = Vec::new();
        let mut modules = Vec::new();
        for name in ["return_pkg", "return_pkg.branch", "return_pkg.branch.leaf"] {
            let key = alloc_str_bits(py, name).unwrap();
            let module = crate::object::builders::alloc_module_obj(py, key);
            assert!(!module.is_null());
            keys.push(key);
            modules.push(MoltObject::from_ptr(module).bits());
        }
        let cache = alloc_dict_with_pairs(
            py,
            &[
                keys[0], modules[0], keys[1], modules[1], keys[2], modules[2],
            ],
        );
        assert!(!cache.is_null());
        for (request, level, expected) in [
            ("return_pkg.branch.leaf", 0, 0),
            ("branch.leaf", 1, 1),
            ("leaf", 1, 2),
            ("", 1, 2),
        ] {
            let ptr = obj_from_bits(modules[expected]).as_ptr().unwrap();
            let before = unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() };
            inc_ref_bits(py, modules[2]);
            let result = importlib_transaction_no_fromlist_value(
                py,
                request,
                level,
                "return_pkg.branch.leaf",
                cache,
                modules[2],
            )
            .unwrap();
            assert!(!exception_pending(py));
            assert_eq!(result, modules[expected]);
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() },
                before + 1,
            );
            dec_ref_bits(py, result);
        }

        // Distinct keys may own the same module. Selection must transfer
        // exactly one result reference, rather than retaining both owners.
        unsafe {
            assert!(dict_set_in_place(py, cache, keys[1], modules[2]));
        }
        let ptr = obj_from_bits(modules[2]).as_ptr().unwrap();
        let before = unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() };
        inc_ref_bits(py, modules[2]);
        let result = importlib_transaction_no_fromlist_value(
            py,
            "branch.leaf",
            1,
            "return_pkg.branch.leaf",
            cache,
            modules[2],
        )
        .unwrap();
        assert_eq!(result, modules[2]);
        assert_eq!(
            unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() },
            before + 1,
        );
        dec_ref_bits(py, result);
        dec_ref_bits(py, MoltObject::from_ptr(cache).bits());
        for bits in modules.into_iter().chain(keys) {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn importlib_cached_empty_namespace_preserves_identity_and_owner() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let name = alloc_str_bits(py, "cached_empty").unwrap();
        let module = crate::object::builders::alloc_module_obj(py, name);
        assert!(!module.is_null());
        let bits = MoltObject::from_ptr(module).bits();
        let modules = alloc_dict_with_pairs(py, &[name, bits]);
        assert!(!modules.is_null());
        let before = unsafe { (*crate::header_from_obj_ptr(module)).ref_count_snapshot() };
        let imported = importlib_import_resolved_module(py, "cached_empty", name, modules, true);
        assert!(!exception_pending(py));
        assert_eq!(imported, bits);
        assert_eq!(
            unsafe { (*crate::header_from_obj_ptr(module)).ref_count_snapshot() },
            before + 1
        );
        dec_ref_bits(py, imported);
        dec_ref_bits(py, MoltObject::from_ptr(modules).bits());
        dec_ref_bits(py, bits);
        dec_ref_bits(py, name);
    });
}
