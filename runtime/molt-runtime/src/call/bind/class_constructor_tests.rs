use super::*;
use crate::object::seq_access::snapshot;
use std::cell::RefCell;

#[derive(Default)]
struct CallbackState {
    events: Vec<String>,
    prepared: u64,
    class_cell: u64,
    namespace_cell: u64,
}
thread_local! {
    static CALLBACKS: RefCell<CallbackState> = RefCell::new(CallbackState::default());
}

unsafe fn call_type(_py: &PyToken<'_>, name: u64, bases: u64, namespace: u64) -> u64 {
    unsafe {
        let args = molt_callargs_new(3, 0);
        assert!(obj_from_bits(args).as_ptr().is_some());
        molt_callargs_push_pos(args, name);
        molt_callargs_push_pos(args, bases);
        molt_callargs_push_pos(args, namespace);
        molt_call_bind(builtin_classes(_py).type_obj, args)
    }
}

unsafe fn empty_type(_py: &PyToken<'_>, name: &[u8], meta: u64, bases: &[u64]) -> u64 {
    unsafe {
        let name_ptr = alloc_string(_py, name);
        let ns = alloc_dict_with_pairs(_py, &[]);
        let base_tuple = alloc_tuple(_py, bases);
        assert!(!name_ptr.is_null() && !ns.is_null() && !base_tuple.is_null());
        let result = molt_type_new(
            meta,
            MoltObject::from_ptr(name_ptr).bits(),
            MoltObject::from_ptr(base_tuple).bits(),
            MoltObject::from_ptr(ns).bits(),
            MoltObject::none().bits(),
        );
        for ptr in [name_ptr, ns, base_tuple] {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
        }
        assert!(!exception_pending(_py));
        assert_eq!(
            crate::object_type_id(obj_from_bits(result).as_ptr().unwrap()),
            TYPE_ID_TYPE
        );
        result
    }
}

#[test]
fn type_call_dispatch_publishes_cells_for_direct_and_metaclass_winner_paths() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            for winner in [false, true] {
                let type_bits = builtin_classes(_py).type_obj;
                let meta = if winner {
                    empty_type(_py, b"CellMeta", type_bits, &[type_bits])
                } else {
                    type_bits
                };
                let base = empty_type(_py, b"CellBase", meta, &[]);
                let name = crate::attr_name_bits_from_bytes(_py, b"CellSubject").unwrap();
                let class_key = crate::attr_name_bits_from_bytes(_py, b"__classcell__").unwrap();
                let dict_key = crate::attr_name_bits_from_bytes(_py, b"__classdictcell__").unwrap();
                let value_key = crate::attr_name_bits_from_bytes(_py, b"value").unwrap();
                let prepared =
                    alloc_dict_with_pairs(_py, &[value_key, MoltObject::from_int(1).bits()]);
                let prepared_bits = MoltObject::from_ptr(prepared).bits();
                let class_cell = crate::alloc_list(_py, &[MoltObject::none().bits()]);
                let dict_cell = crate::alloc_list(_py, &[prepared_bits]);
                let class_cell_bits = MoltObject::from_ptr(class_cell).bits();
                let dict_cell_bits = MoltObject::from_ptr(dict_cell).bits();
                crate::dict_set_in_place(_py, prepared, class_key, class_cell_bits);
                crate::dict_set_in_place(_py, prepared, dict_key, dict_cell_bits);
                let bases = alloc_tuple(_py, &[base]);
                let bases_bits = MoltObject::from_ptr(bases).bits();
                let class_bits = call_type(_py, name, bases_bits, prepared_bits);
                assert!(!exception_pending(_py));
                let class = obj_from_bits(class_bits).as_ptr().unwrap();
                assert_eq!(crate::object_class_bits(class), meta);
                let copied_bits = crate::class_dict_bits(class);
                let copied = obj_from_bits(copied_bits).as_ptr().unwrap();
                assert_ne!(copied_bits, prepared_bits);
                assert_eq!(
                    &*snapshot(_py, class_cell, "class cell").unwrap(),
                    &[class_bits]
                );
                assert_eq!(
                    &*snapshot(_py, dict_cell, "dict cell").unwrap(),
                    &[copied_bits]
                );
                for key in [class_key, dict_key] {
                    assert_eq!(dict_get_in_place(_py, copied, key), None);
                    assert!(
                        dict_get_in_place(_py, prepared, key).is_some(),
                        "type construction must not mutate the supplied namespace"
                    );
                }
                crate::dict_set_in_place(_py, prepared, value_key, MoltObject::from_int(2).bits());
                assert_eq!(
                    dict_get_in_place(_py, copied, value_key),
                    Some(MoltObject::from_int(1).bits())
                );
                crate::dict_set_in_place(_py, copied, value_key, MoltObject::from_int(3).bits());
                assert_eq!(
                    dict_get_in_place(_py, copied, value_key),
                    Some(MoltObject::from_int(3).bits())
                );
                // Drop the caller's class reference first: the class cell still owns it.
                dec_ref_bits(_py, class_bits);
                assert_eq!(crate::object_type_id(class), TYPE_ID_TYPE);
                for bits in [
                    prepared_bits,
                    class_cell_bits,
                    dict_cell_bits,
                    bases_bits,
                    name,
                    class_key,
                    dict_key,
                    value_key,
                    base,
                ] {
                    dec_ref_bits(_py, bits);
                }
                if winner {
                    dec_ref_bits(_py, meta);
                }
                assert!(!exception_pending(_py));
            }
        }
    });
}

#[test]
fn type_call_invalid_cell_releases_copied_namespace_payload() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            for cell_name in [b"__classcell__".as_slice(), b"__classdictcell__".as_slice()] {
                let name = crate::attr_name_bits_from_bytes(_py, b"RejectedCell").unwrap();
                let key = crate::attr_name_bits_from_bytes(_py, cell_name).unwrap();
                let payload_key = crate::attr_name_bits_from_bytes(_py, b"payload").unwrap();
                let payload = crate::alloc_list(_py, &[]);
                let payload_bits = MoltObject::from_ptr(payload).bits();
                let ns = alloc_dict_with_pairs(
                    _py,
                    &[
                        key,
                        MoltObject::from_int(1).bits(),
                        payload_key,
                        payload_bits,
                    ],
                );
                let ns_bits = MoltObject::from_ptr(ns).bits();
                let before = (*crate::header_from_obj_ptr(payload)).ref_count_snapshot();
                let result = call_type(_py, name, MoltObject::none().bits(), ns_bits);
                assert!(obj_from_bits(result).is_none());
                assert!(exception_pending(_py));
                crate::molt_exception_clear();
                assert_eq!(
                    (*crate::header_from_obj_ptr(payload)).ref_count_snapshot(),
                    before
                );
                assert_eq!(
                    dict_get_in_place(_py, ns, key),
                    Some(MoltObject::from_int(1).bits())
                );
                for bits in [ns_bits, payload_bits, name, key, payload_key] {
                    dec_ref_bits(_py, bits);
                }
            }
        }
    });
}

#[test]
fn type_invalid_qualname_precedes_cell_validation_and_publication() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            for cell_name in [b"__classcell__".as_slice(), b"__classdictcell__".as_slice()] {
                for invalid_cell in [false, true] {
                    let name = crate::attr_name_bits_from_bytes(_py, b"InvalidQualname").unwrap();
                    let qualname = crate::attr_name_bits_from_bytes(_py, b"__qualname__").unwrap();
                    let cell_key = crate::attr_name_bits_from_bytes(_py, cell_name).unwrap();
                    let marker = MoltObject::from_int(7).bits();
                    let cell = crate::alloc_list(_py, &[marker]);
                    assert!(!cell.is_null());
                    let cell_bits = MoltObject::from_ptr(cell).bits();
                    let ns = alloc_dict_with_pairs(
                        _py,
                        &[
                            qualname,
                            marker,
                            cell_key,
                            if invalid_cell { marker } else { cell_bits },
                        ],
                    );
                    let bases = alloc_tuple(_py, &[]);
                    assert!(!ns.is_null() && !bases.is_null());
                    let ns_bits = MoltObject::from_ptr(ns).bits();
                    let bases_bits = MoltObject::from_ptr(bases).bits();
                    let result = call_type(_py, name, bases_bits, ns_bits);
                    assert!(obj_from_bits(result).is_none());
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        _py,
                        error,
                        "TypeError"
                    ));
                    let message = crate::format_exception_message(
                        _py,
                        obj_from_bits(error).as_ptr().unwrap(),
                    );
                    assert!(
                        message.contains("type __qualname__ must be a str, not int"),
                        "{message}"
                    );
                    crate::molt_exception_clear();
                    assert_eq!(
                        &*snapshot(_py, cell, "failed construction cell").unwrap(),
                        &[marker]
                    );
                    assert_eq!(dict_get_in_place(_py, ns, qualname), Some(marker));
                    for bits in [
                        result, error, ns_bits, bases_bits, cell_bits, name, qualname, cell_key,
                    ] {
                        dec_ref_bits(_py, bits);
                    }
                    assert!(!exception_pending(_py));
                }
            }
        }
    });
}

extern "C" fn record_set_name(_descriptor: u64, owner: u64, name: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let name_text = crate::string_obj_to_owned(obj_from_bits(name)).unwrap();
            let (prepared, class_cell, namespace_cell) = CALLBACKS.with(|state| {
                let mut state = state.borrow_mut();
                state.events.push(format!("set:{name_text}"));
                (state.prepared, state.class_cell, state.namespace_cell)
            });
            let owner_dict = crate::class_dict_bits(obj_from_bits(owner).as_ptr().unwrap());
            assert_eq!(
                &*snapshot(
                    _py,
                    obj_from_bits(class_cell).as_ptr().unwrap(),
                    "callback class cell"
                )
                .unwrap(),
                &[owner]
            );
            assert_eq!(
                &*snapshot(
                    _py,
                    obj_from_bits(namespace_cell).as_ptr().unwrap(),
                    "callback dict cell"
                )
                .unwrap(),
                &[owner_dict]
            );
            if name_text == "first" {
                let second = crate::attr_name_bits_from_bytes(_py, b"second").unwrap();
                crate::dict_del_in_place(_py, obj_from_bits(owner_dict).as_ptr().unwrap(), second);
                crate::dict_del_in_place(_py, obj_from_bits(prepared).as_ptr().unwrap(), second);
                dec_ref_bits(_py, second);
            }
            MoltObject::none().bits()
        }
    }) as i64
}

extern "C" fn record_left_hook(_owner: u64) -> i64 {
    CALLBACKS.with(|state| state.borrow_mut().events.push("init:left".to_string()));
    MoltObject::none().bits() as i64
}
extern "C" fn record_right_hook(_owner: u64) -> i64 {
    CALLBACKS.with(|state| state.borrow_mut().events.push("init:right".to_string()));
    MoltObject::none().bits() as i64
}

#[test]
fn type_callbacks_use_owned_snapshot_then_one_inherited_hook() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            CALLBACKS.with(|state| *state.borrow_mut() = CallbackState::default());
            let ty = builtin_classes(_py).type_obj;
            let left = empty_type(_py, b"Left", ty, &[]);
            let right = empty_type(_py, b"Right", ty, &[]);
            let descriptor_type = empty_type(_py, b"Descriptor", ty, &[]);
            let hook_key = crate::attr_name_bits_from_bytes(_py, b"__init_subclass__").unwrap();
            let set_key = crate::attr_name_bits_from_bytes(_py, b"__set_name__").unwrap();
            for (owner, function) in [
                (left, record_left_hook as *const ()),
                (right, record_right_hook as *const ()),
            ] {
                let ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(function),
                    1,
                );
                assert!(!ptr.is_null());
                let bits = MoltObject::from_ptr(ptr).bits();
                // type.__new__ installs a plain __init_subclass__ function as a
                // classmethod. This fixture mutates an already-created base
                // dictionary directly, so preserve that namespace contract
                // explicitly before exercising class-mode super dispatch.
                let hook = crate::molt_classmethod_new(bits);
                assert!(!obj_from_bits(hook).is_none());
                let dictionary = crate::class_dict_bits(obj_from_bits(owner).as_ptr().unwrap());
                crate::dict_set_in_place(
                    _py,
                    obj_from_bits(dictionary).as_ptr().unwrap(),
                    hook_key,
                    hook,
                );
                dec_ref_bits(_py, hook);
                dec_ref_bits(_py, bits);
            }
            let ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::provenance::abi::expose_function_address(record_set_name as *const ()),
                3,
            );
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();
            let dictionary =
                crate::class_dict_bits(obj_from_bits(descriptor_type).as_ptr().unwrap());
            crate::dict_set_in_place(
                _py,
                obj_from_bits(dictionary).as_ptr().unwrap(),
                set_key,
                bits,
            );
            dec_ref_bits(_py, bits);
            let first = crate::alloc_instance_for_class(
                _py,
                obj_from_bits(descriptor_type).as_ptr().unwrap(),
            );
            let second = crate::alloc_instance_for_class(
                _py,
                obj_from_bits(descriptor_type).as_ptr().unwrap(),
            );
            let first_key = crate::attr_name_bits_from_bytes(_py, b"first").unwrap();
            let second_key = crate::attr_name_bits_from_bytes(_py, b"second").unwrap();
            let ns = alloc_dict_with_pairs(_py, &[first_key, first, second_key, second]);
            let ns_bits = MoltObject::from_ptr(ns).bits();
            // The prepared mapping is now the only owner of each descriptor.
            dec_ref_bits(_py, first);
            dec_ref_bits(_py, second);
            let class_cell = crate::alloc_list(_py, &[MoltObject::none().bits()]);
            let namespace_cell = crate::alloc_list(_py, &[ns_bits]);
            let class_cell_bits = MoltObject::from_ptr(class_cell).bits();
            let namespace_cell_bits = MoltObject::from_ptr(namespace_cell).bits();
            let class_key = crate::attr_name_bits_from_bytes(_py, b"__classcell__").unwrap();
            let dict_key = crate::attr_name_bits_from_bytes(_py, b"__classdictcell__").unwrap();
            crate::dict_set_in_place(_py, ns, class_key, class_cell_bits);
            crate::dict_set_in_place(_py, ns, dict_key, namespace_cell_bits);
            CALLBACKS.with(|state| {
                let mut state = state.borrow_mut();
                state.prepared = ns_bits;
                state.class_cell = class_cell_bits;
                state.namespace_cell = namespace_cell_bits;
            });
            let name = crate::attr_name_bits_from_bytes(_py, b"Subject").unwrap();
            let bases = alloc_tuple(_py, &[left, right]);
            let bases_bits = MoltObject::from_ptr(bases).bits();
            let result = call_type(_py, name, bases_bits, ns_bits);
            assert!(!exception_pending(_py));
            CALLBACKS.with(|state| {
                assert_eq!(
                    state.borrow().events,
                    ["set:first", "set:second", "init:left"]
                );
                *state.borrow_mut() = CallbackState::default();
            });
            for bits in [
                result,
                ns_bits,
                class_cell_bits,
                namespace_cell_bits,
                name,
                bases_bits,
                first_key,
                second_key,
                class_key,
                dict_key,
                hook_key,
                set_key,
                left,
                right,
                descriptor_type,
            ] {
                dec_ref_bits(_py, bits);
            }
            assert!(!exception_pending(_py));
        }
    });
}
