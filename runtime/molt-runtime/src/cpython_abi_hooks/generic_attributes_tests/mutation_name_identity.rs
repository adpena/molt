//! Name objects remain the same operands across normal and raw mutation.
use super::*;
use crate::builtins::exceptions::ExceptionValue;
use molt_cpython_abi::abi_types::{Py_TPFLAGS_READY, PyBaseObject_Type, PyType_Type, PyTypeObject};

thread_local! {
    static LAST_NAME: Cell<u64> = const { Cell::new(0) };
    static EXACT_NAME_BYTES: Cell<bool> = const { Cell::new(false) };
    static NAME_CALLS: Cell<usize> = const { Cell::new(0) };
    static NAME_HASH: Cell<u64> = const { Cell::new(0) };
    static HASH_CALLS: Cell<usize> = const { Cell::new(0) };
    static HASH_FAIL: Cell<bool> = const { Cell::new(false) };
}

extern "C" fn named_set(_receiver: u64, name: u64, _value: u64) -> u64 {
    LAST_NAME.with(|slot| slot.set(name));
    crate::with_gil_entry_nopanic!(py, {
        EXACT_NAME_BYTES.with(|slot| {
            slot.set(
                crate::type_of_bits(py, name) == crate::builtin_classes(py).str
                    && crate::string_obj_to_owned(obj_from_bits(name)).as_deref()
                        == Some("mutation_name_identity"),
            )
        });
    });
    NAME_CALLS.with(|slot| slot.set(slot.get() + 1));
    let failure = FAILURE.with(Cell::get);
    if failure != 0 {
        return crate::molt_raise(failure);
    }
    MoltObject::none().bits()
}

extern "C" fn named_delete(receiver: u64, name: u64) -> u64 {
    named_set(receiver, name, MoltObject::none().bits())
}

extern "C" fn name_hash(_name: u64) -> u64 {
    HASH_CALLS.with(|slot| slot.set(slot.get() + 1));
    if HASH_FAIL.with(Cell::get) {
        return crate::with_gil_entry_nopanic!(py, {
            crate::raise_exception::<u64>(py, "ValueError", "attribute-name hash sentinel")
        });
    }
    NAME_HASH.with(Cell::get)
}

extern "C" fn name_equal(left: u64, right: u64) -> u64 {
    MoltObject::from_bool(left == right).bits()
}

fn new_class(py: &crate::PyToken<'_>, name: &[u8], base: u64) -> u64 {
    let name = ExceptionValue::adopt(py, crate::attr_name_bits_from_bytes(py, name).unwrap());
    let class = ExceptionValue::adopt(py, crate::molt_class_new(name.bits()));
    crate::molt_class_set_base(class.bits(), base);
    assert!(!crate::exception_pending(py));
    class.into_bits()
}

fn finish_class(py: &crate::PyToken<'_>, class: u64) {
    unsafe { crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap()) }
        .expect("name identity fixture class");
}

fn subclass_name(py: &crate::PyToken<'_>) -> (u64, u64, u64) {
    HASH_FAIL.with(|slot| slot.set(false));
    let spelling = crate::attr_name_bits_from_bytes(py, b"mutation_name_identity").unwrap();
    let spelling_owner = ExceptionValue::adopt(py, spelling);
    let ordinary_hash = crate::molt_hash_builtin(spelling);
    let alternate = MoltObject::from_int(if ordinary_hash == MoltObject::from_int(137).bits() {
        138
    } else {
        137
    })
    .bits();
    dec_ref_bits(py, ordinary_hash);
    NAME_HASH.with(|slot| slot.set(alternate));
    let class = new_class(py, b"MutationName", crate::builtin_classes(py).str);
    let class_owner = ExceptionValue::adopt(py, class);
    method(py, class, b"__eq__", name_equal as *const (), 2);
    method(py, class, b"__hash__", name_hash as *const (), 1);
    finish_class(py, class);
    let name =
        unsafe { crate::call::bind::call_bind_borrowed(py, class, None, &[spelling], &[], &[]) };
    let name_owner = ExceptionValue::adopt(py, name);
    assert!(!crate::exception_pending(py));
    assert_ne!(name, spelling);
    assert_eq!(crate::type_of_bits(py, name), class);
    assert_eq!(
        unsafe { crate::object_type_id(obj_from_bits(name).as_ptr().unwrap()) },
        crate::TYPE_ID_STRING
    );
    (
        name_owner.into_bits(),
        spelling_owner.into_bits(),
        class_owner.into_bits(),
    )
}

#[test]
fn named_mutation_preserves_original_name_for_every_override_family() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            FAILURE.with(|slot| slot.set(0));
            let (name, spelling, name_class) = subclass_name(py);
            let classes = crate::builtin_classes(py);
            for (base, dataclass) in [
                (classes.object, false),
                (classes.list, false),
                (classes.module, false),
                (classes.object, true),
                (classes.type_obj, false),
            ] {
                let class = new_class(py, b"NameObservingOwner", base);
                method(py, class, b"__setattr__", named_set as *const (), 3);
                method(py, class, b"__delattr__", named_delete as *const (), 2);
                finish_class(py, class);
                let receiver = if base == classes.module {
                    crate::call::bind::call_bind_borrowed(py, class, None, &[spelling], &[], &[])
                } else if base == classes.type_obj {
                    let bases = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
                    let namespace = crate::molt_dict_new(0);
                    let receiver = crate::builtins::types::molt_type_new(
                        class,
                        spelling,
                        bases,
                        namespace,
                        MoltObject::none().bits(),
                    );
                    dec_ref_bits(py, namespace);
                    dec_ref_bits(py, bases);
                    receiver
                } else {
                    snapshot_receiver(py, class, dataclass)
                };
                assert!(!crate::exception_pending(py));
                let name_ptr = obj_from_bits(name).as_ptr().unwrap();
                let before = (*crate::header_from_obj_ptr(name_ptr)).ref_count_snapshot();
                NAME_CALLS.with(|slot| slot.set(0));
                crate::molt_set_attr_name(receiver, name, MoltObject::from_int(19).bits());
                assert_eq!(LAST_NAME.with(Cell::get), name);
                crate::molt_del_attr_name(receiver, name);
                assert_eq!(LAST_NAME.with(Cell::get), name);
                assert_eq!(NAME_CALLS.with(Cell::get), 2);
                assert_eq!(
                    (*crate::header_from_obj_ptr(name_ptr)).ref_count_snapshot(),
                    before
                );
                assert!(!crate::exception_pending(py));
                // Both byte ABI directions create their one exact string and
                // enter the same named override transaction.
                let bytes = b"mutation_name_identity";
                crate::molt_set_attr_object(
                    receiver,
                    bytes.as_ptr(),
                    bytes.len() as u64,
                    MoltObject::none().bits(),
                );
                assert!(EXACT_NAME_BYTES.with(Cell::get));
                crate::molt_del_attr_object(receiver, bytes.as_ptr(), bytes.len() as u64);
                assert!(EXACT_NAME_BYTES.with(Cell::get));
                assert_eq!(NAME_CALLS.with(Cell::get), 4);
                assert!(!crate::exception_pending(py));
                let failure = MoltObject::from_ptr(crate::builtins::exceptions::alloc_exception(
                    py,
                    "ValueError",
                    "name override sentinel",
                ))
                .bits();
                FAILURE.with(|slot| slot.set(failure));
                let result = crate::molt_set_attr_name(receiver, name, MoltObject::none().bits());
                let observed = crate::builtins::exceptions::molt_exception_last_pending();
                assert_eq!(observed, failure);
                assert_eq!(LAST_NAME.with(Cell::get), name);
                assert_eq!(
                    (*crate::header_from_obj_ptr(name_ptr)).ref_count_snapshot(),
                    before
                );
                crate::molt_exception_clear();
                FAILURE.with(|slot| slot.set(0));
                for value in [observed, result, failure] {
                    dec_ref_bits(py, value);
                }
                dec_ref_bits(py, receiver);
                dec_ref_bits(py, class);
            }
            for value in [name, spelling, name_class] {
                dec_ref_bits(py, value);
            }
        }
    });
}

#[test]
fn raw_c_generic_mutation_roundtrips_name_subclass_identity_and_hash() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (name, spelling, name_class) = subclass_name(py);
            let owner_class = snapshot_class(py, b"NameStorageOwner");
            let instance = snapshot_receiver(py, owner_class, false);
            let dataclass = snapshot_receiver(py, owner_class, true);
            let native_class = new_class(py, b"NameStorageList", crate::builtin_classes(py).list);
            finish_class(py, native_class);
            let native = snapshot_receiver(py, native_class, false);
            let function = function(py, crossing_receiver as *const (), 1);
            let name_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name);
            let value_bits = MoltObject::from_int(83).bits();
            let value = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(value_bits);
            for receiver in [
                instance,
                dataclass,
                native,
                function,
                owner_class,
                crate::builtin_classes(py).int,
            ] {
                let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(receiver);
                assert!(!view.is_null() && !name_view.is_null() && !value.is_null());
                HASH_CALLS.with(|slot| slot.set(0));
                assert_eq!(object::PyObject_GenericSetAttr(view, name_view, value), 0);
                assert!(HASH_CALLS.with(Cell::get) > 0);
                let dictionary = crate::object::field_storage::current_dictionary(
                    py,
                    obj_from_bits(receiver).as_ptr().unwrap(),
                )
                .expect("valid generic dictionary")
                .expect("materialized generic dictionary");
                let dictionary = obj_from_bits(dictionary).as_ptr().unwrap();
                assert_eq!(
                    crate::dict_get_in_place(py, dictionary, name),
                    Some(value_bits)
                );
                assert_eq!(crate::dict_get_in_place(py, dictionary, spelling), None);
                assert!(crate::dict_live_entries(dictionary).any(|row| row.key == name));
                let read = object::PyObject_GenericGetAttr(view, name_view);
                assert_eq!(take_bits(read), value_bits);
                assert_eq!(
                    object::PyObject_GenericSetAttr(view, name_view, std::ptr::null_mut()),
                    0
                );
                assert_eq!(crate::dict_get_in_place(py, dictionary, name), None);
                assert!(!crate::exception_pending(py));
                refcount::Py_DECREF(view);
            }
            // A name-owned callback failure survives the raw mutation boundary.
            let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(instance);
            HASH_FAIL.with(|slot| slot.set(true));
            assert_eq!(object::PyObject_GenericSetAttr(view, name_view, value), -1);
            assert_ne!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                ),
                0
            );
            HASH_FAIL.with(|slot| slot.set(false));
            errors::PyErr_Clear();
            assert!(!crate::exception_pending(py));
            for pointer in [view, value, name_view] {
                refcount::Py_DECREF(pointer);
            }
            for bits in [
                function,
                native,
                native_class,
                dataclass,
                instance,
                owner_class,
                name,
                spelling,
                name_class,
            ] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

unsafe extern "C" fn native_named_set(
    _receiver: *mut PyObject,
    name: *mut PyObject,
    _value: *mut PyObject,
) -> std::os::raw::c_int {
    let name = GLOBAL_BRIDGE
        .observed_handle_for_pyobj(name)
        .expect("projected name")
        .bits();
    LAST_NAME.with(|slot| slot.set(name));
    NAME_CALLS.with(|slot| slot.set(slot.get() + 1));
    0
}

#[test]
fn foreign_normal_mutation_receives_original_name_projection() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (name, spelling, name_class) = subclass_name(py);
            let mut class: PyTypeObject = std::mem::zeroed();
            class.ob_base.ob_base.ob_refcnt = 1;
            class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            class.tp_base = &raw mut PyBaseObject_Type;
            class.tp_name = c"NativeNameOwner".as_ptr();
            class.tp_flags = Py_TPFLAGS_READY;
            class.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
            class.tp_setattro = Some(native_named_set);
            let mut receiver = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut class,
            };
            let bits = GLOBAL_BRIDGE
                .molt_value_for_pyobj(&raw mut receiver)
                .unwrap();
            NAME_CALLS.with(|slot| slot.set(0));
            crate::molt_set_attr_name(bits, name, MoltObject::from_int(9).bits());
            assert_eq!(LAST_NAME.with(Cell::get), name);
            crate::molt_del_attr_name(bits, name);
            assert_eq!(LAST_NAME.with(Cell::get), name);
            assert_eq!(NAME_CALLS.with(Cell::get), 2);
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, bits);
            for value in [name, spelling, name_class] {
                dec_ref_bits(py, value);
            }
        }
    });
}

#[test]
fn managed_type_defaults_canonicalize_names_without_projection() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let (name, spelling, name_class) = subclass_name(py);
            let _names = [name, spelling, name_class].map(|bits| ExceptionValue::adopt(py, bits));
            let assert_name_unprojected = |phase| {
                assert!(
                    !(*crate::header_from_obj_ptr(obj_from_bits(name).as_ptr().unwrap()))
                        .has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW),
                    "original subclass name projected during {phase}"
                );
            };
            assert_name_unprojected("subclass construction");
            let class = new_class(
                py,
                b"UnprojectedMutableType",
                crate::builtin_classes(py).object,
            );
            let _class_owner = ExceptionValue::adopt(py, class);
            finish_class(py, class);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let dictionary = obj_from_bits(crate::class_dict_bits(class_ptr))
                .as_ptr()
                .unwrap();
            assert!(!GLOBAL_BRIDGE.type_has_projection(class));
            // A subclass hash would choose another dictionary bucket and fail
            // these exact-string reads. Default mutation must never invoke it.
            HASH_CALLS.with(|calls| calls.set(0));
            struct ResetHash;
            impl Drop for ResetHash {
                fn drop(&mut self) {
                    HASH_FAIL.with(|fail| fail.set(false));
                }
            }
            let _reset_hash = ResetHash;
            HASH_FAIL.with(|fail| fail.set(true));
            for explicit in [false, true] {
                if explicit {
                    crate::builtins::methods::type_setattr(
                        class,
                        name,
                        MoltObject::from_int(31).bits(),
                    );
                } else {
                    crate::molt_set_attr_name(class, name, MoltObject::from_int(31).bits());
                }
                assert!(!crate::exception_pending(py));
                assert_name_unprojected(if explicit {
                    "explicit type assignment"
                } else {
                    "normal assignment"
                });
                assert_eq!(
                    crate::dict_get_in_place(py, dictionary, spelling),
                    Some(MoltObject::from_int(31).bits())
                );
                let key = crate::dict_live_entries(dictionary)
                    .find(|row| {
                        crate::string_obj_to_owned(obj_from_bits(row.key)).as_deref()
                            == Some("mutation_name_identity")
                    })
                    .expect("stored class key")
                    .key;
                assert_eq!(crate::type_of_bits(py, key), crate::builtin_classes(py).str);
                assert_ne!(key, name);
                assert!(!GLOBAL_BRIDGE.type_has_projection(class));
                if explicit {
                    crate::builtins::methods::type_delattr(class, name);
                } else {
                    crate::molt_del_attr_name(class, name);
                }
                assert!(!crate::exception_pending(py));
                assert_name_unprojected(if explicit {
                    "explicit type deletion"
                } else {
                    "normal deletion"
                });
                assert_eq!(crate::dict_get_in_place(py, dictionary, spelling), None);
                assert!(!GLOBAL_BRIDGE.type_has_projection(class));
            }
            assert_eq!(HASH_CALLS.with(Cell::get), 0);
            assert!(
                !(*crate::header_from_obj_ptr(obj_from_bits(name).as_ptr().unwrap()))
                    .has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW)
            );
            HASH_FAIL.with(|fail| fail.set(false));
        }
    });
}

#[test]
fn managed_metaclass_override_precedes_immutability_and_keeps_original_name() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            FAILURE.with(|failure| failure.set(0));
            let (name, spelling, name_class) = subclass_name(py);
            let meta = new_class(
                py,
                b"ImmutableOverrideMeta",
                crate::builtin_classes(py).type_obj,
            );
            method(py, meta, b"__setattr__", named_set as *const (), 3);
            method(py, meta, b"__delattr__", named_delete as *const (), 2);
            finish_class(py, meta);
            let bases = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
            let namespace = crate::molt_dict_new(0);
            let class = crate::builtins::types::molt_type_new(
                meta,
                spelling,
                bases,
                namespace,
                MoltObject::none().bits(),
            );
            assert!(!crate::exception_pending(py));
            assert!(crate::object::class_set_immutable(
                py,
                obj_from_bits(class).as_ptr().unwrap()
            ));
            NAME_CALLS.with(|calls| calls.set(0));
            crate::molt_set_attr_name(class, name, MoltObject::none().bits());
            assert_eq!(LAST_NAME.with(Cell::get), name);
            crate::molt_del_attr_name(class, name);
            assert_eq!(LAST_NAME.with(Cell::get), name);
            assert_eq!(NAME_CALLS.with(Cell::get), 2);
            assert!(!crate::exception_pending(py));
            assert!(!GLOBAL_BRIDGE.type_has_projection(class));
            // Explicit default still owns immutable rejection after an override
            // elects to delegate, and cannot invoke the override a second time.
            crate::builtins::methods::type_setattr(class, name, MoltObject::none().bits());
            assert!(crate::exception_pending(py));
            assert_eq!(NAME_CALLS.with(Cell::get), 2);
            crate::molt_exception_clear();
            for bits in [class, namespace, bases, meta, name, spelling, name_class] {
                dec_ref_bits(py, bits);
            }
        }
    });
}
