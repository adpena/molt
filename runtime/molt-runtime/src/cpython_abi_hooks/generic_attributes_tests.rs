//! C generic attributes use logical descriptors while bypassing user overrides.
use super::*;
use crate::obj_from_bits;
use molt_cpython_abi::api::{errors, mapping, object, refcount, sequences, strings};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::cell::Cell;

thread_local! {
    static CALLS: Cell<[usize; 6]> = const { Cell::new([0; 6]) };
    static ASSIGNED: Cell<u64> = const { Cell::new(0) };
    static FAILURE: Cell<u64> = const { Cell::new(0) };
    static FOREIGN_MUTATIONS: Cell<usize> = const { Cell::new(0) };
}

fn record(index: usize) {
    CALLS.with(|calls| {
        let mut next = calls.get();
        next[index] += 1;
        calls.set(next);
    });
}

extern "C" fn crossing_receiver(receiver: u64) -> u64 {
    with_gil(|py| inc_ref_bits(&py, receiver));
    receiver
}

unsafe extern "C" fn crossing_native_get(
    _descriptor: *mut PyObject,
    receiver: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    record(5);
    unsafe { refcount::Py_INCREF(receiver) };
    receiver
}

unsafe extern "C" fn crossing_get_override(
    _receiver: *mut PyObject,
    _name: *mut PyObject,
) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(777) }
}

unsafe extern "C" fn crossing_set_override(
    _receiver: *mut PyObject,
    _name: *mut PyObject,
    _value: *mut PyObject,
) -> std::os::raw::c_int {
    FOREIGN_MUTATIONS.with(|count| count.set(count.get() + 1));
    0
}

// Public error transfer covers both native and runtime pending indicators.
// Restore the same raised instance after rendering, including its traceback.
unsafe fn native_error_description() -> String {
    let error = unsafe { errors::PyErr_GetRaisedException() };
    let error_owner = unsafe { refcount::OwnedPyObject::from_owned(error) };
    if error.is_null() {
        return "no native error".into();
    }
    let description = unsafe {
        errors::with_preserved_error(|| {
            let rendered = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::typeobj::PyObject_Str(error),
            );
            let bytes = strings::PyUnicode_AsUTF8(rendered.as_ptr());
            if bytes.is_null() {
                "native error rendering failed".into()
            } else {
                std::ffi::CStr::from_ptr(bytes)
                    .to_string_lossy()
                    .into_owned()
            }
        })
    };
    unsafe { errors::PyErr_SetRaisedException(error_owner.into_ptr()) };
    description
}

unsafe fn assert_native_setter_rejection(delete: bool, type_name: &str) {
    unsafe {
        let raised = errors::PyErr_GetRaisedException();
        let raised_owner = refcount::OwnedPyObject::from_owned(raised);
        assert!(!raised.is_null());
        assert_ne!(
            errors::PyErr_GivenExceptionMatches(
                raised,
                (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
            ),
            0
        );
        let text = molt_cpython_abi::api::typeobj::PyObject_Str(raised);
        let text_owner = refcount::OwnedPyObject::from_owned(text);
        assert!(!text.is_null());
        let bytes = strings::PyUnicode_AsUTF8(text);
        assert!(!bytes.is_null());
        let operation = if delete { "__delattr__" } else { "__setattr__" };
        assert_eq!(
            std::ffi::CStr::from_ptr(bytes).to_string_lossy(),
            format!("can't apply this {operation} to {type_name} object")
        );
        drop(text_owner);
        drop(raised_owner);
        assert!(errors::PyErr_Occurred().is_null());
    }
}

#[test]
fn native_namespaces_bind_and_mutate_managed_descriptors() {
    use molt_cpython_abi::abi_types::{
        Py_None, Py_TPFLAGS_READY, PyBaseObject_Type, PyType_Type, PyTypeObject,
    };
    use molt_cpython_abi::api::{numbers, typeobj};
    #[repr(C)]
    struct NativeReceiver {
        object: PyObject,
        dictionary: *mut PyObject,
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            CALLS.with(|calls| calls.set([0; 6]));
            FAILURE.with(|failure| failure.set(0));
            let mut class: PyTypeObject = std::mem::zeroed();
            class.ob_base.ob_base.ob_refcnt = 1;
            class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            class.tp_base = &raw mut PyBaseObject_Type;
            class.tp_name = c"NativeManagedDescriptorReceiver".as_ptr();
            class.tp_flags = Py_TPFLAGS_READY;
            class.tp_basicsize = std::mem::size_of::<NativeReceiver>() as isize;
            class.tp_dictoffset = std::mem::offset_of!(NativeReceiver, dictionary) as isize;
            class.tp_dict = mapping::PyDict_New();
            // Independent valid MRO: a NULL tp_mro intentionally bypasses
            // CPython's hackcheck and cannot witness native override rejection.
            class.tp_mro = sequences::PyTuple_New(2);
            assert!(!class.tp_mro.is_null());
            for (index, base) in [(&raw mut class).cast(), (&raw mut PyBaseObject_Type).cast()]
                .into_iter()
                .enumerate()
            {
                refcount::Py_INCREF(base);
                assert_eq!(
                    sequences::PyTuple_SetItem(class.tp_mro, index as isize, base),
                    0
                );
            }
            let mut receiver = NativeReceiver {
                object: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut class,
                },
                dictionary: mapping::PyDict_New(),
            };
            assert!(!class.tp_dict.is_null() && !receiver.dictionary.is_null());
            let receiver_ptr = &raw mut receiver.object;
            let receiver_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(receiver_ptr).unwrap();
            let get = function(py, descriptor_get as *const (), 1);
            let set = function(py, descriptor_set as *const (), 2);
            let delete = function(py, descriptor_delete as *const (), 1);
            let property = crate::molt_property_new(get, set, delete);
            let method = function(py, crossing_receiver as *const (), 1);
            let property_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(property);
            let method_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(method);
            let name = strings::PyUnicode_FromString(c"field".as_ptr());
            let method_name = strings::PyUnicode_FromString(c"method".as_ptr());
            let shadow = numbers::PyLong_FromLong(7);
            assert_eq!(
                mapping::PyDict_SetItem(class.tp_dict, name, property_view),
                0
            );
            assert_eq!(
                mapping::PyDict_SetItem(class.tp_dict, method_name, method_view),
                0
            );
            assert_eq!(
                mapping::PyDict_SetItem(receiver.dictionary, name, shadow),
                0
            );
            assert_eq!(typeobj::PyDescr_IsData(property_view), 1);
            assert_eq!(typeobj::PyDescr_IsData(method_view), 0);
            let read = object::PyObject_GenericGetAttr(receiver_ptr, name);
            assert!(!read.is_null());
            assert_eq!(
                numbers::PyLong_AsLong(read),
                222,
                "managed data descriptor wins over native instance dict"
            );
            refcount::Py_DECREF(read);
            // Runtime explicit object lookup must cross the same generic
            // boundary instead of invoking a native getattribute override.
            class.tp_getattro = Some(crossing_get_override);
            let name_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(name).unwrap();
            let normal = crate::molt_get_attr_name(receiver_bits, name_bits);
            assert_eq!(normal, MoltObject::from_int(777).bits());
            dec_ref_bits(py, normal);
            let generic = crate::molt_object_getattribute(receiver_bits, name_bits);
            assert_eq!(generic, MoltObject::from_int(222).bits());
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, generic);
            dec_ref_bits(py, name_bits);
            class.tp_getattro = None;
            let bound = object::PyObject_GenericGetAttr(receiver_ptr, method_name);
            assert!(!bound.is_null());
            let result = object::PyObject_CallNoArgs(bound);
            assert_eq!(
                result, receiver_ptr,
                "managed function binds the actual native receiver"
            );
            refcount::Py_DECREF(result);
            refcount::Py_DECREF(bound);
            let class_read = object::PyObject_GetAttr((&raw mut class).cast(), name);
            assert_eq!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(class_read)
                    .map(|value| value.bits()),
                Some(property),
                "NULL receiver is class access"
            );
            refcount::Py_DECREF(class_read);

            assert_eq!(
                object::PyObject_GenericSetAttr(receiver_ptr, name, &raw mut Py_None),
                0
            );
            assert_eq!(
                ASSIGNED.with(Cell::get),
                MoltObject::none().bits(),
                "None is a value, not deletion"
            );
            assert_eq!(
                object::PyObject_GenericSetAttr(receiver_ptr, name, ptr::null_mut()),
                0
            );
            assert_eq!(&CALLS.with(Cell::get)[3..5], &[1, 1]);
            class.tp_setattro = Some(crossing_set_override);
            FOREIGN_MUTATIONS.with(|count| count.set(0));
            assert_eq!(
                object::PyObject_SetAttr(receiver_ptr, name, &raw mut Py_None),
                0
            );
            assert_eq!(
                object::PyObject_SetAttr(receiver_ptr, name, ptr::null_mut()),
                0
            );
            assert_eq!(FOREIGN_MUTATIONS.with(Cell::get), 2);
            let name_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(name).unwrap();
            for delete in [false, true] {
                let result = if delete {
                    crate::molt_object_delattr(receiver_bits, name_bits)
                } else {
                    crate::molt_object_setattr(
                        receiver_bits,
                        name_bits,
                        MoltObject::from_int(55).bits(),
                    )
                };
                dec_ref_bits(py, result);
                assert_native_setter_rejection(delete, "NativeManagedDescriptorReceiver");
            }
            assert_eq!(&CALLS.with(Cell::get)[3..5], &[1, 1]);
            assert_eq!(FOREIGN_MUTATIONS.with(Cell::get), 2);

            // The direct C generic primitive deliberately remains unchecked,
            // even on precisely the native override rejected above.
            let value = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(MoltObject::from_int(55).bits());
            assert_eq!(
                object::PyObject_GenericSetAttr(receiver_ptr, name, value),
                0
            );
            assert_eq!(
                object::PyObject_GenericSetAttr(receiver_ptr, name, ptr::null_mut()),
                0
            );
            assert_eq!(ASSIGNED.with(Cell::get), MoltObject::from_int(55).bits());
            assert_eq!(&CALLS.with(Cell::get)[3..5], &[2, 2]);

            class.tp_setattro = Some(object::PyObject_GenericSetAttr);
            let result =
                crate::molt_object_setattr(receiver_bits, name_bits, MoltObject::none().bits());
            dec_ref_bits(py, result);
            let result = crate::molt_object_delattr(receiver_bits, name_bits);
            dec_ref_bits(py, result);
            assert_eq!(&CALLS.with(Cell::get)[3..5], &[3, 3]);
            assert!(!crate::exception_pending(py));

            // NULL slot with an existing MRO is a native mismatch, not Python
            // dispatch. NULL MRO is CPython's separate permissive case.
            class.tp_setattro = None;
            for delete in [false, true] {
                let result = if delete {
                    crate::molt_object_delattr(receiver_bits, name_bits)
                } else {
                    crate::molt_object_setattr(receiver_bits, name_bits, MoltObject::none().bits())
                };
                dec_ref_bits(py, result);
                assert_native_setter_rejection(delete, "NativeManagedDescriptorReceiver");
            }
            let mro = std::mem::replace(&mut class.tp_mro, ptr::null_mut());
            let result =
                crate::molt_object_setattr(receiver_bits, name_bits, MoltObject::none().bits());
            dec_ref_bits(py, result);
            let result = crate::molt_object_delattr(receiver_bits, name_bits);
            dec_ref_bits(py, result);
            class.tp_mro = mro;
            assert_eq!(&CALLS.with(Cell::get)[3..5], &[4, 4]);
            assert_eq!(FOREIGN_MUTATIONS.with(Cell::get), 2);
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, name_bits);
            // No mutation protocol means ordinary instance-dictionary assignment.
            let zero = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(MoltObject::from_float(0.0).bits());
            assert_eq!(
                object::PyObject_GenericSetAttr(receiver_ptr, method_name, zero),
                0
            );
            let read = object::PyObject_GenericGetAttr(receiver_ptr, method_name);
            assert_eq!(numbers::PyFloat_AsDouble(read), 0.0);
            refcount::Py_DECREF(read);
            assert_eq!(
                object::PyObject_GenericSetAttr(receiver_ptr, method_name, ptr::null_mut()),
                0
            );

            let failure = MoltObject::from_ptr(crate::builtins::exceptions::alloc_exception(
                py,
                "ValueError",
                "managed descriptor body",
            ))
            .bits();
            FAILURE.with(|slot| slot.set(failure));
            for operation in 0..3 {
                if operation == 0 {
                    assert!(object::PyObject_GenericGetAttr(receiver_ptr, name).is_null());
                } else {
                    let value = if operation == 1 {
                        &raw mut Py_None
                    } else {
                        ptr::null_mut()
                    };
                    assert_eq!(
                        object::PyObject_GenericSetAttr(receiver_ptr, name, value),
                        -1
                    );
                }
                let raised = errors::PyErr_GetRaisedException();
                let raised_owner = refcount::OwnedPyObject::from_owned(raised);
                assert!(!raised.is_null());
                assert_eq!(
                    GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(raised)
                        .map(|value| value.bits()),
                    Some(failure)
                );
                drop(raised_owner);
                assert_eq!(
                    mapping::PyDict_GetItem(receiver.dictionary, name),
                    shadow,
                    "descriptor errors never fall back to storage"
                );
            }
            FAILURE.with(|slot| slot.set(0));

            // Metaclass data descriptors have the same precedence on native types.
            let mut meta: PyTypeObject = std::mem::zeroed();
            meta.ob_base.ob_base.ob_refcnt = 1;
            meta.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            meta.tp_base = &raw mut PyType_Type;
            meta.tp_name = c"ManagedDescriptorMeta".as_ptr();
            meta.tp_flags = Py_TPFLAGS_READY;
            meta.tp_getattro = PyType_Type.tp_getattro;
            meta.tp_dict = mapping::PyDict_New();
            assert!(!meta.tp_dict.is_null());
            assert_eq!(
                mapping::PyDict_SetItem(meta.tp_dict, name, property_view),
                0
            );
            class.ob_base.ob_base.ob_type = &raw mut meta;
            let class_read = object::PyObject_GetAttr((&raw mut class).cast(), name);
            assert!(!class_read.is_null());
            assert_eq!(numbers::PyLong_AsLong(class_read), 222);
            refcount::Py_DECREF(class_read);
            assert_eq!(
                object::PyObject_GenericSetAttr((&raw mut class).cast(), name, &raw mut Py_None),
                0
            );
            assert_eq!(
                object::PyObject_GenericSetAttr((&raw mut class).cast(), name, ptr::null_mut()),
                0
            );
            class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            refcount::Py_DECREF(meta.tp_dict);

            // A native descriptor remains on its exact C slot after acquiring a
            // foreign runtime wrapper; that wrapper is not a managed C view.
            let mut native_type: PyTypeObject = std::mem::zeroed();
            native_type.tp_descr_get = Some(crossing_native_get);
            let mut native = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut native_type,
            };
            let native_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(&raw mut native).unwrap();
            assert!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(&raw mut native)
                    .is_none()
            );
            assert_eq!(
                mapping::PyDict_SetItem(class.tp_dict, method_name, &raw mut native),
                0
            );
            let result = object::PyObject_GenericGetAttr(receiver_ptr, method_name);
            assert_eq!(result, receiver_ptr);
            assert_eq!(CALLS.with(Cell::get)[5], 1);
            refcount::Py_DECREF(result);
            assert_eq!(mapping::PyDict_DelItem(class.tp_dict, method_name), 0);
            dec_ref_bits(py, native_bits);
            let mro = std::mem::replace(&mut class.tp_mro, ptr::null_mut());
            for value in [
                mro,
                receiver.dictionary,
                class.tp_dict,
                name,
                method_name,
                shadow,
            ] {
                refcount::Py_DECREF(value);
            }
            for bits in [failure, method, property, get, set, delete, receiver_bits] {
                dec_ref_bits(py, bits);
            }
            ASSIGNED.with(|slot| slot.set(0));
            assert!(!crate::exception_pending(py));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

extern "C" fn overridden_get(_receiver: u64, _name: u64) -> u64 {
    record(0);
    MoltObject::from_int(701).bits()
}

extern "C" fn overridden_set(_receiver: u64, _name: u64, _value: u64) -> u64 {
    record(1);
    MoltObject::none().bits()
}

extern "C" fn overridden_delete(_receiver: u64, _name: u64) -> u64 {
    record(2);
    MoltObject::none().bits()
}

extern "C" fn descriptor_get(_receiver: u64) -> u64 {
    let failure = FAILURE.with(Cell::get);
    if failure != 0 {
        return crate::molt_raise(failure);
    }
    MoltObject::from_int(222).bits()
}

extern "C" fn descriptor_set(_receiver: u64, value: u64) -> u64 {
    record(3);
    ASSIGNED.with(|assigned| assigned.set(value));
    let failure = FAILURE.with(Cell::get);
    if failure != 0 {
        return crate::molt_raise(failure);
    }
    MoltObject::none().bits()
}

extern "C" fn descriptor_delete(_receiver: u64) -> u64 {
    record(4);
    let failure = FAILURE.with(Cell::get);
    if failure != 0 {
        return crate::molt_raise(failure);
    }
    MoltObject::none().bits()
}

fn function(py: &crate::PyToken<'_>, target: *const (), arity: u64) -> u64 {
    MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::provenance::abi::expose_function_address(target),
        arity,
    ))
    .bits()
}

fn member(py: &crate::PyToken<'_>, class: u64, name: &[u8], value: u64) {
    let key = crate::attr_name_bits_from_bytes(py, name).unwrap();
    crate::molt_set_attr_name(class, key, value);
    dec_ref_bits(py, key);
    assert!(!crate::exception_pending(py));
}

fn method(py: &crate::PyToken<'_>, class: u64, name: &[u8], target: *const (), arity: u64) {
    let callable = function(py, target, arity);
    member(py, class, name, callable);
    dec_ref_bits(py, callable);
}

fn attribute_class(py: &crate::PyToken<'_>, base: u64, dictless: bool) -> u64 {
    let name = crate::attr_name_bits_from_bytes(py, b"ManagedGenericAttributes").unwrap();
    let class = crate::molt_class_new(name);
    dec_ref_bits(py, name);
    crate::molt_class_set_base(class, base);
    if dictless {
        let slots = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
        member(py, class, b"__slots__", slots);
        dec_ref_bits(py, slots);
    }
    method(
        py,
        class,
        b"__getattribute__",
        overridden_get as *const (),
        2,
    );
    method(py, class, b"__setattr__", overridden_set as *const (), 3);
    method(py, class, b"__delattr__", overridden_delete as *const (), 2);
    member(py, class, b"shadow", MoltObject::from_int(9).bits());
    let get = function(py, descriptor_get as *const (), 1);
    let set = function(py, descriptor_set as *const (), 2);
    let delete = function(py, descriptor_delete as *const (), 1);
    let property = crate::molt_property_new(get, set, delete);
    member(py, class, b"managed", property);
    for bits in [property, get, set, delete] {
        dec_ref_bits(py, bits);
    }
    unsafe {
        crate::object::class_finish_definition(py, MoltObject::from_bits(class).as_ptr().unwrap())
            .unwrap();
    }
    class
}

unsafe fn take_bits(value: *mut PyObject) -> u64 {
    let value_owner = unsafe { refcount::OwnedPyObject::from_owned(value) };
    assert!(!value.is_null());
    let bits = GLOBAL_BRIDGE
        .observed_handle_for_pyobj(value)
        .unwrap()
        .bits();
    drop(value_owner);
    bits
}

#[test]
fn raised_error_conversion_unwind_retires_unprojected_native_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(super::register_cpython_hooks());
    crate::concurrency::gil::with_gil(|py| unsafe {
        let args = refcount::OwnedPyObject::from_owned(sequences::PyTuple_New(0));
        assert!(!args.as_ptr().is_null());
        let native = refcount::OwnedPyObject::from_owned(errors::molt_native_exception_new(
            &raw mut molt_cpython_abi::abi_types::PyExc_ValueError,
            args.as_ptr(),
            ptr::null_mut(),
        ));
        assert!(!native.as_ptr().is_null());
        let address = native.as_ptr().addr();
        assert!(crate::object::gc::native_gc_is_enrolled(address));
        assert!(
            GLOBAL_BRIDGE
                .observed_handle_for_pyobj(native.as_ptr())
                .is_none()
        );
        errors::PyErr_SetRaisedException(native.into_ptr());
        // The existing consuming helper requires a managed projection. A real
        // unprojected native exception exercises its conversion failure after
        // the public getter has transferred the last C owner.
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            take_bits(errors::PyErr_GetRaisedException())
        }));
        assert!(
            failure.is_err(),
            "the unprojected fixture must fail conversion"
        );
        assert!(
            !crate::object::gc::native_gc_is_enrolled(address),
            "conversion unwind leaked the transferred native exception"
        );
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn explicit_native_wrappers_skip_managed_dispatch_but_keep_native_ancestors() {
    use molt_cpython_abi::abi_types::{Py_None, PyBaseObject_Type};
    use molt_cpython_abi::api::typeobj;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for native_override in [false, true] {
                CALLS.with(|calls| calls.set([0; 6]));
                FAILURE.with(|failure| failure.set(0));
                let classes = crate::builtin_classes(py);
                let base = if native_override {
                    classes.type_obj
                } else {
                    classes.object
                };
                let class = attribute_class(py, base, false);
                let receiver = if native_override {
                    let name =
                        crate::attr_name_bits_from_bytes(py, b"SetterClassInstance").unwrap();
                    let bases = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
                    let namespace = crate::molt_dict_new(0);
                    let value = crate::builtins::types::molt_type_new(
                        class,
                        name,
                        bases,
                        namespace,
                        MoltObject::none().bits(),
                    );
                    for bits in [name, bases, namespace] {
                        dec_ref_bits(py, bits);
                    }
                    value
                } else {
                    crate::call::bind::call_bind_borrowed(py, class, None, &[], &[], &[])
                };
                assert!(
                    !crate::exception_pending(py),
                    "managed receiver construction"
                );
                assert_eq!(crate::type_of_bits(py, receiver), class);
                let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(receiver);
                let class_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class);
                assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(class_view).is_some());
                assert!(
                    !(*class_view.cast::<molt_cpython_abi::abi_types::PyTypeObject>())
                        .tp_mro
                        .is_null()
                );
                let name = strings::PyUnicode_FromString(c"managed".as_ptr());
                for delete in [false, true] {
                    let descriptor = if delete {
                        typeobj::attribute_slot_wrapper_for_test::<true>(
                            &raw mut PyBaseObject_Type,
                            object::PyObject_GenericSetAttr,
                        )
                    } else {
                        typeobj::attribute_slot_wrapper_for_test::<false>(
                            &raw mut PyBaseObject_Type,
                            object::PyObject_GenericSetAttr,
                        )
                    };
                    assert!(!descriptor.is_null());
                    let bound = typeobj::PyWrapper_New(descriptor, view);
                    assert!(!bound.is_null());
                    let values = if delete {
                        vec![name]
                    } else {
                        vec![name, &raw mut Py_None]
                    };
                    let args = sequences::PyTuple_New(values.len() as isize);
                    assert!(!args.is_null());
                    for (index, value) in values.into_iter().enumerate() {
                        refcount::Py_INCREF(value);
                        assert_eq!(sequences::PyTuple_SetItem(args, index as isize, value), 0);
                    }
                    let result = object::PyObject_Call(bound, args, ptr::null_mut());
                    if native_override {
                        assert!(result.is_null());
                        assert_native_setter_rejection(delete, "ManagedGenericAttributes");
                    } else {
                        assert_eq!(result, &raw mut Py_None);
                        refcount::Py_DECREF(result);
                    }
                    for value in [args, bound, descriptor] {
                        refcount::Py_DECREF(value);
                    }
                }
                let expected = if native_override { [0, 0] } else { [1, 1] };
                assert_eq!(&CALLS.with(Cell::get)[3..5], &expected);
                assert_eq!(
                    &CALLS.with(Cell::get)[1..3],
                    &[0, 0],
                    "Python overrides are bypassed"
                );
                for value in [name, view] {
                    refcount::Py_DECREF(value);
                }
                for bits in [receiver, class] {
                    dec_ref_bits(py, bits);
                }
                assert!(!crate::exception_pending(py));
            }
        }
    });
}

fn pending_error(py: &crate::PyToken<'_>, kind: &str) -> u64 {
    crate::raise_exception::<u64>(py, kind, "generic descriptor sentinel");
    let error = crate::builtins::exceptions::molt_exception_last_pending();
    crate::molt_exception_clear();
    error
}

#[test]
fn managed_generic_attributes_use_logical_class_and_bypass_override_family() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let classes = crate::builtin_classes(&py);
        for (base, dictless) in [
            (classes.list, false),
            (classes.tuple, false),
            (classes.tuple, true),
        ] {
            CALLS.with(|calls| calls.set([0; 6]));
            FAILURE.with(|failure| failure.set(0));
            let class = attribute_class(&py, base, dictless);
            let bits = crate::call::bind::call_bind_borrowed(&py, class, None, &[], &[], &[]);
            assert!(!crate::exception_pending(&py));
            let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
            let name = strings::PyUnicode_FromString(c"managed".as_ptr());
            let shadow = strings::PyUnicode_FromString(c"shadow".as_ptr());
            let class_name = strings::PyUnicode_FromString(c"__class__".as_ptr());
            let zero =
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_float(0.0).bits());

            assert_eq!(
                take_bits(object::PyObject_GenericGetAttr(view, class_name)),
                class
            );
            assert_eq!(
                take_bits(object::PyObject_GenericGetAttr(view, name)),
                MoltObject::from_int(222).bits()
            );
            assert_eq!(object::PyObject_GenericSetAttr(view, name, zero), 0);
            assert_eq!(
                ASSIGNED.with(Cell::get),
                0,
                "zero bits are an assignment, never deletion"
            );
            assert_eq!(
                object::PyObject_GenericSetAttr(view, name, ptr::null_mut()),
                0
            );
            assert_eq!(CALLS.with(Cell::get), [0, 0, 0, 1, 1, 0]);

            if !dictless {
                assert_eq!(object::PyObject_GenericSetAttr(view, shadow, zero), 0);
                assert_eq!(take_bits(object::PyObject_GenericGetAttr(view, shadow)), 0);
                assert_eq!(
                    object::PyObject_GenericSetAttr(view, shadow, ptr::null_mut()),
                    0
                );
                assert_eq!(
                    take_bits(object::PyObject_GenericGetAttr(view, shadow)),
                    MoltObject::from_int(9).bits()
                );
            }
            assert_eq!(
                take_bits(object::PyObject_GetAttr(view, name)),
                MoltObject::from_int(701).bits()
            );
            assert_eq!(object::PyObject_SetAttr(view, name, zero), 0);
            assert_eq!(
                object::PyObject_SetAttrString(view, c"managed".as_ptr(), ptr::null_mut()),
                0
            );
            assert_eq!(CALLS.with(Cell::get), [1, 1, 1, 1, 1, 0]);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
            for value in [zero, class_name, shadow, name, view] {
                refcount::Py_DECREF(value);
            }
            dec_ref_bits(&py, bits);
            dec_ref_bits(&py, class);
        }
    });
}

#[test]
fn managed_generic_dictionary_replaces_only_instance_tier_and_preserves_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        FAILURE.with(|failure| failure.set(0));
        let class = attribute_class(&py, crate::builtin_classes(&py).list, false);
        let bits = crate::call::bind::call_bind_borrowed(&py, class, None, &[], &[], &[]);
        let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
        let data = strings::PyUnicode_FromString(c"managed".as_ptr());
        let shadow = strings::PyUnicode_FromString(c"shadow".as_ptr());
        let absent = strings::PyUnicode_FromString(c"absent".as_ptr());
        let dictionary = mapping::PyDict_New();
        let value = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_int(44).bits());
        assert_eq!(object::PyObject_GenericSetAttr(view, shadow, value), 0);
        assert_eq!(mapping::PyDict_SetItem(dictionary, data, value), 0);
        assert_eq!(mapping::PyDict_SetItem(dictionary, shadow, value), 0);
        assert_eq!(
            take_bits(object::_PyObject_GenericGetAttrWithDict(
                view, data, dictionary, 0
            )),
            MoltObject::from_int(222).bits()
        );
        assert_eq!(
            take_bits(object::_PyObject_GenericGetAttrWithDict(
                view, shadow, dictionary, 0
            )),
            MoltObject::from_int(44).bits()
        );
        assert_eq!(mapping::PyDict_DelItem(dictionary, shadow), 0);
        assert_eq!(
            take_bits(object::_PyObject_GenericGetAttrWithDict(
                view, shadow, dictionary, 0
            )),
            MoltObject::from_int(9).bits()
        );
        assert_eq!(
            take_bits(object::PyObject_GenericGetAttr(view, shadow)),
            MoltObject::from_int(44).bits()
        );
        assert!(object::_PyObject_GenericGetAttrWithDict(view, absent, dictionary, 1).is_null());
        assert!(errors::PyErr_Occurred().is_null());

        for kind in ["RuntimeError", "AttributeError"] {
            let failure = pending_error(&py, kind);
            FAILURE.with(|slot| slot.set(failure));
            for suppress in [0, 1] {
                assert!(
                    object::_PyObject_GenericGetAttrWithDict(view, data, dictionary, suppress)
                        .is_null()
                );
                if suppress == 1 && kind == "AttributeError" {
                    assert!(errors::PyErr_Occurred().is_null());
                } else {
                    assert_eq!(take_bits(errors::PyErr_GetRaisedException()), failure);
                }
                assert!(!crate::exception_pending(&py));
            }
            assert!(object::PyObject_GenericGetAttr(view, data).is_null());
            assert_eq!(take_bits(errors::PyErr_GetRaisedException()), failure);
            assert_eq!(object::PyObject_GenericSetAttr(view, data, value), -1);
            assert_eq!(take_bits(errors::PyErr_GetRaisedException()), failure);
            assert_eq!(
                object::PyObject_GenericSetAttr(view, data, ptr::null_mut()),
                -1
            );
            assert_eq!(take_bits(errors::PyErr_GetRaisedException()), failure);
            FAILURE.with(|slot| slot.set(0));
            dec_ref_bits(&py, failure);
        }
        for value in [value, dictionary, absent, shadow, data, view] {
            refcount::Py_DECREF(value);
        }
        dec_ref_bits(&py, bits);
        dec_ref_bits(&py, class);
        assert!(!crate::exception_pending(&py));
    });
}

extern "C" fn colliding_hash(_receiver: u64) -> u64 {
    with_gil(|py| {
        let name = crate::attr_name_bits_from_bytes(&py, b"shadow").unwrap();
        let result = crate::molt_hash_builtin(name);
        dec_ref_bits(&py, name);
        result
    })
}

extern "C" fn failing_equality(_receiver: u64, _other: u64) -> u64 {
    record(5);
    crate::molt_raise(FAILURE.with(Cell::get))
}

#[test]
fn managed_generic_suppressed_dictionary_attribute_error_continues_to_class() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        CALLS.with(|calls| calls.set([0; 6]));
        FAILURE.with(|failure| failure.set(0));
        let class = attribute_class(&py, crate::builtin_classes(&py).list, false);
        let bits = crate::call::bind::call_bind_borrowed(&py, class, None, &[], &[], &[]);
        let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
        let key_name = crate::attr_name_bits_from_bytes(&py, b"CollidingAttributeKey").unwrap();
        let key_class = crate::molt_class_new(key_name);
        dec_ref_bits(&py, key_name);
        method(&py, key_class, b"__hash__", colliding_hash as *const (), 1);
        method(&py, key_class, b"__eq__", failing_equality as *const (), 2);
        crate::object::class_finish_definition(
            &py,
            MoltObject::from_bits(key_class).as_ptr().unwrap(),
        )
        .unwrap();
        let key = crate::call::bind::call_bind_borrowed(&py, key_class, None, &[], &[], &[]);
        let dictionary_bits = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
            &py,
            &[key, MoltObject::none().bits()],
        ))
        .bits();
        assert!(!crate::exception_pending(&py));
        let dictionary = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(dictionary_bits);
        let shadow = strings::PyUnicode_FromString(c"shadow".as_ptr());
        crate::object::field_storage::replace_dictionary(
            &py,
            MoltObject::from_bits(bits).as_ptr().unwrap(),
            Some(dictionary_bits),
        );
        for kind in ["AttributeError", "RuntimeError"] {
            let failure = pending_error(&py, kind);
            FAILURE.with(|slot| slot.set(failure));
            for explicit in [ptr::null_mut(), dictionary] {
                for suppress in [0, 1] {
                    let result =
                        object::_PyObject_GenericGetAttrWithDict(view, shadow, explicit, suppress);
                    if kind == "AttributeError" && suppress == 1 {
                        assert_eq!(take_bits(result), MoltObject::from_int(9).bits());
                        assert!(errors::PyErr_Occurred().is_null());
                    } else {
                        assert!(result.is_null());
                        assert_eq!(take_bits(errors::PyErr_GetRaisedException()), failure);
                    }
                }
            }
            FAILURE.with(|slot| slot.set(0));
            dec_ref_bits(&py, failure);
        }
        assert_eq!(
            CALLS.with(Cell::get)[5],
            8,
            "one equality probe per dictionary tier"
        );
        for value in [shadow, dictionary, view] {
            refcount::Py_DECREF(value);
        }
        for value in [dictionary_bits, key, key_class, bits, class] {
            dec_ref_bits(&py, value);
        }
        assert!(!crate::exception_pending(&py));
    });
}

// Variant 0 is explicit object.__getattribute__; 1 and 2 are the C generic
// entrypoints, with the latter replacing only the instance-dictionary tier.
unsafe fn generic_read_variant(
    py: &crate::PyToken<'_>,
    receiver: u64,
    name: u64,
    dictionary: u64,
    variant: u8,
) -> Option<u64> {
    unsafe {
        if variant == 0 {
            let value = crate::molt_object_getattribute(receiver, name);
            if crate::exception_pending(py) {
                dec_ref_bits(py, value);
                return None;
            }
            return Some(value);
        }
        let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(receiver);
        let key = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name);
        let replacement = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(dictionary);
        let result = if variant == 1 {
            object::PyObject_GenericGetAttr(view, key)
        } else {
            object::_PyObject_GenericGetAttrWithDict(view, key, replacement, 0)
        };
        let value = if result.is_null() {
            None
        } else {
            let bits = GLOBAL_BRIDGE
                .observed_handle_for_pyobj(result)
                .unwrap()
                .bits();
            inc_ref_bits(py, bits);
            refcount::Py_DECREF(result);
            Some(bits)
        };
        for value in [replacement, key, view] {
            refcount::Py_DECREF(value);
        }
        value
    }
}

extern "C" fn numeric_attribute_override(_receiver: u64) -> u64 {
    MoltObject::from_int(333).bits()
}

#[test]
fn managed_generic_numeric_subclasses_share_logical_attribute_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        FAILURE.with(|failure| failure.set(0));
        let classes = crate::builtin_classes(&py);
        for (base, input, expected_float) in [
            (classes.int, MoltObject::from_int(42).bits(), 42.0),
            (
                classes.int,
                crate::int_bits_from_i128(&py, 1i128 << 80),
                2_f64.powi(80),
            ),
            (classes.float, MoltObject::from_float(1.25).bits(), 1.25),
            (
                classes.float,
                crate::object::ops::float_result_bits(&py, f64::NAN),
                f64::NAN,
            ),
        ] {
            for dictless in [false, true] {
                let class = attribute_class(&py, base, dictless);
                member(&py, class, b"marker", MoltObject::from_int(55).bits());
                method(
                    &py,
                    class,
                    b"conjugate",
                    numeric_attribute_override as *const (),
                    1,
                );
                let receiver = if base == classes.int {
                    crate::molt_int_new(class, input, crate::missing_bits(&py))
                } else {
                    crate::molt_float_new(class, input)
                };
                assert!(!crate::exception_pending(&py));
                let class_name = crate::attr_name_bits_from_bytes(&py, b"__class__").unwrap();
                let marker = crate::attr_name_bits_from_bytes(&py, b"marker").unwrap();
                let data = crate::attr_name_bits_from_bytes(&py, b"managed").unwrap();
                let shadow = crate::attr_name_bits_from_bytes(&py, b"shadow").unwrap();
                let conjugate = crate::attr_name_bits_from_bytes(&py, b"conjugate").unwrap();
                let float = crate::attr_name_bits_from_bytes(&py, b"__float__").unwrap();
                let dictionary = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
                    &py,
                    &[
                        shadow,
                        MoltObject::from_int(44).bits(),
                        data,
                        MoltObject::from_int(44).bits(),
                    ],
                ))
                .bits();
                if !dictless {
                    crate::molt_object_setattr(receiver, shadow, MoltObject::from_int(33).bits());
                    assert!(!crate::exception_pending(&py));
                }
                for variant in 0..3 {
                    for (name, expected) in [
                        (class_name, class),
                        (marker, MoltObject::from_int(55).bits()),
                        (data, MoltObject::from_int(222).bits()),
                        (
                            shadow,
                            MoltObject::from_int(if variant == 2 {
                                44
                            } else if dictless {
                                9
                            } else {
                                33
                            })
                            .bits(),
                        ),
                    ] {
                        let value =
                            generic_read_variant(&py, receiver, name, dictionary, variant).unwrap();
                        assert_eq!(value, expected);
                        dec_ref_bits(&py, value);
                    }
                    let method =
                        generic_read_variant(&py, receiver, conjugate, dictionary, variant)
                            .unwrap();
                    let value =
                        crate::call::bind::call_bind_borrowed(&py, method, None, &[], &[], &[]);
                    assert_eq!(value, MoltObject::from_int(333).bits());
                    dec_ref_bits(&py, value);
                    dec_ref_bits(&py, method);
                    let method =
                        generic_read_variant(&py, receiver, float, dictionary, variant).unwrap();
                    let value =
                        crate::call::bind::call_bind_borrowed(&py, method, None, &[], &[], &[]);
                    let actual = crate::as_float_extended(MoltObject::from_bits(value));
                    if expected_float.is_nan() {
                        assert!(actual.is_some_and(f64::is_nan));
                    } else {
                        assert_eq!(actual, Some(expected_float));
                    }
                    dec_ref_bits(&py, value);
                    dec_ref_bits(&py, method);
                    assert!(!crate::exception_pending(&py));
                    assert!(errors::PyErr_Occurred().is_null());
                }
                // Every scalar resolver caller must let a heap subtype's
                // normal override run, including synthetic __class__ reads.
                assert_eq!(
                    crate::molt_get_attr_name(receiver, class_name),
                    MoltObject::from_int(701).bits()
                );
                assert_eq!(
                    crate::molt_get_attr_name_default(
                        receiver,
                        class_name,
                        MoltObject::none().bits()
                    ),
                    MoltObject::from_int(701).bits()
                );
                assert_eq!(
                    crate::molt_get_attr_object(
                        receiver,
                        b"__class__".as_ptr(),
                        b"__class__".len() as u64
                    ),
                    MoltObject::from_int(701).bits()
                );
                assert_eq!(
                    crate::molt_has_attr_name(receiver, marker),
                    MoltObject::from_bool(true).bits()
                );
                for value in [
                    dictionary, float, conjugate, shadow, data, marker, class_name, receiver, class,
                ] {
                    dec_ref_bits(&py, value);
                }
            }
            dec_ref_bits(&py, input);
        }
        assert!(!crate::exception_pending(&py));
    });
}

extern "C" fn optional_getattribute(_receiver: u64, _name: u64) -> u64 {
    record(0);
    let failure = FAILURE.with(Cell::get);
    if failure != 0 {
        return crate::molt_raise(failure);
    }
    MoltObject::from_int(701).bits()
}

extern "C" fn optional_getattr(_receiver: u64, _name: u64) -> u64 {
    record(1);
    MoltObject::from_int(702).bits()
}

extern "C" fn optional_index(_receiver: u64) -> u64 {
    record(2);
    MoltObject::from_int(42).bits()
}

#[test]
fn optional_attributes_share_normal_lookup_and_preserve_special_protocols() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let classes = crate::builtin_classes(py);
            let name = crate::attr_name_bits_from_bytes(py, b"__class__").unwrap();
            let index = crate::attr_name_bits_from_bytes(py, b"__index__").unwrap();
            let default = MoltObject::from_int(703).bits();
            for (base, dictless, dataclass) in [
                (classes.object, false, false),
                (classes.object, true, false),
                (classes.object, false, true),
                (classes.list, false, false),
                (classes.tuple, true, false),
                (classes.int, false, false),
                (classes.int, true, false),
                (classes.float, false, false),
                (classes.float, true, false),
            ] {
                CALLS.with(|calls| calls.set([0; 6]));
                FAILURE.with(|failure| failure.set(0));
                let class = attribute_class(py, base, dictless);
                method(
                    py,
                    class,
                    b"__getattribute__",
                    optional_getattribute as *const (),
                    2,
                );
                method(py, class, b"__index__", optional_index as *const (), 1);
                let receiver = snapshot_receiver(py, class, dataclass);
                assert!(!crate::exception_pending(py));
                let pointer = MoltObject::from_bits(receiver).as_ptr().unwrap();

                // Every normal entrypoint uses the same override regardless of
                // payload representation or the presence of instance storage.
                let c_receiver = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(receiver);
                let c_name = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name);
                CALLS.with(|calls| calls.set([0; 6]));
                for result in [
                    crate::molt_get_attr_name(receiver, name),
                    crate::molt_get_attr_name_default(receiver, name, default),
                    crate::molt_get_attr_object(
                        receiver,
                        b"__class__".as_ptr(),
                        b"__class__".len() as u64,
                    ),
                    crate::builtins::attributes::molt_get_attr_object_ic(
                        receiver,
                        b"__class__".as_ptr(),
                        b"__class__".len() as u64,
                        0,
                    ),
                    crate::builtins::attributes::molt_get_attr_generic(
                        pointer,
                        b"__class__".as_ptr(),
                        b"__class__".len() as u64,
                    ),
                    take_bits(object::PyObject_GetAttr(c_receiver, c_name)),
                    crate::builtins::attr::attr_lookup_ptr_allow_missing(py, pointer, name)
                        .unwrap(),
                ] {
                    assert_eq!(result, MoltObject::from_int(701).bits());
                    dec_ref_bits(py, result);
                }
                assert_eq!(
                    crate::molt_has_attr_name(receiver, name),
                    MoltObject::from_bool(true).bits()
                );
                assert_eq!(CALLS.with(Cell::get)[0], 8);
                refcount::Py_DECREF(c_name);
                refcount::Py_DECREF(c_receiver);

                // Implicit protocols use the shared lookup authority in every tier.
                let special =
                    crate::builtins::attr::lookup_special_method_bits(py, receiver, index)
                        .expect("present __index__ protocol");
                let result =
                    crate::call::bind::call_bind_borrowed(py, special, None, &[], &[], &[]);
                assert_eq!(result, MoltObject::from_int(42).bits());
                dec_ref_bits(py, result);
                dec_ref_bits(py, special);
                assert_eq!(&CALLS.with(Cell::get)[0..3], &[8, 0, 1]);
                #[cfg(feature = "stdlib_math")]
                {
                    let mut math_special = MoltObject::none().bits();
                    assert_eq!(
                        crate::math_bridge::__molt_math_lookup_special_method(
                            pointer,
                            index,
                            &mut math_special,
                        ),
                        1
                    );
                    let math_result = crate::call::bind::call_bind_borrowed(
                        py,
                        math_special,
                        None,
                        &[],
                        &[],
                        &[],
                    );
                    assert_eq!(math_result, MoltObject::from_int(42).bits());
                    dec_ref_bits(py, math_result);
                    dec_ref_bits(py, math_special);
                    assert_eq!(&CALLS.with(Cell::get)[0..3], &[8, 0, 2]);
                }
                member(py, class, b"__index__", MoltObject::from_float(0.0).bits());
                assert_eq!(
                    crate::builtins::attr::lookup_special_method_bits(py, receiver, index),
                    Some(0),
                    "a present zero-bit protocol value is not a miss"
                );
                // The optional math satellite projects the same zero-valued fact.
                #[cfg(feature = "stdlib_math")]
                {
                    let mut projected = u64::MAX;
                    assert_eq!(
                        crate::math_bridge::__molt_math_lookup_special_method(
                            pointer,
                            index,
                            &mut projected,
                        ),
                        1
                    );
                    assert_eq!(projected, 0);
                }

                for (kind, missing) in [("AttributeError", true), ("ValueError", false)] {
                    let failure =
                        MoltObject::from_ptr(crate::builtins::exceptions::alloc_exception(
                            py,
                            kind,
                            "optional attribute failure",
                        ))
                        .bits();
                    FAILURE.with(|slot| slot.set(failure));
                    let result = crate::molt_get_attr_name_default(receiver, name, default);
                    if missing {
                        assert_eq!(result, default);
                        assert!(!crate::exception_pending(py));
                    } else {
                        let observed = crate::builtins::exceptions::molt_exception_last_pending();
                        assert_eq!(observed, failure);
                        crate::molt_exception_clear();
                        dec_ref_bits(py, observed);
                    }
                    dec_ref_bits(py, result);
                    FAILURE.with(|slot| slot.set(0));
                    dec_ref_bits(py, failure);
                }

                // __getattr__ runs before the default argument is selected.
                method(py, class, b"__getattr__", optional_getattr as *const (), 2);
                let failure = MoltObject::from_ptr(crate::builtins::exceptions::alloc_exception(
                    py,
                    "AttributeError",
                    "invoke fallback",
                ))
                .bits();
                FAILURE.with(|slot| slot.set(failure));
                let result = crate::molt_get_attr_name_default(receiver, name, default);
                assert_eq!(result, MoltObject::from_int(702).bits());
                assert_eq!(CALLS.with(Cell::get)[1], 1);
                assert!(!crate::exception_pending(py));
                dec_ref_bits(py, result);
                FAILURE.with(|slot| slot.set(0));
                dec_ref_bits(py, failure);
                let failure = MoltObject::from_ptr(crate::builtins::exceptions::alloc_exception(
                    py,
                    "ValueError",
                    "do not invoke fallback",
                ))
                .bits();
                FAILURE.with(|slot| slot.set(failure));
                let result = crate::molt_get_attr_name_default(receiver, name, default);
                let observed = crate::builtins::exceptions::molt_exception_last_pending();
                assert_eq!(observed, failure);
                assert_eq!(CALLS.with(Cell::get)[1], 1);
                crate::molt_exception_clear();
                FAILURE.with(|slot| slot.set(0));
                for value in [result, observed, failure, receiver, class] {
                    dec_ref_bits(py, value);
                }
                assert!(errors::PyErr_Occurred().is_null());
            }
            dec_ref_bits(py, index);
            dec_ref_bits(py, name);
            assert!(!crate::exception_pending(py));
        }
    });
}

thread_local! {
    // Class whose namespace changes, receiver, optional replacement class,
    // and replacement value. The fixture owns all of these references.
    static SNAPSHOT_MUTATION: Cell<[u64; 4]> = const { Cell::new([0; 4]) };
}

fn snapshot_class(py: &crate::PyToken<'_>, name: &[u8]) -> u64 {
    let name = crate::attr_name_bits_from_bytes(py, name).unwrap();
    let class = crate::molt_class_new(name);
    dec_ref_bits(py, name);
    crate::molt_class_set_base(class, crate::builtin_classes(py).object);
    member(py, class, b"cache_flush", MoltObject::from_int(7).bits());
    unsafe {
        crate::object::class_finish_definition(py, MoltObject::from_bits(class).as_ptr().unwrap())
            .unwrap();
    }
    class
}

fn snapshot_receiver(py: &crate::PyToken<'_>, class: u64, dataclass: bool) -> u64 {
    if !dataclass {
        return unsafe { crate::call::bind::call_bind_borrowed(py, class, None, &[], &[], &[]) };
    }
    let name = crate::attr_name_bits_from_bytes(py, b"SnapshotRecord").unwrap();
    let empty = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
    let receiver = crate::molt_dataclass_new(name, empty, empty, MoltObject::from_int(0).bits());
    crate::molt_dataclass_set_class(receiver, class);
    dec_ref_bits(py, empty);
    dec_ref_bits(py, name);
    assert!(!crate::exception_pending(py));
    receiver
}

extern "C" fn mutating_equality(_receiver: u64, _other: u64) -> u64 {
    record(5);
    with_gil(|py| {
        let [class, receiver, next_class, value] = SNAPSHOT_MUTATION.with(Cell::get);
        let name = crate::attr_name_bits_from_bytes(&py, b"shadow").unwrap();
        crate::molt_set_attr_name(class, name, value);
        dec_ref_bits(&py, name);
        if next_class != 0 {
            let name = crate::attr_name_bits_from_bytes(&py, b"__class__").unwrap();
            crate::molt_object_setattr(receiver, name, next_class);
            dec_ref_bits(&py, name);
        }
        // A real nested lookup retires the cache's descriptor reference. The
        // interrupted lookup must own its original selection independently.
        let flush = crate::attr_name_bits_from_bytes(&py, b"cache_flush").unwrap();
        let result = crate::molt_object_getattribute(receiver, flush);
        dec_ref_bits(&py, result);
        dec_ref_bits(&py, flush);
        MoltObject::from_bool(false).bits()
    })
}

fn mutating_dictionary(py: &crate::PyToken<'_>) -> u64 {
    let class = snapshot_class(py, b"MutatingAttributeKey");
    method(py, class, b"__hash__", colliding_hash as *const (), 1);
    method(py, class, b"__eq__", mutating_equality as *const (), 2);
    let key = unsafe { crate::call::bind::call_bind_borrowed(py, class, None, &[], &[], &[]) };
    let dictionary = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
        py,
        &[key, MoltObject::none().bits()],
    ))
    .bits();
    dec_ref_bits(py, key);
    dec_ref_bits(py, class);
    assert!(!crate::exception_pending(py));
    dictionary
}

#[test]
fn managed_generic_dictionary_mutation_retains_initial_class_value_and_miss() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        for dataclass in [false, true] {
            for present in [false, true] {
                for variant in 0..3 {
                    CALLS.with(|calls| calls.set([0; 6]));
                    let class = snapshot_class(&py, b"SnapshotOwner");
                    let old = crate::attr_name_bits_from_bytes(&py, b"old").unwrap();
                    let new = crate::attr_name_bits_from_bytes(&py, b"new").unwrap();
                    let name = crate::attr_name_bits_from_bytes(&py, b"shadow").unwrap();
                    if present {
                        member(&py, class, b"shadow", old);
                    }
                    let receiver = snapshot_receiver(&py, class, dataclass);
                    let dictionary = mutating_dictionary(&py);
                    if variant != 2 {
                        crate::object::field_storage::replace_dictionary(
                            &py,
                            MoltObject::from_bits(receiver).as_ptr().unwrap(),
                            Some(dictionary),
                        );
                    }
                    SNAPSHOT_MUTATION.with(|context| context.set([class, receiver, 0, new]));
                    let result = generic_read_variant(&py, receiver, name, dictionary, variant);
                    if present {
                        assert_eq!(result, Some(old));
                        dec_ref_bits(&py, result.unwrap());
                    } else {
                        assert!(
                            result.is_none(),
                            "the initial MRO miss must survive dictionary mutation"
                        );
                        if variant == 0 {
                            assert!(crate::builtins::attr::clear_attribute_error_if_pending(&py));
                        } else {
                            assert_eq!(
                                errors::PyErr_ExceptionMatches(
                                    (&raw mut molt_cpython_abi::abi_types::PyExc_AttributeError)
                                        .cast::<PyObject>(),
                                ),
                                1,
                            );
                            errors::PyErr_Clear();
                        }
                    }
                    assert_eq!(
                        CALLS.with(Cell::get)[5],
                        1,
                        "one dictionary equality probe per lookup"
                    );
                    assert!(!crate::exception_pending(&py));
                    assert!(errors::PyErr_Occurred().is_null());
                    SNAPSHOT_MUTATION.with(|context| context.set([0; 4]));
                    for value in [dictionary, receiver, name, new, old, class] {
                        dec_ref_bits(&py, value);
                    }
                }
            }
        }
    });
}

extern "C" fn snapshot_descriptor_get(descriptor: u64, _receiver: u64, owner: u64) -> u64 {
    ASSIGNED.with(|assigned| assigned.set(descriptor));
    with_gil(|py| {
        inc_ref_bits(&py, owner);
        owner
    })
}

#[test]
fn managed_generic_dictionary_mutation_binds_retained_descriptor_to_live_class() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        for variant in 0..3 {
            CALLS.with(|calls| calls.set([0; 6]));
            ASSIGNED.with(|assigned| assigned.set(0));
            let class = snapshot_class(&py, b"InitialDescriptorOwner");
            let next_class = snapshot_class(&py, b"CurrentDescriptorOwner");
            let descriptor_class = snapshot_class(&py, b"RetainedDescriptor");
            method(
                &py,
                descriptor_class,
                b"__get__",
                snapshot_descriptor_get as *const (),
                3,
            );
            let descriptor =
                crate::call::bind::call_bind_borrowed(&py, descriptor_class, None, &[], &[], &[]);
            member(&py, class, b"shadow", descriptor);
            dec_ref_bits(&py, descriptor);
            let receiver = snapshot_receiver(&py, class, false);
            let dictionary = mutating_dictionary(&py);
            if variant != 2 {
                crate::object::field_storage::replace_dictionary(
                    &py,
                    MoltObject::from_bits(receiver).as_ptr().unwrap(),
                    Some(dictionary),
                );
            }
            let name = crate::attr_name_bits_from_bytes(&py, b"shadow").unwrap();
            SNAPSHOT_MUTATION.with(|context| {
                context.set([class, receiver, next_class, MoltObject::from_int(99).bits()])
            });
            let result = generic_read_variant(&py, receiver, name, dictionary, variant).unwrap();
            assert_eq!(
                result, next_class,
                "__get__ receives the receiver's current class"
            );
            assert_eq!(
                ASSIGNED.with(Cell::get),
                descriptor,
                "the original descriptor survives replacement and cache retirement"
            );
            assert_eq!(CALLS.with(Cell::get)[5], 1);
            assert!(!crate::exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());
            SNAPSHOT_MUTATION.with(|context| context.set([0; 4]));
            ASSIGNED.with(|assigned| assigned.set(0));
            for value in [
                result,
                name,
                dictionary,
                receiver,
                descriptor_class,
                next_class,
                class,
            ] {
                dec_ref_bits(&py, value);
            }
        }
    });
}

#[test]
fn managed_generic_descriptor_cache_keeps_lossless_name_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        for dataclass in [false, true] {
            let class = snapshot_class(&py, b"LosslessAttributeOwner");
            let spellings: [&[u8]; 5] = [
                b"n\xed\xa0\x80\0m",
                b"n\xed\xa0\x81\0m",
                b"n\0m",
                b"n\0s",
                b"",
            ];
            let mut names = Vec::new();
            for (index, spelling) in spellings.into_iter().enumerate() {
                member(
                    &py,
                    class,
                    spelling,
                    MoltObject::from_int(index as i64).bits(),
                );
                names.push(crate::attr_name_bits_from_bytes(&py, spelling).unwrap());
            }
            let receiver = snapshot_receiver(&py, class, dataclass);
            let dictionary = MoltObject::from_ptr(crate::alloc_dict_with_pairs(&py, &[])).bits();
            for variant in 0..3 {
                for index in [0, 0, 1, 1, 0, 2, 3, 4, 1] {
                    let result =
                        generic_read_variant(&py, receiver, names[index], dictionary, variant)
                            .unwrap();
                    assert_eq!(result, MoltObject::from_int(index as i64).bits());
                    dec_ref_bits(&py, result);
                }
            }
            for name in names {
                dec_ref_bits(&py, name);
            }
            for value in [dictionary, receiver, class] {
                dec_ref_bits(&py, value);
            }
            assert!(!crate::exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

mod mutation_consumers;

mod mutation_name_identity;

mod mutation_native_slots;
