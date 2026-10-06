use super::*;
use crate::{MoltObject, alloc_string, builtin_classes, dec_ref_bits};

fn list_class(py: &PyToken<'_>, slots: Option<&[&[u8]]>) -> u64 {
    let name = alloc_string(py, b"StoredList");
    let name = MoltObject::from_ptr(name).bits();
    let class = crate::molt_class_new(name);
    dec_ref_bits(py, name);
    let _ = crate::molt_class_set_base(class, builtin_classes(py).list);
    if let Some(names) = slots {
        let fields: Vec<u64> = names
            .iter()
            .map(|name| MoltObject::from_ptr(alloc_string(py, name)).bits())
            .collect();
        let declaration = MoltObject::from_ptr(crate::alloc_tuple(py, &fields)).bits();
        let key = crate::attr_name_bits_from_bytes(py, b"__slots__").unwrap();
        crate::molt_set_attr_name(class, key, declaration);
        dec_ref_bits(py, declaration);
        dec_ref_bits(py, key);
        for field in fields {
            dec_ref_bits(py, field);
        }
    }
    assert!(!crate::exception_pending(py));
    let ptr = obj_from_bits(class).as_ptr().unwrap();
    assert!(unsafe { class_finish_definition(py, ptr) }.is_ok());
    class
}

#[test]
fn list_subclass_native_prefix_and_lifecycle_compose_all_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = list_class(py, Some(&[b"field", b"__dict__", b"__weakref__"]));
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        assert_eq!(unsafe { class_instance_type_id(class_ptr) }, TYPE_ID_LIST);
        assert_eq!(unsafe { class_reserved_layout_prefix(class_ptr) }, 8);
        let instance = unsafe { crate::call::class_init::alloc_instance_for_class(py, class_ptr) };
        let ptr = obj_from_bits(instance).as_ptr().unwrap();
        assert_eq!(unsafe { object_type_id(ptr) }, TYPE_ID_LIST);
        assert!(!unsafe { layout::seq_vec_ptr(ptr) }.is_null());
        assert_eq!(unsafe { crate::list_len(ptr) }, 0);
        assert_eq!(crate::type_of_bits(py, instance), class);
        assert!(weakref::object_supports_weakrefs(py, instance));

        let target = alloc_string(py, b"list owned target");
        let target_bits = MoltObject::from_ptr(target).bits();
        crate::molt_list_append(instance, target_bits);
        let field = crate::attr_name_bits_from_bytes(py, b"field").unwrap();
        crate::molt_set_attr_name(instance, field, target_bits);
        let extra = crate::attr_name_bits_from_bytes(py, b"extra").unwrap();
        crate::molt_set_attr_name(instance, extra, target_bits);
        assert!(!crate::exception_pending(py));
        dec_ref_bits(py, field);
        dec_ref_bits(py, extra);
        dec_ref_bits(py, target_bits);

        let dictionary = obj_from_bits(unsafe { instance_dict_bits(ptr) })
            .as_ptr()
            .unwrap();
        let mut edges = Vec::new();
        unsafe { heap_lifecycle::visit_owned_edges(py, ptr, &mut |edge| edges.push(edge)) };
        assert_eq!(edges.iter().filter(|&&edge| edge == target).count(), 2);
        assert!(edges.contains(&dictionary));
        assert!(edges.contains(&class_ptr));
        unsafe { heap_lifecycle::clear_cycle_edges(py, ptr) };
        assert_eq!(unsafe { crate::list_len(ptr) }, 0);
        assert_eq!(unsafe { instance_dict_bits(ptr) }, 0);
        let mut remaining = Vec::new();
        unsafe { heap_lifecycle::visit_owned_edges(py, ptr, &mut |edge| remaining.push(edge)) };
        assert_eq!(remaining, vec![class_ptr]);
        unsafe { heap_lifecycle::clear_cycle_edges(py, ptr) };
        dec_ref_bits(py, instance);
        dec_ref_bits(py, class);
    });
}

#[test]
fn list_unpublished_rollback_does_not_skip_class_state_without_a_vec() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = list_class(py, None);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        let payload = unsafe { layout::class_cached_layout_size(class_ptr) }.unwrap();
        let ptr = alloc_object_zeroed_unpublished_with_aux(
            py,
            std::mem::size_of::<MoltHeader>() + payload,
            TYPE_ID_LIST,
            ObjectAuxPreselection::ClassInline,
        );
        assert!(!ptr.is_null());
        assert!(unsafe {
            object_init_class_edge_unpublished(py, ptr, class, ClassEdgeOwnership::Owned)
        });
        let dictionary = crate::alloc_dict_with_pairs(py, &[]);
        unsafe { instance_set_dict_bits(py, ptr, MoltObject::from_ptr(dictionary).bits()) };
        let mut edges = Vec::new();
        unsafe { heap_lifecycle::visit_owned_edges(py, ptr, &mut |edge| edges.push(edge)) };
        assert!(edges.contains(&dictionary));
        unsafe { heap_lifecycle::clear_cycle_edges(py, ptr) };
        assert_eq!(unsafe { instance_dict_bits(ptr) }, 0);
        dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
        dec_ref_bits(py, class);
    });
}

#[test]
fn managed_list_abi_distinguishes_physical_storage_from_exact_class() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        let class = list_class(py, None);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        let subtype = unsafe { crate::call::class_init::alloc_instance_for_class(py, class_ptr) };
        let exact = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
        for (bits, expected_exact) in [(subtype, 0), (exact, 1)] {
            let view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
            assert!(!view.is_null());
            unsafe {
                assert_eq!(molt_cpython_abi::api::sequences::PyList_Check(view), 1);
                assert_eq!(
                    molt_cpython_abi::api::sequences::PyList_CheckExact(view),
                    expected_exact
                );
                assert_eq!(molt_cpython_abi::api::sequences::PyList_Size(view), 0);
            }
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, bits);
        }
        dec_ref_bits(py, class);
    });
}

#[test]
fn exact_list_class_edge_does_not_admit_instance_state_or_reassignment() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let builtins = builtin_classes(py);
        let class_ptr = obj_from_bits(builtins.list).as_ptr().unwrap();
        let exact = unsafe { crate::call::class_init::alloc_instance_for_class(py, class_ptr) };
        let literal = MoltObject::from_ptr(crate::alloc_list(py, &[])).bits();
        let child = list_class(py, Some(&[]));
        let class_name = crate::attr_name_bits_from_bytes(py, b"__class__").unwrap();
        for bits in [literal, exact] {
            let ptr = obj_from_bits(bits).as_ptr().unwrap();
            assert!(!unsafe { field_storage::allows_dictionary(py, ptr) });
            assert!(!weakref::object_supports_weakrefs(py, bits));
            assert_eq!(unsafe { instance_dict_bits(ptr) }, 0);
            for target in [child, builtins.list] {
                crate::molt_set_attr_name(bits, class_name, target);
                assert!(crate::exception_pending(py));
                crate::molt_exception_clear();
                assert_eq!(crate::type_of_bits(py, bits), builtins.list);
                assert_eq!(unsafe { instance_dict_bits(ptr) }, 0);
            }
            let mut edges = Vec::new();
            unsafe { heap_lifecycle::visit_owned_edges(py, ptr, &mut |edge| edges.push(edge)) };
            assert!(edges.iter().all(|edge| *edge == class_ptr));
            dec_ref_bits(py, bits);
        }
        dec_ref_bits(py, class_name);
        dec_ref_bits(py, child);
        assert!(!crate::exception_pending(py));
    });
}
