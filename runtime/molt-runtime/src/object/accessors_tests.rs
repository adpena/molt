use super::*;
use crate::attr_name_bits_from_bytes;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use crate::object::builders::alloc_class_instance;
use std::sync::atomic::{AtomicU64, Ordering};

struct FieldAbi {
    pointer: bool,
    get: unsafe extern "C" fn(u64, u64) -> u64,
    set: unsafe extern "C" fn(u64, u64, u64) -> u64,
    init: unsafe extern "C" fn(u64, u64, u64) -> u64,
}

impl FieldAbi {
    fn handle(&self, bits: u64) -> u64 {
        if self.pointer {
            crate::provenance::abi::expose_address(obj_from_bits(bits).as_ptr().unwrap())
        } else {
            bits
        }
    }
}

const FIELD_ABIS: [FieldAbi; 2] = [
    FieldAbi {
        pointer: false,
        get: molt_object_field_get,
        set: molt_object_field_set,
        init: molt_object_field_init,
    },
    FieldAbi {
        pointer: true,
        get: molt_object_field_get_ptr,
        set: molt_object_field_set_ptr,
        init: molt_object_field_init_ptr,
    },
];

fn field_object(_py: &PyToken<'_>) -> (u64, u64) {
    let name = attr_name_bits_from_bytes(_py, b"FieldOwner").unwrap();
    let field = attr_name_bits_from_bytes(_py, b"field").unwrap();
    let class = crate::molt_class_new(name);
    assert!(obj_from_bits(class).as_ptr().is_some());
    let offsets_key = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__").unwrap();
    let size_key = attr_name_bits_from_bytes(_py, b"__molt_layout_size__").unwrap();
    let offsets = crate::alloc_dict_with_pairs(_py, &[field, MoltObject::from_int(0).bits()]);
    assert!(!offsets.is_null());
    let offsets_bits = MoltObject::from_ptr(offsets).bits();
    crate::molt_set_attr_name(class, offsets_key, offsets_bits);
    crate::molt_set_attr_name(class, size_key, MoltObject::from_int(16).bits());
    assert!(!exception_pending(_py));
    unsafe { crate::object::class_finish_definition(_py, obj_from_bits(class).as_ptr().unwrap()) }
        .expect("seal field fixture");
    let object = alloc_class_instance(_py, 16, class);
    let ptr = obj_from_bits(object).as_ptr().unwrap();
    unsafe { crate::object::gc::gc_publish_initialized(_py, ptr) };
    for bits in [name, class, offsets_key, size_key, offsets_bits] {
        dec_ref_bits(_py, bits);
    }
    (object, field)
}

fn materialize_dict(_py: &PyToken<'_>, object: u64) -> u64 {
    let key = attr_name_bits_from_bytes(_py, b"__dict__").unwrap();
    let dict = crate::molt_get_attr_name(object, key);
    dec_ref_bits(_py, key);
    assert!(obj_from_bits(dict).as_ptr().is_some());
    assert!(!exception_pending(_py));
    dict
}

fn immutable_field_object(_py: &PyToken<'_>) -> (u64, u64) {
    let result = field_object(_py);
    let class = unsafe { object_class_bits(obj_from_bits(result.0).as_ptr().unwrap()) };
    assert!(unsafe {
        crate::object::class_set_immutable(_py, obj_from_bits(class).as_ptr().unwrap())
    });
    result
}

fn refcount(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().unwrap();
    unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

#[test]
fn empty_fields_and_both_float_zeros_remain_distinct_across_backing_transitions() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for abi in &FIELD_ABIS {
            for zero in [0.0f64, -0.0f64] {
                let (object, field) = field_object(_py);
                let ptr = obj_from_bits(object).as_ptr().unwrap();
                let handle = abi.handle(object);
                let value = MoltObject::from_float(zero).bits();
                unsafe {
                    assert!(is_missing_bits(_py, *ptr.cast::<u64>()));
                    assert!(
                        !(*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_HAS_PTRS)
                    );
                    assert!(!object_field_delete_ptr_raw(_py, ptr, 0));
                    let class = object_class_bits(ptr);
                    let mut referents = Vec::new();
                    crate::object::heap_lifecycle::visit_owned_values(_py, ptr, &mut |bits| {
                        referents.push(bits)
                    });
                    assert_eq!(
                        referents,
                        vec![class],
                        "empty metadata is not a Python referent"
                    );
                    crate::molt_set_attr_name(class, field, MoltObject::from_int(7).bits());
                    assert_eq!((abi.get)(handle, 0), MoltObject::from_int(7).bits());
                    (abi.init)(handle, 0, value);
                    assert_eq!(
                        (abi.get)(handle, 0),
                        value,
                        "inline float preserves its sign bit"
                    );
                    referents.clear();
                    crate::object::heap_lifecycle::visit_owned_values(_py, ptr, &mut |bits| {
                        referents.push(bits)
                    });
                    assert_eq!(
                        referents,
                        vec![class, value],
                        "float zero is a real stored referent"
                    );
                    assert!(
                        !(*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_HAS_PTRS)
                    );
                    let dict = materialize_dict(_py, object);
                    let dict_ptr = obj_from_bits(dict).as_ptr().unwrap();
                    assert_eq!(dict_get_in_place(_py, dict_ptr, field), Some(value));
                    assert_eq!((abi.get)(handle, 0), value);
                    assert!(is_missing_bits(_py, *ptr.cast::<u64>()));
                    assert!(object_field_delete_ptr_raw(_py, ptr, 0));
                    assert!(!object_field_delete_ptr_raw(_py, ptr, 0));
                    assert_eq!((abi.get)(handle, 0), MoltObject::from_int(7).bits());
                    field_storage::replace_dictionary(_py, ptr, None);
                    (abi.set)(handle, 0, value);
                    assert_eq!((abi.get)(handle, 0), value);
                    assert!(object_field_delete_ptr_raw(_py, ptr, 0));
                    let empty = materialize_dict(_py, object);
                    assert_eq!(
                        dict_get_in_place(_py, obj_from_bits(empty).as_ptr().unwrap(), field),
                        None
                    );
                    for bits in [dict, empty] {
                        dec_ref_bits(_py, bits);
                    }
                }
                for bits in [object, field] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!exception_pending(_py));
            }
        }
    });
}

#[test]
fn stack_constructor_shares_empty_field_and_payload_extent_authority() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let (heap, field) = immutable_field_object(_py);
        let class = unsafe { object_class_bits(obj_from_bits(heap).as_ptr().unwrap()) };
        let class_owners = refcount(class);
        let mut storage = [0u64; (std::mem::size_of::<crate::MoltHeader>() + 16) / 8];
        let stack = unsafe {
            crate::object::molt_object_init_stack(storage.as_mut_ptr().cast(), class, 16)
        };
        assert_eq!(refcount(class), class_owners + 1, "frame owns its class");
        let ptr = obj_from_bits(stack).as_ptr().unwrap();
        unsafe {
            assert!((*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_SCOPED));
            assert!(crate::object::gc::gc_is_tracked(ptr));
            assert_eq!(object_payload_size(ptr), 16);
            assert!(is_missing_bits(_py, *ptr.cast::<u64>()));
            assert_eq!(instance_dict_bits(ptr), 0);
            assert!(!(*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_HAS_PTRS));
            molt_object_field_init(stack, 0, MoltObject::from_float(0.0).bits());
            assert_eq!(molt_object_field_get(stack, 0), 0);
            assert!(object_field_delete_ptr_raw(_py, ptr, 0));
            assert!(is_missing_bits(_py, *ptr.cast::<u64>()));
        }
        inc_ref_bits(_py, stack);
        dec_ref_bits(_py, stack);
        assert_eq!(
            refcount(class),
            class_owners + 1,
            "nonterminal alias drop retains class"
        );
        dec_ref_bits(_py, stack);
        assert!(!unsafe { crate::object::gc::gc_is_tracked(ptr) });
        assert_eq!(
            refcount(class),
            class_owners,
            "terminal frame drop releases class"
        );
        assert_eq!(
            unsafe { object_class_bits(ptr) },
            0,
            "frame storage remains valid but empty"
        );
        for bits in [heap, field] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn frame_storage_collects_inline_and_dictionary_cycles_before_scope_exit() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::with_gil_entry_nopanic!(_py, {
        for dictionary_backed in [false, true] {
            let (heap, field) = immutable_field_object(_py);
            let class = unsafe { object_class_bits(obj_from_bits(heap).as_ptr().unwrap()) };
            let class_owners = refcount(class);
            let mut storage = [0u64; (std::mem::size_of::<crate::MoltHeader>() + 16) / 8];
            let stack = unsafe {
                crate::object::molt_object_init_stack(storage.as_mut_ptr().cast(), class, 16)
            };
            let ptr = obj_from_bits(stack).as_ptr().unwrap();
            unsafe {
                molt_object_field_set(stack, 0, stack);
                if dictionary_backed {
                    let dictionary = materialize_dict(_py, stack);
                    dec_ref_bits(_py, dictionary);
                    // __dict__ attribute resolution caches its negative
                    // descriptor lookup with an owned class reference. Remove
                    // that independent root before measuring the scoped edge.
                    crate::builtins::attr::clear_attr_tls_caches(_py);
                }
                assert_eq!(refcount(class), class_owners + 1, "one scoped class owner");
                dec_ref_bits(_py, stack);
                assert!(crate::object::gc::gc_is_tracked(ptr));
                crate::object::gc::collect_cycles(_py);
                assert!(!crate::object::gc::gc_is_tracked(ptr));
                assert_eq!((*header_from_obj_ptr(ptr)).ref_count_snapshot(), 0);
                assert_eq!(object_class_bits(ptr), 0);
            }
            assert_eq!(
                refcount(class),
                class_owners,
                "collection releases scoped class edge"
            );
            for bits in [heap, field] {
                dec_ref_bits(_py, bits);
            }
            assert!(!exception_pending(_py));
        }
    });
}

#[test]
fn frame_candidate_with_different_sealed_extent_returns_owned_heap_storage() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let (heap, field) = immutable_field_object(_py);
        let class = unsafe { object_class_bits(obj_from_bits(heap).as_ptr().unwrap()) };
        let class_owners = refcount(class);
        let mut storage = [u64::MAX; (std::mem::size_of::<crate::MoltHeader>() + 8) / 8];
        let result =
            unsafe { crate::object::molt_object_init_stack(storage.as_mut_ptr().cast(), class, 8) };
        let ptr = obj_from_bits(result).as_ptr().unwrap();
        unsafe {
            assert!(!(*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_SCOPED));
            assert!(is_missing_bits(_py, *ptr.cast::<u64>()));
        }
        assert!(
            storage.iter().all(|&word| word == u64::MAX),
            "heap realization does not initialize frame storage"
        );
        assert_eq!(refcount(class), class_owners + 1);
        dec_ref_bits(_py, result);
        assert_eq!(refcount(class), class_owners);
        for bits in [heap, field] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

static RESURRECTED_FRAME_CANDIDATE: AtomicU64 = AtomicU64::new(0);

extern "C" fn resurrect_frame_candidate(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, bits);
        RESURRECTED_FRAME_CANDIDATE.store(bits, Ordering::SeqCst);
        MoltObject::none().bits()
    })
}

#[test]
fn mutable_frame_candidate_survives_late_finalizer_resurrection_on_heap() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        RESURRECTED_FRAME_CANDIDATE.store(0, Ordering::SeqCst);
        let (heap, field) = field_object(_py);
        let class = unsafe { object_class_bits(obj_from_bits(heap).as_ptr().unwrap()) };
        inc_ref_bits(_py, class);
        dec_ref_bits(_py, heap);
        let mut storage = [u64::MAX; (std::mem::size_of::<crate::MoltHeader>() + 16) / 8];
        let result = unsafe {
            crate::object::molt_object_init_stack(storage.as_mut_ptr().cast(), class, 16)
        };
        let ptr = obj_from_bits(result).as_ptr().unwrap();
        unsafe {
            assert!(!(*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_SCOPED));
            molt_object_field_set(result, 0, MoltObject::from_int(73).bits());
        }
        assert!(storage.iter().all(|&word| word == u64::MAX));
        let name = attr_name_bits_from_bytes(_py, b"__del__").unwrap();
        let finalizer = callable(_py, "resurrect_frame_candidate", resurrect_frame_candidate);
        crate::molt_set_attr_name(class, name, finalizer);
        assert!(!exception_pending(_py));
        dec_ref_bits(_py, result);
        let resurrected = RESURRECTED_FRAME_CANDIDATE.swap(0, Ordering::SeqCst);
        assert_eq!(
            resurrected, result,
            "late finalizer retains the actual receiver"
        );
        assert_eq!(
            unsafe { molt_object_field_get(resurrected, 0) },
            MoltObject::from_int(73).bits()
        );
        dec_ref_bits(_py, resurrected);
        assert_eq!(RESURRECTED_FRAME_CANDIDATE.load(Ordering::SeqCst), 0);
        for bits in [class, field, name, finalizer] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn immutable_finalizing_class_still_requires_heap_storage() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let (heap, field) = field_object(_py);
        let class = unsafe { object_class_bits(obj_from_bits(heap).as_ptr().unwrap()) };
        let name = attr_name_bits_from_bytes(_py, b"__del__").unwrap();
        let finalizer = callable(_py, "frame_finalizer_payload", callable_payload);
        crate::molt_set_attr_name(class, name, finalizer);
        assert!(!exception_pending(_py));
        assert!(unsafe {
            crate::object::class_set_immutable(_py, obj_from_bits(class).as_ptr().unwrap())
        });
        let mut storage = [u64::MAX; (std::mem::size_of::<crate::MoltHeader>() + 16) / 8];
        let result = unsafe {
            crate::object::molt_object_init_stack(storage.as_mut_ptr().cast(), class, 16)
        };
        let ptr = obj_from_bits(result).as_ptr().unwrap();
        assert!(!unsafe {
            (*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_SCOPED)
        });
        assert!(storage.iter().all(|&word| word == u64::MAX));
        for bits in [result, heap, field, name, finalizer] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn immutable_frame_candidate_with_mutable_base_remains_heap_owned() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let (parent_object, field) = field_object(_py);
        let parent = unsafe { object_class_bits(obj_from_bits(parent_object).as_ptr().unwrap()) };
        let name = attr_name_bits_from_bytes(_py, b"ImmutableChild").unwrap();
        let class = crate::molt_class_new(name);
        crate::molt_class_set_base(class, parent);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        unsafe {
            crate::object::class_finish_definition(_py, class_ptr).expect("seal child layout");
            assert_eq!(
                crate::object::layout::class_cached_layout_size(class_ptr),
                Some(16)
            );
            assert!(crate::object::class_set_immutable(_py, class_ptr));
        }
        let mut storage = [u64::MAX; (std::mem::size_of::<crate::MoltHeader>() + 16) / 8];
        let result = unsafe {
            crate::object::molt_object_init_stack(storage.as_mut_ptr().cast(), class, 16)
        };
        let ptr = obj_from_bits(result).as_ptr().unwrap();
        assert!(!unsafe {
            (*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_SCOPED)
        });
        assert!(storage.iter().all(|&word| word == u64::MAX));
        for bits in [result, class, name, parent_object, field] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn scoped_class_replacement_preserves_lifetime_admission_atomically() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let (heap, field) = immutable_field_object(_py);
        let (mutable_heap, mutable_field) = field_object(_py);
        let class = unsafe { object_class_bits(obj_from_bits(heap).as_ptr().unwrap()) };
        let mutable_class =
            unsafe { object_class_bits(obj_from_bits(mutable_heap).as_ptr().unwrap()) };
        let mut storage = [0u64; (std::mem::size_of::<crate::MoltHeader>() + 16) / 8];
        let result = unsafe {
            crate::object::molt_object_init_stack(storage.as_mut_ptr().cast(), class, 16)
        };
        let ptr = obj_from_bits(result).as_ptr().unwrap();
        let class_owners = refcount(class);
        let mutable_owners = refcount(mutable_class);
        unsafe {
            assert!((*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_SCOPED));
            crate::molt_object_set_class(
                crate::provenance::abi::expose_address(ptr),
                mutable_class,
            );
            assert!(exception_pending(_py));
            assert_eq!(object_class_bits(ptr), class);
            assert_eq!(refcount(class), class_owners);
            assert_eq!(refcount(mutable_class), mutable_owners);
        }
        crate::molt_exception_clear();
        for bits in [result, heap, field, mutable_heap, mutable_field] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn field_abis_share_initialization_read_and_replacement_ownership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for abi in &FIELD_ABIS {
            let (object, field) = field_object(_py);
            let value = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
            let before = refcount(value);
            let handle = abi.handle(object);
            unsafe {
                assert_eq!((abi.init)(handle, 0, value), MoltObject::none().bits());
                assert_eq!(
                    refcount(value),
                    before + 1,
                    "field initialization owns its value"
                );
                let read = (abi.get)(handle, 0);
                assert_eq!(read, value);
                assert_eq!(
                    refcount(value),
                    before + 2,
                    "field read returns an owned reference"
                );
                dec_ref_bits(_py, read);
                (abi.set)(handle, 0, value);
                assert_eq!(
                    refcount(value),
                    before + 1,
                    "self assignment preserves ownership"
                );
                (abi.set)(handle, 0, MoltObject::none().bits());
                assert_eq!(
                    refcount(value),
                    before,
                    "replacement releases exactly one owner"
                );
            }
            for bits in [object, field, value] {
                dec_ref_bits(_py, bits);
            }
            assert!(!exception_pending(_py));
        }
    });
}

#[test]
fn field_abis_share_dictionary_storage_without_an_inline_mirror() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for abi in &FIELD_ABIS {
            let (object, field) = field_object(_py);
            let value = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
            let handle = abi.handle(object);
            unsafe { (abi.init)(handle, 0, MoltObject::from_int(1).bits()) };
            let dict = materialize_dict(_py, object);
            let dict_ptr = obj_from_bits(dict).as_ptr().unwrap();
            unsafe {
                assert!(is_missing_bits(
                    _py,
                    *obj_from_bits(object).as_ptr().unwrap().cast::<u64>()
                ));
                (abi.set)(handle, 0, value);
                assert_eq!(dict_get_in_place(_py, dict_ptr, field), Some(value));
                // Python can edit __dict__ independently of an inline slot.
                dict_set_in_place(_py, dict_ptr, field, MoltObject::from_int(9).bits());
                assert_eq!((abi.get)(handle, 0), MoltObject::from_int(9).bits());
                assert_eq!(
                    refcount(value),
                    1,
                    "no hidden inline owner survives dictionary replacement"
                );
                (abi.set)(handle, 0, value);
                assert_eq!(dict_get_in_place(_py, dict_ptr, field), Some(value));
                (abi.set)(handle, 0, MoltObject::none().bits());
                assert_eq!(
                    dict_get_in_place(_py, dict_ptr, field),
                    Some(MoltObject::none().bits())
                );
                assert!(is_missing_bits(
                    _py,
                    *obj_from_bits(object).as_ptr().unwrap().cast::<u64>()
                ));
            }
            for bits in [dict, object, field, value] {
                dec_ref_bits(_py, bits);
            }
            assert!(!exception_pending(_py));
        }
    });
}

#[test]
fn classless_payload_tail_is_a_field_not_an_instance_dictionary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let object = crate::molt_alloc(16);
        let value = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
        unsafe {
            molt_object_field_init(object, 8, MoltObject::from_int(7).bits());
            molt_object_field_init(object, 0, value);
            let read = molt_object_field_get(object, 0);
            assert_eq!(read, value);
            dec_ref_bits(_py, read);
            assert_eq!(
                molt_object_field_get(object, 8),
                MoltObject::from_int(7).bits()
            );
            assert_eq!(
                crate::object::object_shape_id(obj_from_bits(object).as_ptr().unwrap()),
                crate::object::ObjectShapeId::BoxedFields
            );
        }
        dec_ref_bits(_py, object);
        assert_eq!(
            refcount(value),
            1,
            "boxed payload destruction releases its field owner"
        );
        for bits in [value] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn task_capture_layout_excludes_nonowning_field_fast_paths_before_payload_writes() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let task = crate::molt_task_new(0, 16, crate::TASK_KIND_FUTURE);
        let ptr = obj_from_bits(task).as_ptr().unwrap();
        assert!(unsafe {
            (*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_HAS_PTRS)
        });
        dec_ref_bits(_py, task);
        assert!(!exception_pending(_py));
    });
}

#[test]
fn dictionary_materialization_moves_one_owner_and_preserves_none_and_deletions() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for abi in &FIELD_ABIS {
            let (object, field) = field_object(_py);
            let ptr = obj_from_bits(object).as_ptr().unwrap();
            let value = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
            let handle = abi.handle(object);
            unsafe { (abi.init)(handle, 0, value) };
            assert_eq!(refcount(value), 2);
            let dict = materialize_dict(_py, object);
            let dict_ptr = obj_from_bits(dict).as_ptr().unwrap();
            assert_eq!(
                refcount(value),
                2,
                "materialization moves rather than copies ownership"
            );
            assert!(is_missing_bits(_py, unsafe { *ptr.cast::<u64>() }));
            let again = materialize_dict(_py, object);
            assert_eq!(again, dict);
            assert_eq!(refcount(value), 2);
            dec_ref_bits(_py, again);
            unsafe {
                assert!(crate::dict_del_in_place(_py, dict_ptr, field));
                assert_eq!(refcount(value), 1);
                let absent = object_field_get_ptr_raw(_py, ptr, 0);
                assert!(is_missing_bits(_py, absent));
                dec_ref_bits(_py, absent);
                let again = materialize_dict(_py, object);
                assert_eq!(dict_get_in_place(_py, dict_ptr, field), None);
                dec_ref_bits(_py, again);
                (abi.set)(handle, 0, MoltObject::none().bits());
                assert_eq!(
                    dict_get_in_place(_py, dict_ptr, field),
                    Some(MoltObject::none().bits())
                );
                assert_eq!((abi.get)(handle, 0), MoltObject::none().bits());
                assert!(object_field_delete_ptr_raw(_py, ptr, 0));
                assert!(!object_field_delete_ptr_raw(_py, ptr, 0));
            }
            for bits in [dict, object, field, value] {
                dec_ref_bits(_py, bits);
            }
            assert!(!exception_pending(_py));
        }
    });
}

#[test]
fn replacing_and_resetting_dictionary_never_reacquires_retired_inline_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let (object, field) = field_object(_py);
        let ptr = obj_from_bits(object).as_ptr().unwrap();
        let old = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
        let replacement = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
            _py,
            &[field, MoltObject::from_int(8).bits()],
        ))
        .bits();
        unsafe {
            molt_object_field_init(object, 0, old);
            field_storage::replace_dictionary(_py, ptr, Some(replacement));
            assert_eq!(refcount(old), 1);
            assert!(is_missing_bits(_py, *ptr.cast::<u64>()));
            assert_eq!(
                molt_object_field_get(object, 0),
                MoltObject::from_int(8).bits()
            );
            field_storage::replace_dictionary(_py, ptr, None);
            assert_eq!(instance_dict_bits(ptr), 0);
            assert_eq!(
                dict_get_in_place(_py, obj_from_bits(replacement).as_ptr().unwrap(), field),
                Some(MoltObject::from_int(8).bits())
            );
            let absent = object_field_get_ptr_raw(_py, ptr, 0);
            assert!(is_missing_bits(_py, absent));
            dec_ref_bits(_py, absent);
            let reset = materialize_dict(_py, object);
            assert_eq!(
                dict_get_in_place(_py, obj_from_bits(reset).as_ptr().unwrap(), field),
                None
            );
            dec_ref_bits(_py, reset);
        }
        for bits in [object, field, old, replacement] {
            dec_ref_bits(_py, bits);
        }
        assert!(!exception_pending(_py));
    });
}

static CALLBACK_OBJECT: AtomicU64 = AtomicU64::new(0);
static CALLBACK_FIELD: AtomicU64 = AtomicU64::new(0);
static CALLBACK_SEEN_SLOT: AtomicU64 = AtomicU64::new(0);
static CALLBACK_SEEN_DICT: AtomicU64 = AtomicU64::new(0);

extern "C" fn callable_payload(_value: u64) -> u64 {
    MoltObject::none().bits()
}

extern "C" fn observe_and_replace_field(_weak: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let object = CALLBACK_OBJECT.load(Ordering::SeqCst);
        let field = CALLBACK_FIELD.load(Ordering::SeqCst);
        unsafe {
            let read = molt_object_field_get(object, 0);
            CALLBACK_SEEN_SLOT.store(read, Ordering::SeqCst);
            dec_ref_bits(_py, read);
            let ptr = obj_from_bits(object).as_ptr().unwrap();
            let dict = obj_from_bits(instance_dict_bits(ptr)).as_ptr().unwrap();
            CALLBACK_SEEN_DICT.store(
                dict_get_in_place(_py, dict, field).unwrap(),
                Ordering::SeqCst,
            );
            molt_object_field_set(object, 0, MoltObject::from_int(77).bits())
        }
    })
}

fn callable(_py: &PyToken<'_>, name: &str, target: extern "C" fn(u64) -> u64) -> u64 {
    let ptr = alloc_runtime_function_obj(_py, runtime_fn_addr(name, target as *const ()), 1);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

#[test]
fn field_abis_publish_before_release_and_preserve_reentrant_replacement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for abi in &FIELD_ABIS {
            let (object, field) = field_object(_py);
            let old = callable(_py, "field_callable_payload", callable_payload);
            let callback = callable(_py, "observe_and_replace_field", observe_and_replace_field);
            let weak_type = crate::molt_weakref_reference_type();
            let weak = crate::molt_weakref_new(weak_type, old, callback);
            dec_ref_bits(_py, weak_type);
            assert!(obj_from_bits(weak).as_ptr().is_some());
            let incoming = MoltObject::from_ptr(crate::alloc_list(_py, &[])).bits();
            let handle = abi.handle(object);
            unsafe { (abi.init)(handle, 0, old) };
            let dict = materialize_dict(_py, object);
            dec_ref_bits(_py, old);
            CALLBACK_OBJECT.store(object, Ordering::SeqCst);
            CALLBACK_FIELD.store(field, Ordering::SeqCst);
            CALLBACK_SEEN_SLOT.store(0, Ordering::SeqCst);
            CALLBACK_SEEN_DICT.store(0, Ordering::SeqCst);
            unsafe { (abi.set)(handle, 0, incoming) };
            assert_eq!(CALLBACK_SEEN_SLOT.load(Ordering::SeqCst), incoming);
            assert_eq!(CALLBACK_SEEN_DICT.load(Ordering::SeqCst), incoming);
            assert_eq!(
                unsafe { (abi.get)(handle, 0) },
                MoltObject::from_int(77).bits()
            );
            assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
            CALLBACK_OBJECT.store(0, Ordering::SeqCst);
            CALLBACK_FIELD.store(0, Ordering::SeqCst);
            for bits in [dict, object, field, incoming, weak, callback] {
                dec_ref_bits(_py, bits);
            }
            assert!(!exception_pending(_py));
        }
    });
}

#[test]
fn null_receiver_is_a_layout_guard_miss_without_header_access() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        assert!(!unsafe {
            guard_layout_match(
                _py,
                std::ptr::null_mut(),
                MoltObject::none().bits(),
                MoltObject::from_int(0).bits(),
            )
        });
        assert!(!exception_pending(_py));
    });
}

fn assert_and_clear_attribute_error(_py: &PyToken<'_>) {
    assert!(exception_pending(_py));
    assert_eq!(crate::builtins::exceptions::molt_exception_pending(), 1);
    let error = crate::builtins::exceptions::molt_exception_last_pending();
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        _py,
        error,
        "AttributeError"
    ));
    crate::clear_exception(_py);
    dec_ref_bits(_py, error);
    assert_eq!(crate::builtins::exceptions::molt_exception_pending(), 0);
}

#[test]
fn tagged_scalar_receivers_miss_layout_and_preserve_generic_attribute_dispatch() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = crate::builtin_classes(_py);
        let version = MoltObject::from_int(0).bits();
        let class_attr = b"__class__";

        let missing_attr = b"missing";
        for (name, scalar, class) in [
            ("int", MoltObject::from_int(3).bits(), builtins.int),
            ("bool", MoltObject::from_bool(true).bits(), builtins.bool),
            ("float", MoltObject::from_float(1.5).bits(), builtins.float),
            ("+0.0", MoltObject::from_float(0.0).bits(), builtins.float),
            ("-0.0", MoltObject::from_float(-0.0).bits(), builtins.float),
            ("None", MoltObject::none().bits(), builtins.none_type),
        ] {
            assert_eq!(
                unsafe { molt_guard_layout(scalar, class, version) },
                MoltObject::from_bool(false).bits(),
                "{name} layout guard"
            );
            assert!(!exception_pending(_py), "{name} layout guard");

            let generic_class = unsafe {
                crate::molt_get_attr_object(scalar, class_attr.as_ptr(), class_attr.len() as u64)
            };
            assert_eq!(generic_class, class, "{name} generic class lookup");
            assert!(!exception_pending(_py));
            let guarded_class = unsafe {
                molt_guarded_field_get(
                    scalar,
                    class,
                    version,
                    0,
                    crate::provenance::abi::expose_address(class_attr.as_ptr()),
                    class_attr.len() as u64,
                )
            };
            assert_eq!(guarded_class, generic_class, "{name} guarded class lookup");
            assert!(!exception_pending(_py));
            dec_ref_bits(_py, generic_class);
            dec_ref_bits(_py, guarded_class);

            // These are value-returning intrinsics, not C status/pointer APIs.
            // The generated exception predicate distinguishes failure from a
            // successful None result; raw zero instead represents float +0.0.
            let generic_get = unsafe {
                crate::molt_get_attr_object(
                    scalar,
                    missing_attr.as_ptr(),
                    missing_attr.len() as u64,
                )
            };
            assert_eq!(
                generic_get,
                MoltObject::none().bits(),
                "{name} generic get error value"
            );
            assert_and_clear_attribute_error(_py);
            let result = unsafe {
                molt_guarded_field_get(
                    scalar,
                    class,
                    version,
                    0,
                    crate::provenance::abi::expose_address(missing_attr.as_ptr()),
                    missing_attr.len() as u64,
                )
            };
            assert_eq!(result, generic_get, "{name} guarded get error value");
            assert_and_clear_attribute_error(_py);

            let generic_set = unsafe {
                crate::molt_set_attr_object(
                    scalar,
                    missing_attr.as_ptr(),
                    missing_attr.len() as u64,
                    MoltObject::from_int(7).bits(),
                )
            };
            assert_eq!(
                generic_set,
                MoltObject::none().bits(),
                "{name} generic set error value"
            );
            assert_and_clear_attribute_error(_py);
            let result = unsafe {
                molt_guarded_field_set(
                    scalar,
                    class,
                    version,
                    0,
                    MoltObject::from_int(7).bits(),
                    crate::provenance::abi::expose_address(missing_attr.as_ptr()),
                    missing_attr.len() as u64,
                )
            };
            assert_eq!(result, generic_set, "{name} guarded set error value");
            assert_and_clear_attribute_error(_py);
        }
        assert!(!exception_pending(_py));
    });
}

#[test]
fn guard_layout_accepts_exact_builtin_dict_with_bootstrap_version() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = crate::builtin_classes(_py);
        let dict_ptr = crate::alloc_dict_with_pairs(_py, &[]);
        assert!(!dict_ptr.is_null());
        let dict_class_ptr = obj_from_bits(builtins.dict).as_ptr().unwrap();
        let current_version = unsafe { class_layout_version_bits(dict_class_ptr) };
        assert!(current_version > 0);
        assert!(unsafe {
            guard_layout_match(
                _py,
                dict_ptr,
                builtins.dict,
                MoltObject::from_int(current_version as i64).bits(),
            )
        });
        assert!(!unsafe {
            guard_layout_match(_py, dict_ptr, builtins.dict, MoltObject::from_int(0).bits())
        });
        dec_ref_bits(_py, MoltObject::from_ptr(dict_ptr).bits());
    });
}
