use super::*;
use crate::*;

fn class_with_field(py: &PyToken<'_>) -> u64 {
    let name = attr_name_bits_from_bytes(py, b"PollLogicalClass").unwrap();
    let field = attr_name_bits_from_bytes(py, b"field").unwrap();
    let offsets_key = attr_name_bits_from_bytes(py, b"__molt_field_offsets__").unwrap();
    let size_key = attr_name_bits_from_bytes(py, b"__molt_layout_size__").unwrap();
    let class = crate::molt_class_new(name);
    let offsets = crate::alloc_dict_with_pairs(py, &[field, MoltObject::from_int(0).bits()]);
    assert!(!offsets.is_null());
    let offsets = MoltObject::from_ptr(offsets).bits();
    crate::molt_set_attr_name(class, offsets_key, offsets);
    crate::molt_set_attr_name(class, size_key, MoltObject::from_int(16).bits());
    assert!(!exception_pending(py));
    unsafe { class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap()) }
        .expect("seal field layout");
    for bits in [name, field, offsets_key, size_key, offsets] {
        dec_ref_bits(py, bits);
    }
    class
}

#[test]
fn every_task_shape_keeps_capture_and_dictionary_ownership_disjoint() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = class_with_field(py);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let owner = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let owner_ptr = obj_from_bits(owner).as_ptr().unwrap();
            let baseline = (*header_from_obj_ptr(owner_ptr)).ref_count_snapshot();

            for shape in (0..=ObjectShapeId::MAX_ID)
                .filter_map(ObjectShapeId::from_u16)
                .filter(|&shape| object_shape_is_task(shape))
            {
                // Slot zero is empty, including for resource-bearing tasks.
                // Slot one is an ordinary owned capture, not a dictionary.
                let ptr = alloc_object_with_aux(
                    py,
                    std::mem::size_of::<MoltHeader>() + 16,
                    TYPE_ID_OBJECT,
                    ObjectAuxPreselection::Sidecar,
                );
                assert!(!ptr.is_null());
                *ptr.cast::<u64>() = MoltObject::none().bits();
                inc_ref_bits(py, owner);
                *ptr.cast::<u64>().add(1) = owner;
                assert!(object_init_shape_unpublished(ptr, shape));
                assert!(object_init_class_edge_unpublished(
                    py,
                    ptr,
                    class,
                    ClassEdgeOwnership::Owned
                ));
                object_mark_has_ptrs(py, ptr);
                assert_eq!(object_class_bits(ptr), class);
                assert!(!object_has_class_shape(ptr), "{shape:?}");
                assert!(instance_dict_bits_ptr(ptr).is_null(), "{shape:?}");
                assert!(matches!(
                    field_storage::current_dictionary(py, ptr),
                    Ok(None)
                ));
                assert!(field_storage::field_at_offset(py, ptr, 0).is_none());
                let mut fields = 0;
                field_storage::for_each_instance_field(py, ptr, class_ptr, &mut |_, _| fields += 1);
                assert_eq!(fields, 0, "{shape:?}");
                assert!(field_storage::slot_state_names(py, ptr).is_none());
                field_storage::reset(py, ptr);
                assert_eq!(*ptr.cast::<u64>().add(1), owner);

                let mut edges: Vec<*mut u8> = Vec::new();
                heap_lifecycle::visit_owned_edges(py, ptr, &mut |child| edges.push(child));
                assert_eq!(
                    edges.iter().filter(|&&child| child == owner_ptr).count(),
                    1,
                    "{shape:?}"
                );
                assert_eq!(
                    edges.iter().filter(|&&child| child == class_ptr).count(),
                    1,
                    "{shape:?}"
                );
                assert_eq!(heap_lifecycle::try_clear_cycle_edges(py, ptr), 0);
                assert_eq!(*ptr.cast::<u64>().add(1), MoltObject::none().bits());
                assert_eq!(
                    (*header_from_obj_ptr(owner_ptr)).ref_count_snapshot(),
                    baseline
                );
                assert_eq!(heap_lifecycle::try_clear_cycle_edges(py, ptr), 0);
                assert_eq!(
                    (*header_from_obj_ptr(owner_ptr)).ref_count_snapshot(),
                    baseline
                );
                dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
                assert!(!exception_pending(py));
            }

            // The identical logical class still gives real class allocations
            // their managed field and dictionary tail.
            let ordinary = builders::alloc_class_instance(py, 16, class);
            let ptr = obj_from_bits(ordinary).as_ptr().unwrap();
            assert!(object_has_class_shape(ptr));
            assert!(!instance_dict_bits_ptr(ptr).is_null());
            assert!(field_storage::field_at_offset(py, ptr, 0).is_some());
            dec_ref_bits(py, ordinary);
            dec_ref_bits(py, owner);
            dec_ref_bits(py, class);
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn task_construction_without_poll_address_still_owns_capture_storage() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let owner = MoltObject::from_ptr(alloc_list(py, &[])).bits();
            let owner_ptr = obj_from_bits(owner).as_ptr().unwrap();
            let baseline = (*header_from_obj_ptr(owner_ptr)).ref_count_snapshot();
            for kind in [crate::TASK_KIND_FUTURE, crate::TASK_KIND_COROUTINE] {
                let task = crate::molt_task_new(0, 8, kind);
                let ptr = obj_from_bits(task).as_ptr().unwrap();
                assert_eq!(object_shape_id(ptr), ObjectShapeId::GenericTaskPayload);
                inc_ref_bits(py, owner);
                *ptr.cast::<u64>() = owner;
                assert!(object_init_class_edge_unpublished(
                    py,
                    ptr,
                    builtin_classes(py).coroutine_wrapper,
                    ClassEdgeOwnership::Owned
                ));
                assert!(instance_dict_bits_ptr(ptr).is_null());
                let mut captures = 0;
                heap_lifecycle::visit_owned_edges(py, ptr, &mut |child| {
                    captures += usize::from(child == owner_ptr);
                });
                assert_eq!(captures, 1);
                dec_ref_bits(py, task);
                assert_eq!(
                    (*header_from_obj_ptr(owner_ptr)).ref_count_snapshot(),
                    baseline
                );
            }
            dec_ref_bits(py, owner);
            assert!(!exception_pending(py));
        }
    });
}
