use super::*;

#[test]
fn class_namespace_cell_rejection_releases_unpublished_class_payload() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let name = attr_name_bits_from_bytes(_py, b"RejectedNamespaceOwner").unwrap();
        let cell_key = attr_name_bits_from_bytes(_py, b"__classdictcell__").unwrap();
        let payload_key = attr_name_bits_from_bytes(_py, b"payload").unwrap();
        let payload = alloc_list(_py, &[]);
        assert!(!payload.is_null());
        let payload_bits = MoltObject::from_ptr(payload).bits();
        let attrs = [
            cell_key,
            MoltObject::from_int(123).bits(),
            payload_key,
            payload_bits,
        ];
        let namespace = alloc_dict_with_pairs(_py, &attrs);
        assert!(!namespace.is_null());
        let namespace_bits = MoltObject::from_ptr(namespace).bits();
        let before = unsafe { (*crate::header_from_obj_ptr(payload)).ref_count_snapshot() };
        let result = molt_type_new(
            builtin_classes(_py).type_obj,
            name,
            MoltObject::none().bits(),
            namespace_bits,
            MoltObject::none().bits(),
        );
        assert!(obj_from_bits(result).is_none());
        assert!(exception_pending(_py));
        crate::molt_exception_clear();
        assert_eq!(
            unsafe { (*crate::header_from_obj_ptr(payload)).ref_count_snapshot() },
            before,
            "rejected type construction must release its copied namespace"
        );
        dec_ref_bits(_py, namespace_bits);
        dec_ref_bits(_py, payload_bits);
        dec_ref_bits(_py, payload_key);
        dec_ref_bits(_py, cell_key);
        dec_ref_bits(_py, name);
        assert!(!exception_pending(_py));
    });
}

#[test]
fn class_namespace_cell_publishes_actual_copied_dictionary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let name = attr_name_bits_from_bytes(_py, b"NamespaceOwner").unwrap();
        let cell_key = attr_name_bits_from_bytes(_py, b"__classdictcell__").unwrap();
        let key = attr_name_bits_from_bytes(_py, b"injected").unwrap();
        let prepared = alloc_dict_with_pairs(_py, &[key, MoltObject::from_int(1).bits()]);
        let prepared_bits = MoltObject::from_ptr(prepared).bits();
        let cell = crate::object::cells::alloc_cell(_py, prepared_bits);
        let cell_bits = MoltObject::from_ptr(cell).bits();
        let class = alloc_class_obj(_py, name);
        assert!(!prepared.is_null() && !cell.is_null() && !class.is_null());
        unsafe {
            let copied_bits = class_dict_bits(class);
            let copied = obj_from_bits(copied_bits).as_ptr().unwrap();
            dict_set_in_place(_py, copied, key, MoltObject::from_int(2).bits());
            dict_set_in_place(_py, copied, cell_key, cell_bits);
            assert!(class_finalize_namespace_metadata(_py, class, name));
            assert_eq!(crate::object::cells::cell_value_bits(cell), copied_bits);
            assert_eq!(dict_get_in_place(_py, copied, cell_key), None);
            dict_set_in_place(_py, prepared, key, MoltObject::from_int(3).bits());
            assert_eq!(
                dict_get_in_place(_py, copied, key),
                Some(MoltObject::from_int(2).bits())
            );
            dict_set_in_place(_py, copied, key, MoltObject::from_int(4).bits());
            let captured = crate::object::cells::cell_value_bits(cell);
            assert_eq!(
                dict_get_in_place(_py, obj_from_bits(captured).as_ptr().unwrap(), key),
                Some(MoltObject::from_int(4).bits())
            );
        }
        dec_ref_bits(_py, cell_bits);
        dec_ref_bits(_py, prepared_bits);
        dec_ref_bits(_py, MoltObject::from_ptr(class).bits());
        dec_ref_bits(_py, key);
        dec_ref_bits(_py, cell_key);
        dec_ref_bits(_py, name);
        assert!(!exception_pending(_py));
    });
}
