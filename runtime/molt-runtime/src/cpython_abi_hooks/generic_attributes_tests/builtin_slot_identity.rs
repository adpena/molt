//! Physical builtin identity must survive managed facades and native children.
use super::*;

thread_local! {
    static CONSTRUCTION_COUNTS: Cell<[usize; 3]> = const { Cell::new([0; 3]) };
}

unsafe fn class_with_slots(
    name: &'static std::ffi::CStr,
    base: *mut PyObject,
    mut declarations: Vec<PyType_Slot>,
) -> OwnedPyObject {
    declarations.push(PyType_Slot {
        slot: 0,
        pfunc: ptr::null_mut(),
    });
    let mut specification = PyType_Spec {
        name: name.as_ptr(),
        basicsize: std::mem::size_of::<PyObject>() as i32,
        itemsize: 0,
        flags: (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE) as u32,
        slots: declarations.as_mut_ptr(),
    };
    // Deliberately no default PyType_GenericNew: omission is a real inherited
    // object.__new__ consumer, independently of the older native_class fixture.
    let result = unsafe {
        OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut specification,
            base,
        ))
    };
    assert!(!result.as_ptr().is_null());
    result
}

unsafe extern "C" fn accepting_new(
    tp: *mut PyTypeObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> *mut PyObject {
    assert_eq!(unsafe { sequences::PyTuple_Size(args) }, 1);
    assert_eq!(unsafe { mapping::PyDict_Size(kwds) }, 1);
    CONSTRUCTION_COUNTS.with(|state| {
        let mut counts = state.get();
        counts[0] += 1;
        state.set(counts);
    });
    unsafe { typeobj::PyType_GenericNew(tp, args, kwds) }
}
unsafe extern "C" fn accepting_init(
    _: *mut PyObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> i32 {
    assert_eq!(unsafe { sequences::PyTuple_Size(args) }, 1);
    assert_eq!(unsafe { mapping::PyDict_Size(kwds) }, 1);
    CONSTRUCTION_COUNTS.with(|state| {
        let mut counts = state.get();
        counts[1] += 1;
        state.set(counts);
    });
    0
}
extern "C" fn spoofed_constructor(_: u64) -> u64 {
    CONSTRUCTION_COUNTS.with(|state| {
        let mut counts = state.get();
        counts[2] += 1;
        state.set(counts);
    });
    MoltObject::none().bits()
}

unsafe fn argument_pair() -> (OwnedPyObject, OwnedPyObject) {
    unsafe {
        let args = OwnedPyObject::from_owned(sequences::PyTuple_New(1));
        assert_eq!(
            sequences::PyTuple_SetItem(args.as_ptr(), 0, numbers::PyLong_FromLong(7)),
            0
        );
        let kwds = OwnedPyObject::from_owned(mapping::PyDict_New());
        let value = OwnedPyObject::from_owned(numbers::PyLong_FromLong(11));
        assert_eq!(
            mapping::PyDict_SetItemString(kwds.as_ptr(), c"value".as_ptr(), value.as_ptr()),
            0
        );
        (args, kwds)
    }
}
unsafe fn clear_type_error() {
    unsafe {
        assert_ne!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
            0
        );
        errors::PyErr_Clear();
    }
}

#[test]
fn builtin_slots_keep_physical_object_identity_through_managed_and_native_descendants() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            CONSTRUCTION_COUNTS.with(|state| state.set([0; 3]));
            let class = snapshot_class(py, b"BuiltinSlotIdentity");
            let base = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            let plain = class_with_slots(c"identity.InheritedNew", base.as_ptr(), vec![]);
            let root = &raw mut PyBaseObject_Type;
            for slot in [
                slots::Py_tp_new,
                slots::Py_tp_init,
                slots::Py_tp_repr,
                slots::Py_tp_str,
                slots::Py_tp_hash,
                slots::Py_tp_richcompare,
                slots::Py_tp_getattro,
                slots::Py_tp_setattro,
            ] {
                let expected = typeobj::PyType_GetSlot(root, slot);
                assert!(!expected.is_null(), "missing object slot {slot}");
                assert_eq!(
                    typeobj::PyType_GetSlot(base.as_ptr().cast(), slot),
                    expected,
                    "managed slot {slot}"
                );
                assert_eq!(
                    typeobj::PyType_GetSlot(plain.as_ptr().cast(), slot),
                    expected,
                    "native slot {slot}"
                );
            }
            let instance = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(plain.as_ptr()));
            assert!(!instance.as_ptr().is_null());
            let representation =
                OwnedPyObject::from_owned(typeobj::PyObject_Repr(instance.as_ptr()));
            assert!(!representation.as_ptr().is_null());
            let equal = OwnedPyObject::from_owned((*root).tp_richcompare.unwrap()(
                instance.as_ptr(),
                instance.as_ptr(),
                2,
            ));
            assert_eq!(object::PyObject_IsTrue(equal.as_ptr()), 1);
            let (args, kwds) = argument_pair();
            assert!(object::PyObject_Call(plain.as_ptr(), args.as_ptr(), kwds.as_ptr()).is_null());
            clear_type_error();
            let custom_new = class_with_slots(
                c"identity.AcceptingNew",
                base.as_ptr(),
                vec![PyType_Slot {
                    slot: slots::Py_tp_new,
                    pfunc: accepting_new as *const () as *mut std::ffi::c_void,
                }],
            );
            let new_instance = OwnedPyObject::from_owned(object::PyObject_Call(
                custom_new.as_ptr(),
                args.as_ptr(),
                kwds.as_ptr(),
            ));
            assert!(!new_instance.as_ptr().is_null());
            assert_eq!(
                (*root).tp_init.unwrap()(new_instance.as_ptr(), args.as_ptr(), kwds.as_ptr()),
                0
            );
            assert!(
                (*root).tp_new.unwrap()(custom_new.as_ptr().cast(), args.as_ptr(), kwds.as_ptr())
                    .is_null()
            );
            clear_type_error();
            let custom_init = class_with_slots(
                c"identity.AcceptingInit",
                base.as_ptr(),
                vec![PyType_Slot {
                    slot: slots::Py_tp_init,
                    pfunc: accepting_init as *const () as *mut std::ffi::c_void,
                }],
            );
            let init_instance = OwnedPyObject::from_owned(object::PyObject_Call(
                custom_init.as_ptr(),
                args.as_ptr(),
                kwds.as_ptr(),
            ));
            assert!(!init_instance.as_ptr().is_null());
            assert_eq!(
                (*root).tp_init.unwrap()(init_instance.as_ptr(), args.as_ptr(), kwds.as_ptr()),
                -1
            );
            clear_type_error();
            assert_eq!(CONSTRUCTION_COUNTS.with(Cell::get), [1, 1, 0]);
            dec_ref_bits(py, class);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn native_new_wrapper_preserves_existing_constructor_on_mutation_and_inheritance() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            CONSTRUCTION_COUNTS.with(|state| state.set([0; 3]));
            let class = snapshot_class(py, b"NativeNewWrapper");
            let base = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            let custom = class_with_slots(
                c"identity.WrappedNew",
                base.as_ptr(),
                vec![PyType_Slot {
                    slot: slots::Py_tp_new,
                    pfunc: accepting_new as *const () as *mut std::ffi::c_void,
                }],
            );
            let wrapper = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                custom.as_ptr(),
                c"__new__".as_ptr(),
            ));
            assert!(!wrapper.as_ptr().is_null());
            set(custom.as_ptr(), c"__new__", wrapper.as_ptr());
            let child = class_with_slots(c"identity.WrappedNewChild", custom.as_ptr(), vec![]);
            set(child.as_ptr(), c"__new__", wrapper.as_ptr());
            let (args, kwds) = argument_pair();
            for owner in [&custom, &child] {
                assert_eq!(
                    typeobj::PyType_GetSlot(owner.as_ptr().cast(), slots::Py_tp_new),
                    accepting_new as *const () as *mut std::ffi::c_void
                );
                let instance = OwnedPyObject::from_owned(object::PyObject_Call(
                    owner.as_ptr(),
                    args.as_ptr(),
                    kwds.as_ptr(),
                ));
                assert!(!instance.as_ptr().is_null());
            }
            let explicit = OwnedPyObject::from_owned(sequences::PyTuple_New(2));
            refcount::Py_INCREF(child.as_ptr());
            assert_eq!(
                sequences::PyTuple_SetItem(explicit.as_ptr(), 0, child.as_ptr()),
                0
            );
            assert_eq!(
                sequences::PyTuple_SetItem(explicit.as_ptr(), 1, numbers::PyLong_FromLong(7)),
                0
            );
            let instance = OwnedPyObject::from_owned(object::PyObject_Call(
                wrapper.as_ptr(),
                explicit.as_ptr(),
                kwds.as_ptr(),
            ));
            assert!(!instance.as_ptr().is_null());
            assert_eq!(CONSTRUCTION_COUNTS.with(Cell::get), [3, 0, 0]);
            dec_ref_bits(py, class);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn same_named_python_functions_never_acquire_builtin_slot_identity_and_deletion_restores_it() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            CONSTRUCTION_COUNTS.with(|state| state.set([0; 3]));
            let class = snapshot_class(py, b"SpoofedBuiltinSlot");
            let base = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            let child = class_with_slots(c"identity.SpoofedChild", base.as_ptr(), vec![]);
            let spoof = function(py, spoofed_constructor as *const (), 1);
            let name_key = crate::attr_name_bits_from_bytes(py, b"__name__").unwrap();
            for (name, slot) in [
                (b"__init__".as_slice(), slots::Py_tp_init),
                (b"__new__".as_slice(), slots::Py_tp_new),
            ] {
                let key = crate::attr_name_bits_from_bytes(py, name).unwrap();
                crate::molt_set_attr_name(spoof, name_key, key);
                assert!(!crate::exception_pending(py));
                crate::molt_set_attr_name(class, key, spoof);
                assert!(!crate::exception_pending(py));
                let canonical = typeobj::PyType_GetSlot(&raw mut PyBaseObject_Type, slot);
                for owner in [&base, &child] {
                    assert_ne!(
                        typeobj::PyType_GetSlot(owner.as_ptr().cast(), slot),
                        canonical
                    );
                }
                let instance =
                    OwnedPyObject::from_owned(object::PyObject_CallNoArgs(child.as_ptr()));
                assert!(!instance.as_ptr().is_null());
                if slot == slots::Py_tp_new {
                    assert_eq!(instance.as_ptr(), &raw mut Py_None);
                }
                crate::molt_del_attr_name(class, key);
                for owner in [&base, &child] {
                    assert_eq!(
                        typeobj::PyType_GetSlot(owner.as_ptr().cast(), slot),
                        canonical
                    );
                }
                dec_ref_bits(py, key);
            }
            assert_eq!(CONSTRUCTION_COUNTS.with(Cell::get), [0, 0, 2]);
            for bits in [name_key, spoof, class] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn managed_exception_descendant_preserves_native_exception_slots() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = crate::attr_name_bits_from_bytes(py, b"PhysicalExceptionSlots").unwrap();
            let class = crate::molt_class_new(name);
            crate::molt_class_set_base(class, crate::builtin_classes(py).base_exception);
            crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
            let view = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class));
            let root = (&raw mut PyExc_BaseException).cast::<PyTypeObject>();
            for slot in [
                slots::Py_tp_init,
                slots::Py_tp_new,
                slots::Py_tp_repr,
                slots::Py_tp_str,
            ] {
                let native = typeobj::PyType_GetSlot(root, slot);
                assert!(!native.is_null());
                assert_eq!(typeobj::PyType_GetSlot(view.as_ptr().cast(), slot), native);
            }
            dec_ref_bits(py, class);
            dec_ref_bits(py, name);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}
