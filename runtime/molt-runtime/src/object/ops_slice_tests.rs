use super::*;
use crate::object::{
    accessors, builders, class_finish_definition, field_storage, gc, layout, ops_builtins,
    seq_access,
};

unsafe fn new_dataclass_class(_py: &PyToken<'_>, name: &[u8]) -> u64 {
    let name_bits = attr_name_bits_from_bytes(_py, name).expect("class name");
    let class_bits = molt_class_new(name_bits);
    dec_ref_bits(_py, name_bits);
    assert!(obj_from_bits(class_bits).as_ptr().is_some());
    assert_eq!(
        molt_class_set_base(class_bits, builtin_classes(_py).object),
        MoltObject::none().bits()
    );
    assert!(!exception_pending(_py));
    class_bits
}

fn heap_refcount(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
    unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

fn new_unpublished_dataclass(_py: &PyToken<'_>, value_bits: u64, field_name: &[u8]) -> u64 {
    let name_bits = attr_name_bits_from_bytes(_py, b"Record").expect("dataclass name");
    let field_bits = attr_name_bits_from_bytes(_py, field_name).expect("field name");
    let field_names_ptr = alloc_tuple(_py, &[field_bits]);
    let values_ptr = alloc_tuple(_py, &[value_bits]);
    assert!(!field_names_ptr.is_null());
    assert!(!values_ptr.is_null());
    let field_names_bits = MoltObject::from_ptr(field_names_ptr).bits();
    let values_bits = MoltObject::from_ptr(values_ptr).bits();
    let instance_bits = molt_dataclass_new(
        name_bits,
        field_names_bits,
        values_bits,
        MoltObject::from_int(0).bits(),
    );
    for bits in [values_bits, field_names_bits, field_bits, name_bits] {
        dec_ref_bits(_py, bits);
    }
    assert!(!exception_pending(_py));
    instance_bits
}

#[test]
fn dataclass_payload_stays_private_until_class_finalization_publishes_once() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let value_ptr = alloc_string(_py, b"owned value");
        assert!(!value_ptr.is_null());
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let value_owners = heap_refcount(value_bits);
        let instance_bits = new_unpublished_dataclass(_py, value_bits, b"field");
        let instance_ptr = obj_from_bits(instance_bits)
            .as_ptr()
            .expect("dataclass instance");
        let header = unsafe { &*crate::object::header_from_obj_ptr(instance_ptr) };
        assert!(!header.gc_is_published());
        assert_eq!(unsafe { object_class_bits(instance_ptr) }, 0);
        assert_eq!(heap_refcount(value_bits), value_owners + 1);

        let class_bits = unsafe { new_dataclass_class(_py, b"Record") };
        let class_owners = heap_refcount(class_bits);
        assert_eq!(
            unsafe { dataclass_finish_construction_unpublished(_py, instance_ptr, class_bits) },
            MoltObject::none().bits()
        );
        assert!(!exception_pending(_py));
        assert!(header.gc_is_published());
        assert_eq!(unsafe { object_class_bits(instance_ptr) }, class_bits);
        assert_eq!(heap_refcount(class_bits), class_owners + 1);

        dec_ref_bits(_py, instance_bits);
        assert_eq!(heap_refcount(class_bits), class_owners);
        assert_eq!(heap_refcount(value_bits), value_owners);
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, value_bits);
        assert!(!exception_pending(_py));
    });
}

#[test]
fn dataclass_finalization_rejects_retry_and_failed_construction_tears_down_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let value_ptr = alloc_string(_py, b"rollback value");
        assert!(!value_ptr.is_null());
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let value_owners = heap_refcount(value_bits);

        let rejected_bits = new_unpublished_dataclass(_py, value_bits, b"field");
        let rejected_ptr = obj_from_bits(rejected_bits)
            .as_ptr()
            .expect("rejected dataclass");
        assert_eq!(
            unsafe {
                dataclass_finish_construction_unpublished(
                    _py,
                    rejected_ptr,
                    MoltObject::from_int(7).bits(),
                )
            },
            MoltObject::none().bits()
        );
        assert!(exception_pending(_py));
        assert!(!unsafe { (*crate::object::header_from_obj_ptr(rejected_ptr)).gc_is_published() });
        assert_eq!(unsafe { object_class_bits(rejected_ptr) }, 0);
        crate::molt_exception_clear();
        dec_ref_bits(_py, rejected_bits);
        assert_eq!(heap_refcount(value_bits), value_owners);

        let attached_bits = new_unpublished_dataclass(_py, value_bits, b"field");
        let attached_ptr = obj_from_bits(attached_bits)
            .as_ptr()
            .expect("attached dataclass");
        let rollback_class = unsafe { new_dataclass_class(_py, b"RollbackRecord") };
        let rollback_ptr = obj_from_bits(rollback_class)
            .as_ptr()
            .expect("rollback class");
        unsafe { crate::object::class_finish_definition(_py, rollback_ptr) }
            .expect("seal rollback class");
        let rollback_class_owners = heap_refcount(rollback_class);
        assert!(unsafe {
            crate::object::object_init_class_edge_unpublished(
                _py,
                attached_ptr,
                rollback_class,
                crate::object::ClassEdgeOwnership::Owned,
            )
        });
        assert!(!unsafe { (*crate::object::header_from_obj_ptr(attached_ptr)).gc_is_published() });
        assert_eq!(heap_refcount(rollback_class), rollback_class_owners + 1);
        assert_eq!(
            unsafe { dataclass_finish_construction_unpublished(_py, attached_ptr, rollback_class) },
            MoltObject::none().bits()
        );
        assert!(exception_pending(_py));
        assert!(!unsafe { (*crate::object::header_from_obj_ptr(attached_ptr)).gc_is_published() });
        assert_eq!(unsafe { object_class_bits(attached_ptr) }, rollback_class);
        assert_eq!(heap_refcount(rollback_class), rollback_class_owners + 1);
        crate::molt_exception_clear();
        dec_ref_bits(_py, attached_bits);
        assert_eq!(heap_refcount(rollback_class), rollback_class_owners);
        assert_eq!(heap_refcount(value_bits), value_owners);
        dec_ref_bits(_py, rollback_class);

        let instance_bits = new_unpublished_dataclass(_py, value_bits, b"field");
        let instance_ptr = obj_from_bits(instance_bits)
            .as_ptr()
            .expect("dataclass instance");
        let first_class = unsafe { new_dataclass_class(_py, b"FirstRecord") };
        assert_eq!(
            molt_dataclass_set_class(instance_bits, first_class),
            MoltObject::none().bits()
        );
        assert!(!exception_pending(_py));
        assert!(unsafe { (*crate::object::header_from_obj_ptr(instance_ptr)).gc_is_published() });

        let retry_class = unsafe { new_dataclass_class(_py, b"RetryRecord") };
        let retry_ptr = obj_from_bits(retry_class).as_ptr().expect("retry class");
        assert!(!unsafe { crate::object::class_definition_is_finished(retry_ptr) });
        assert_eq!(
            molt_dataclass_set_class(instance_bits, retry_class),
            MoltObject::none().bits()
        );
        assert!(exception_pending(_py));
        assert_eq!(unsafe { object_class_bits(instance_ptr) }, first_class);
        assert!(!unsafe { crate::object::class_definition_is_finished(retry_ptr) });
        crate::molt_exception_clear();

        dec_ref_bits(_py, instance_bits);
        dec_ref_bits(_py, retry_class);
        dec_ref_bits(_py, first_class);
        assert_eq!(heap_refcount(value_bits), value_owners);
        dec_ref_bits(_py, value_bits);
        assert!(!exception_pending(_py));
    });
}

fn published_record(py: &PyToken<'_>, value: u64) -> (u64, u64) {
    let object = new_unpublished_dataclass(py, value, b"field");
    let class = unsafe { new_dataclass_class(py, b"Record") };
    molt_dataclass_set_class(object, class);
    assert!(!exception_pending(py));
    (object, class)
}

#[test]
fn dataclass_dictionary_is_the_only_ordinary_field_owner_after_exposure() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
        let (object, class) = published_record(_py, value);
        let ptr = obj_from_bits(object).as_ptr().unwrap();
        let field = attr_name_bits_from_bytes(_py, b"field").unwrap();
        let dict_name = attr_name_bits_from_bytes(_py, b"__dict__").unwrap();
        let dict = unsafe {
            crate::builtins::attr::dataclass_attr_lookup_raw(_py, ptr, dict_name).unwrap()
        };
        let dictionary = obj_from_bits(dict).as_ptr().unwrap();
        unsafe {
            assert!(is_missing_bits(_py, (&*dataclass_fields_ptr(ptr))[0]));
            assert_eq!(heap_refcount(value), 2, "one field owner, no mirror");
            dict_set_in_place(_py, dictionary, field, MoltObject::from_int(9).bits());
            assert_eq!(heap_refcount(value), 1);
        }
        assert_eq!(
            molt_dataclass_get(object, MoltObject::from_int(0).bits()),
            MoltObject::from_int(9).bits()
        );
        molt_dataclass_set(object, MoltObject::from_int(0).bits(), value);
        assert_eq!(
            unsafe { dict_get_in_place(_py, dictionary, field) },
            Some(value)
        );
        let state = crate::object::ops_builtins::molt_object_getstate(object);
        assert_eq!(state, dict, "getstate consumes the same backing authority");
        dec_ref_bits(_py, state);
        unsafe {
            assert!(dict_del_in_place(_py, dictionary, field));
            assert!(crate::builtins::attr::dataclass_attr_lookup_raw(_py, ptr, field).is_none());
            assert!(is_missing_bits(_py, (&*dataclass_fields_ptr(ptr))[0]));
        }
        assert_eq!(heap_refcount(value), 1);
        let replacement = MoltObject::from_ptr(alloc_dict_with_pairs(
            _py,
            &[field, MoltObject::from_int(41).bits()],
        ))
        .bits();
        crate::molt_set_attr_name(object, dict_name, replacement);
        assert!(!exception_pending(_py));
        assert_eq!(
            molt_dataclass_get(object, MoltObject::from_int(0).bits()),
            MoltObject::from_int(41).bits()
        );
        crate::molt_del_attr_name(object, dict_name);
        assert!(!exception_pending(_py));
        assert_eq!(unsafe { instance_dict_bits(ptr) }, 0);
        assert_eq!(
            unsafe { dict_get_in_place(_py, obj_from_bits(replacement).as_ptr().unwrap(), field) },
            Some(MoltObject::from_int(41).bits())
        );
        for bits in [dict, replacement, object, class, field, dict_name, value] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use std::sync::atomic::{AtomicU64, Ordering};

static DATACLASS_CALLBACK_OWNER: AtomicU64 = AtomicU64::new(0);
static DATACLASS_CALLBACK_SEEN: AtomicU64 = AtomicU64::new(0);

extern "C" fn dataclass_payload(_value: u64) -> u64 {
    MoltObject::none().bits()
}

fn dataclass_callable(py: &PyToken<'_>, name: &str, target: extern "C" fn(u64) -> u64) -> u64 {
    let ptr = alloc_runtime_function_obj(py, runtime_fn_addr(name, target as *const ()), 1);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

extern "C" fn dataclass_reentrant_replacement(_weak: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let owner = DATACLASS_CALLBACK_OWNER.load(Ordering::SeqCst);
        let ptr = obj_from_bits(owner).as_ptr().unwrap();
        unsafe {
            let seen = crate::object::accessors::object_field_get_ptr_raw(_py, ptr, 0);
            DATACLASS_CALLBACK_SEEN.store(seen, Ordering::SeqCst);
            dec_ref_bits(_py, seen);
            crate::object::field_storage::materialize(_py, ptr).expect("callback dictionary");
        }
        molt_dataclass_set(
            owner,
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(77).bits(),
        );
        MoltObject::none().bits()
    })
}

#[test]
fn dataclass_mutation_publishes_before_release_across_every_backing_transition() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for materialized in [false, true] {
            for operation in 0..4 {
                let old = dataclass_callable(_py, "dataclass_payload", dataclass_payload);
                let callback = dataclass_callable(
                    _py,
                    "dataclass_reentrant_replacement",
                    dataclass_reentrant_replacement,
                );
                let weak_type = crate::molt_weakref_reference_type();
                let weak = crate::molt_weakref_new(weak_type, old, callback);
                dec_ref_bits(_py, weak_type);
                let (owner, class) = published_record(_py, old);
                let ptr = obj_from_bits(owner).as_ptr().unwrap();
                let field = attr_name_bits_from_bytes(_py, b"field").unwrap();
                if materialized {
                    unsafe {
                        crate::object::field_storage::materialize(_py, ptr).unwrap();
                    }
                }
                dec_ref_bits(_py, old);
                DATACLASS_CALLBACK_OWNER.store(owner, Ordering::SeqCst);
                DATACLASS_CALLBACK_SEEN.store(0, Ordering::SeqCst);
                let incoming = MoltObject::from_int(22).bits();
                match operation {
                    0 => {
                        molt_dataclass_set(owner, MoltObject::from_int(0).bits(), incoming);
                    }
                    1 => {
                        crate::molt_set_attr_name(owner, field, incoming);
                    }
                    2 => {
                        crate::molt_del_attr_name(owner, field);
                    }
                    3 => unsafe {
                        crate::object::field_storage::reset(_py, ptr);
                    },
                    _ => unreachable!(),
                }
                assert!(!exception_pending(_py));
                let expected = if operation < 2 {
                    incoming
                } else {
                    missing_bits(_py)
                };
                assert_eq!(DATACLASS_CALLBACK_SEEN.load(Ordering::SeqCst), expected);
                assert_eq!(
                    molt_dataclass_get(owner, MoltObject::from_int(0).bits()),
                    MoltObject::from_int(77).bits(),
                    "outer mutation must not overwrite reentry"
                );
                assert!(unsafe { is_missing_bits(_py, (&*dataclass_fields_ptr(ptr))[0]) });
                assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
                DATACLASS_CALLBACK_OWNER.store(0, Ordering::SeqCst);
                for bits in [owner, class, field, weak, callback] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!exception_pending(_py));
            }
        }
    });
}

#[test]
fn dataclass_declared_slot_and_same_name_dictionary_key_remain_independent() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let field = attr_name_bits_from_bytes(_py, b"field").unwrap();
        let dict_name = attr_name_bits_from_bytes(_py, b"__dict__").unwrap();
        let slots_name = attr_name_bits_from_bytes(_py, b"__slots__").unwrap();
        let offsets_name = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__").unwrap();
        let size_name = attr_name_bits_from_bytes(_py, b"__molt_layout_size__").unwrap();
        let slots = MoltObject::from_ptr(alloc_tuple(_py, &[field, dict_name])).bits();
        let offsets = MoltObject::from_ptr(alloc_dict_with_pairs(
            _py,
            &[field, MoltObject::from_int(0).bits()],
        ))
        .bits();
        let class = unsafe { new_dataclass_class(_py, b"SlottedRecord") };
        for (name, value) in [
            (slots_name, slots),
            (offsets_name, offsets),
            (size_name, MoltObject::from_int(16).bits()),
        ] {
            crate::molt_set_attr_name(class, name, value);
        }
        let object = new_unpublished_dataclass(_py, MoltObject::from_int(5).bits(), b"field");
        let ptr = obj_from_bits(object).as_ptr().unwrap();
        molt_dataclass_set_class(object, class);
        assert!(!exception_pending(_py));
        let dict = unsafe { crate::object::field_storage::materialize(_py, ptr).unwrap() };
        unsafe {
            dict_set_in_place(
                _py,
                obj_from_bits(dict).as_ptr().unwrap(),
                field,
                MoltObject::from_int(99).bits(),
            );
            assert_eq!(
                (&*dataclass_fields_ptr(ptr))[0],
                MoltObject::from_int(5).bits()
            );
        }
        assert_eq!(
            molt_dataclass_get(object, MoltObject::from_int(0).bits()),
            MoltObject::from_int(5).bits()
        );
        unsafe {
            crate::object::accessors::object_field_delete_ptr_raw(_py, ptr, 0);
        }
        assert_eq!(
            unsafe { dict_get_in_place(_py, obj_from_bits(dict).as_ptr().unwrap(), field) },
            Some(MoltObject::from_int(99).bits())
        );
        unsafe {
            crate::object::field_storage::replace_dictionary(_py, ptr, None);
        }
        for bits in [
            object,
            class,
            field,
            dict_name,
            slots_name,
            offsets_name,
            size_name,
            slots,
            offsets,
        ] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

static UNPUBLISHED_FINALIZER_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn observe_unpublished_finalizer(_object: u64) -> u64 {
    UNPUBLISHED_FINALIZER_CALLS.fetch_add(1, Ordering::SeqCst);
    MoltObject::none().bits()
}

#[test]
fn failed_dataclass_metadata_preparation_never_attaches_finalizing_class() {
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let class = unsafe { new_dataclass_class(_py, b"FailedPreparation") };
        let finalizer_name = attr_name_bits_from_bytes(_py, b"__del__").unwrap();
        let finalizer = dataclass_callable(
            _py,
            "observe_unpublished_finalizer",
            observe_unpublished_finalizer,
        );
        crate::molt_set_attr_name(class, finalizer_name, finalizer);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        unsafe {
            crate::object::class_finish_definition(_py, class_ptr).unwrap();
        }
        // A valid non-ASCII Python identifier avoids the immortal ASCII-name
        // pool: after TLS eviction metadata preparation must really allocate.
        let object =
            new_unpublished_dataclass(_py, MoltObject::from_int(1).bits(), "δfield".as_bytes());
        let ptr = obj_from_bits(object).as_ptr().unwrap();
        let class_owners = heap_refcount(class);
        // Force cold descriptor-key preparation using the existing cache reset
        // and resource-denial mechanism, not a constructor-only failure hook.
        crate::builtins::attr::clear_attr_tls_caches(_py);
        let _ = missing_bits(_py);
        UNPUBLISHED_FINALIZER_CALLS.store(0, Ordering::SeqCst);
        struct TrackerReset;
        impl Drop for TrackerReset {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let reset = TrackerReset;
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(0),
            ..Default::default()
        })));
        unsafe {
            dataclass_finish_construction_unpublished(_py, ptr, class);
        }
        drop(reset);
        assert!(exception_pending(_py));
        assert_eq!(unsafe { object_class_bits(ptr) }, 0);
        assert!(!unsafe { (*crate::object::header_from_obj_ptr(ptr)).gc_is_published() });
        assert_eq!(heap_refcount(class), class_owners);
        crate::molt_exception_clear();
        dec_ref_bits(_py, object);
        assert_eq!(UNPUBLISHED_FINALIZER_CALLS.load(Ordering::SeqCst), 0);
        for bits in [class, finalizer_name, finalizer] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn dataclass_reset_resource_denial_preserves_every_existing_owner() {
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for materialized in [false, true] {
            let value = MoltObject::from_ptr(alloc_list(_py, &[])).bits();
            let (object, class) = published_record(_py, value);
            let ptr = obj_from_bits(object).as_ptr().unwrap();
            if materialized {
                unsafe {
                    crate::object::field_storage::materialize(_py, ptr).unwrap();
                }
            }
            let old_dict = unsafe { instance_dict_bits(ptr) };
            let old_slot = unsafe { (&*dataclass_fields_ptr(ptr))[0] };
            struct TrackerReset;
            impl Drop for TrackerReset {
                fn drop(&mut self) {
                    set_tracker(Box::new(UnlimitedTracker));
                }
            }
            let reset = TrackerReset;
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            unsafe {
                crate::object::field_storage::reset(_py, ptr);
            }
            drop(reset);
            assert!(exception_pending(_py));
            assert_eq!(unsafe { instance_dict_bits(ptr) }, old_dict);
            assert_eq!(unsafe { (&*dataclass_fields_ptr(ptr))[0] }, old_slot);
            assert_eq!(heap_refcount(value), 2);
            crate::molt_exception_clear();
            let read = molt_dataclass_get(object, MoltObject::from_int(0).bits());
            assert_eq!(read, value);
            dec_ref_bits(_py, read);
            for bits in [object, class, value] {
                dec_ref_bits(_py, bits);
            }
            assert!(!exception_pending(_py));
        }
    });
}

#[test]
fn dataclass_slot_state_follows_mro_then_declaration_order() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let base_field = attr_name_bits_from_bytes(_py, b"base_field").unwrap();
        let child_first = attr_name_bits_from_bytes(_py, b"child_first").unwrap();
        let child_second = attr_name_bits_from_bytes(_py, b"child_second").unwrap();
        let slots_name = attr_name_bits_from_bytes(_py, b"__slots__").unwrap();
        let base_slots = MoltObject::from_ptr(alloc_tuple(_py, &[base_field])).bits();
        let child_slots =
            MoltObject::from_ptr(alloc_tuple(_py, &[child_first, child_second])).bits();
        let base = unsafe { new_dataclass_class(_py, b"SlotBase") };
        crate::molt_set_attr_name(base, slots_name, base_slots);
        unsafe {
            crate::object::class_finish_definition(_py, obj_from_bits(base).as_ptr().unwrap())
                .expect("seal base slots");
        }
        let child_name = attr_name_bits_from_bytes(_py, b"SlotChild").unwrap();
        let child = crate::molt_class_new(child_name);
        crate::molt_class_set_base(child, base);
        crate::molt_set_attr_name(child, slots_name, child_slots);
        let names =
            MoltObject::from_ptr(alloc_tuple(_py, &[base_field, child_first, child_second])).bits();
        let object = dataclass_new_from_value_slice(
            _py,
            child_name,
            names,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
            MoltObject::from_int(8).bits(),
        );
        molt_dataclass_set_class(object, child);
        assert!(!exception_pending(_py));
        let state = crate::object::ops_builtins::molt_object_getstate(object);
        assert!(!exception_pending(_py));
        let state_ptr = obj_from_bits(state).as_ptr().unwrap();
        let slot_state = unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(state_ptr, |parts| parts[1])
                .unwrap()
        };
        let keys = unsafe {
            dict_order(obj_from_bits(slot_state).as_ptr().unwrap())
                .chunks_exact(2)
                .map(|pair| pair[0])
                .collect::<Vec<_>>()
        };
        assert_eq!(keys, vec![child_first, child_second, base_field]);
        for bits in [
            state,
            object,
            names,
            child,
            child_name,
            base,
            base_slots,
            child_slots,
            slots_name,
            base_field,
            child_first,
            child_second,
        ] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn ordinary_slot_state_omits_shadowed_storage_but_reset_releases_both_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let field = attr_name_bits_from_bytes(py, b"x").unwrap();
        let slots_name = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
        let offsets_name = attr_name_bits_from_bytes(py, b"__molt_field_offsets__").unwrap();
        let size_name = attr_name_bits_from_bytes(py, b"__molt_layout_size__").unwrap();
        let slots = MoltObject::from_ptr(alloc_tuple(py, &[field])).bits();
        let make_class = |name: &[u8], base: u64, offset: i64| {
            let name = attr_name_bits_from_bytes(py, name).unwrap();
            let class = crate::molt_class_new(name);
            crate::molt_class_set_base(class, base);
            let offsets = MoltObject::from_ptr(alloc_dict_with_pairs(
                py,
                &[field, MoltObject::from_int(offset).bits()],
            ))
            .bits();
            crate::molt_set_attr_name(class, slots_name, slots);
            crate::molt_set_attr_name(class, offsets_name, offsets);
            crate::molt_set_attr_name(class, size_name, MoltObject::from_int(offset + 16).bits());
            unsafe { class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap()) }
                .expect("seal distinct physical slot");
            dec_ref_bits(py, offsets);
            dec_ref_bits(py, name);
            class
        };
        let base = make_class(b"ShadowBase", builtin_classes(py).object, 0);
        let child = make_class(b"ShadowChild", base, 8);
        let object = builders::alloc_class_instance(py, 24, child);
        let ptr = obj_from_bits(object).as_ptr().unwrap();
        unsafe { gc::gc_publish_initialized(py, ptr) };
        let base_value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        let child_value = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        unsafe { accessors::molt_object_field_set(object, 0, base_value) };
        assert_eq!(
            ops_builtins::molt_object_getstate(object),
            MoltObject::none().bits(),
            "missing visible slot must not reveal a populated base slot"
        );
        unsafe { accessors::molt_object_field_set(object, 8, child_value) };
        let state = ops_builtins::molt_object_getstate(object);
        assert!(!exception_pending(py));
        let slot_state = unsafe {
            seq_access::with_immutable_tuple_slice(
                obj_from_bits(state).as_ptr().unwrap(),
                |parts| parts[1],
            )
            .unwrap()
        };
        assert_eq!(
            unsafe { dict_order(obj_from_bits(slot_state).as_ptr().unwrap()).as_slice() },
            &[field, child_value]
        );
        dec_ref_bits(py, state);
        assert_eq!(heap_refcount(base_value), 2);
        assert_eq!(heap_refcount(child_value), 2);
        unsafe { field_storage::reset(py, ptr) };
        assert!(!exception_pending(py));
        assert_eq!(heap_refcount(base_value), 1);
        assert_eq!(heap_refcount(child_value), 1);
        for bits in [
            object,
            child,
            base,
            slots,
            field,
            slots_name,
            offsets_name,
            size_name,
            base_value,
            child_value,
        ] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

static SLOT_STATE_READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SLOT_STATE_READ_MODE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

extern "C" fn slot_state_getattribute(_object: u64, name: u64) -> u64 {
    use std::sync::atomic::Ordering;
    crate::with_gil_entry_nopanic!(py, {
        assert_eq!(
            string_obj_to_owned(obj_from_bits(name)).as_deref(),
            Some("x")
        );
        let count = SLOT_STATE_READS.fetch_add(1, Ordering::SeqCst) + 1;
        match SLOT_STATE_READ_MODE.load(Ordering::SeqCst) {
            1 => raise_exception::<u64>(py, "AttributeError", "slot absent"),
            2 => raise_exception::<u64>(py, "RuntimeError", "slot getter failed"),
            _ => MoltObject::from_int(count as i64).bits(),
        }
    })
}

#[test]
fn slot_state_uses_sequential_attribute_protocol_and_propagates_errors() {
    use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
    use std::sync::atomic::Ordering;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let field = attr_name_bits_from_bytes(py, b"x").unwrap();
        let slots_name = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
        let getter_name = attr_name_bits_from_bytes(py, b"__getattribute__").unwrap();
        let slots = MoltObject::from_ptr(alloc_tuple(py, &[field])).bits();
        let base = unsafe { new_dataclass_class(py, b"StateProtocolBase") };
        crate::molt_set_attr_name(base, slots_name, slots);
        unsafe {
            class_finish_definition(py, obj_from_bits(base).as_ptr().unwrap()).unwrap();
        }
        let child_name = attr_name_bits_from_bytes(py, b"StateProtocolChild").unwrap();
        let child = crate::molt_class_new(child_name);
        crate::molt_class_set_base(child, base);
        crate::molt_set_attr_name(child, slots_name, slots);
        let getter = MoltObject::from_ptr(alloc_runtime_function_obj(
            py,
            runtime_fn_addr(
                "slot_state_getattribute",
                slot_state_getattribute as *const (),
            ),
            2,
        ))
        .bits();
        crate::molt_set_attr_name(child, getter_name, getter);
        let class = obj_from_bits(child).as_ptr().unwrap();
        unsafe {
            class_finish_definition(py, class).unwrap();
        }
        let size = unsafe { layout::class_cached_layout_size(class).unwrap() };
        let object = builders::alloc_class_instance(py, size, child);
        unsafe {
            gc::gc_publish_initialized(py, obj_from_bits(object).as_ptr().unwrap());
        }
        for mode in 0..3 {
            SLOT_STATE_READS.store(0, Ordering::SeqCst);
            SLOT_STATE_READ_MODE.store(mode, Ordering::SeqCst);
            let state = ops_builtins::molt_object_getstate(object);
            assert_eq!(
                SLOT_STATE_READS.load(Ordering::SeqCst),
                if mode == 2 { 1 } else { 2 }
            );
            if mode == 0 {
                let slots = unsafe {
                    seq_access::with_immutable_tuple_slice(
                        obj_from_bits(state).as_ptr().unwrap(),
                        |parts| parts[1],
                    )
                    .unwrap()
                };
                assert_eq!(
                    unsafe { dict_get_in_place(py, obj_from_bits(slots).as_ptr().unwrap(), field) },
                    Some(MoltObject::from_int(2).bits())
                );
            } else {
                assert_eq!(state, MoltObject::none().bits());
            }
            assert_eq!(exception_pending(py), mode == 2);
            if mode == 2 {
                crate::molt_exception_clear();
            }
            dec_ref_bits(py, state);
        }
        for bits in [
            object,
            child,
            child_name,
            base,
            getter,
            getter_name,
            slots,
            slots_name,
            field,
        ] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}
