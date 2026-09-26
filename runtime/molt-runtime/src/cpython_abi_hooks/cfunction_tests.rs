//! Exercise C convention transport and managed calls through the real runtime binder.

use super::*;
use molt_cpython_abi::abi_types::{METH_O, METH_STATIC, METH_VARARGS};
use molt_cpython_abi::api::{mapping, numbers as numeric, sequences, strings};
use std::cell::RefCell;

#[test]
fn managed_exception_class_creation_and_calls_use_runtime_authority() {
    use molt_cpython_abi::abi_types::{PyExc_ValueError, PyType_Type};
    use molt_cpython_abi::api::{errors, object, refcount, typeobj};

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    unsafe {
        // PyErr_NewException calls the canonical type(name, bases, dict),
        // then its result must itself be callable through the C API.
        let class = errors::PyErr_NewException(
            c"extension.ManagedError".as_ptr(),
            (&raw mut PyExc_ValueError).cast(),
            ptr::null_mut(),
        );
        assert!(
            !class.is_null(),
            "runtime-backed type() must create heap classes"
        );
        assert!(errors::PyErr_Occurred().is_null());
        assert_eq!(typeobj::PyCallable_Check(class), 1);
        let message = strings::PyUnicode_FromString(c"managed class call".as_ptr());
        assert!(!message.is_null());
        let instance = object::PyObject_CallOneArg(class, message);
        assert!(
            !instance.is_null(),
            "managed heap class must use runtime construction"
        );
        let semantic_type = typeobj::PyObject_Type(instance);
        assert_eq!(typeobj::PyCallable_Check(instance), 0);
        assert_eq!(semantic_type, class);
        refcount::Py_DECREF(semantic_type);
        let type_call = object::PyObject_CallOneArg((&raw mut PyType_Type).cast(), instance);
        assert_eq!(type_call, class, "type(x) and PyObject_Type(x) must agree");
        refcount::Py_DECREF(type_call);
        let text = typeobj::PyObject_Str(instance);
        assert!(!text.is_null());
        let utf8 = strings::PyUnicode_AsUTF8(text);
        assert!(!utf8.is_null());
        assert_eq!(CStr::from_ptr(utf8), c"managed class call");
        let representation = typeobj::PyObject_Repr(instance);
        assert!(!representation.is_null());
        let utf8 = strings::PyUnicode_AsUTF8(representation);
        assert!(!utf8.is_null());
        assert_eq!(CStr::from_ptr(utf8), c"ManagedError('managed class call')");
        let escaped = strings::PyUnicode_FromString(c"line\n\\tail".as_ptr());
        assert!(!escaped.is_null());
        let escaped_repr = typeobj::PyObject_Repr(escaped);
        assert!(!escaped_repr.is_null());
        let utf8 = strings::PyUnicode_AsUTF8(escaped_repr);
        assert!(!utf8.is_null());
        assert_eq!(CStr::from_ptr(utf8), c"'line\\n\\\\tail'");
        assert!(errors::PyErr_Occurred().is_null());
        for value in [
            escaped_repr,
            escaped,
            representation,
            text,
            instance,
            message,
            class,
        ] {
            refcount::Py_DECREF(value);
        }
    }
}

#[derive(Debug, PartialEq)]
struct ObservedCall {
    null_self: bool,
    receiver: Option<u64>,
    defining_class: Option<u64>,
    positional: Vec<i64>,
    keywords: Vec<(String, i64)>,
}

thread_local! {
    static OBSERVED: RefCell<Option<ObservedCall>> = const { RefCell::new(None) };
}

unsafe fn keyword_pair(name: *mut PyObject, value: *mut PyObject) -> (String, i64) {
    let name = unsafe { strings::PyUnicode_AsUTF8(name) };
    let name = if name.is_null() {
        "<invalid keyword>".to_owned()
    } else {
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    (name, unsafe { numeric::PyLong_AsLongLong(value) })
}

unsafe fn capture_vector(
    receiver: *mut PyObject,
    class: *mut PyTypeObject,
    args: *mut *mut PyObject,
    count: usize,
    names: *mut PyObject,
) -> *mut PyObject {
    let positional = (0..count)
        .map(|i| unsafe { numeric::PyLong_AsLongLong(*args.add(i)) })
        .collect();
    let keyword_count = if names.is_null() {
        0
    } else {
        unsafe { sequences::PyTuple_Size(names) }.max(0) as usize
    };
    let keywords = (0..keyword_count)
        .map(|i| unsafe {
            keyword_pair(
                sequences::PyTuple_GetItem(names, i as Py_ssize_t),
                *args.add(count + i),
            )
        })
        .collect();
    OBSERVED.with(|slot| {
        *slot.borrow_mut() = Some(ObservedCall {
            null_self: receiver.is_null(),
            receiver: molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(receiver)
                .map(|handle| handle.bits()),
            defining_class: molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(class.cast())
                .map(|handle| handle.bits()),
            positional,
            keywords,
        })
    });
    unsafe { numeric::PyLong_FromLongLong(197) }
}

unsafe extern "C" fn noargs(receiver: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    unsafe {
        capture_vector(
            receiver,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
        )
    }
}

unsafe extern "C" fn one(receiver: *mut PyObject, mut arg: *mut PyObject) -> *mut PyObject {
    unsafe { capture_vector(receiver, ptr::null_mut(), &raw mut arg, 1, ptr::null_mut()) }
}

unsafe extern "C" fn fastcall(
    receiver: *mut PyObject,
    args: *mut *mut PyObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    unsafe {
        capture_vector(
            receiver,
            ptr::null_mut(),
            args,
            count as usize,
            ptr::null_mut(),
        )
    }
}

unsafe extern "C" fn fastcall_keywords(
    receiver: *mut PyObject,
    args: *mut *mut PyObject,
    count: Py_ssize_t,
    names: *mut PyObject,
) -> *mut PyObject {
    unsafe { capture_vector(receiver, ptr::null_mut(), args, count as usize, names) }
}

unsafe extern "C" fn method(
    receiver: *mut PyObject,
    class: *mut PyTypeObject,
    args: *mut *mut PyObject,
    count: usize,
    names: *mut PyObject,
) -> *mut PyObject {
    unsafe { capture_vector(receiver, class, args, count, names) }
}

unsafe extern "C" fn varargs(receiver: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    unsafe { varargs_keywords(receiver, args, ptr::null_mut()) }
}

unsafe extern "C" fn varargs_keywords(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let count = unsafe { sequences::PyTuple_Size(args) }.max(0) as usize;
    let mut values: Vec<_> = (0..count)
        .map(|i| unsafe { sequences::PyTuple_GetItem(args, i as Py_ssize_t) })
        .collect();
    let result = unsafe {
        capture_vector(
            receiver,
            ptr::null_mut(),
            values.as_mut_ptr(),
            count,
            ptr::null_mut(),
        )
    };
    if !kwargs.is_null() {
        let mut position = 0;
        let mut key = ptr::null_mut();
        let mut value = ptr::null_mut();
        let mut keywords = Vec::new();
        while unsafe {
            mapping::PyDict_Next(kwargs, &raw mut position, &raw mut key, &raw mut value)
        } != 0
        {
            keywords.push(unsafe { keyword_pair(key, value) });
        }
        OBSERVED.with(|slot| slot.borrow_mut().as_mut().unwrap().keywords = keywords);
    }
    result
}

unsafe fn register(target: *const (), flags: c_int, class: u64) -> u64 {
    unsafe {
        hook_register_c_function(
            crate::provenance::abi::expose_function_address(target),
            flags,
            MoltObject::none().bits(),
            false,
            class,
            b"convention_probe".as_ptr(),
            b"convention_probe".len(),
        )
    }
}

fn bind(function: u64, positional: &[i64], keywords: &[(&str, i64)]) -> u64 {
    let builder = crate::molt_callargs_new(positional.len() as u64, keywords.len() as u64);
    assert_ne!(builder, 0);
    for &value in positional {
        unsafe { crate::molt_callargs_push_pos(builder, MoltObject::from_int(value).bits()) };
    }
    for &(name, value) in keywords {
        with_gil(|py| unsafe {
            let name = alloc_string(&py, name.as_bytes());
            assert!(!name.is_null());
            let name = MoltObject::from_ptr(name).bits();
            crate::molt_callargs_push_kw(builder, name, MoltObject::from_int(value).bits());
            dec_ref_bits(&py, name);
        });
    }
    crate::molt_call_bind(function, builder)
}

fn defining_class() -> u64 {
    with_gil(|py| {
        let name = alloc_string(&py, b"DefiningClass");
        assert!(!name.is_null());
        let name_bits = MoltObject::from_ptr(name).bits();
        let class = crate::object::builders::alloc_class_obj(&py, name_bits);
        dec_ref_bits(&py, name_bits);
        assert!(!class.is_null());
        MoltObject::from_ptr(class).bits()
    })
}

unsafe fn assert_callable_class_identity(callable: *mut PyObject, is_method: bool) {
    use molt_cpython_abi::abi_types::{PyCFunction_Type, PyCMethod_Type, PyType_Type};
    use molt_cpython_abi::api::{object, refcount, typeobj};

    with_gil(|py| unsafe {
        let classes = crate::builtin_classes(&py);
        let expected_class = if is_method {
            classes.builtin_method
        } else {
            classes.builtin_function_or_method
        };
        let expected_view = if is_method {
            &raw mut PyCMethod_Type
        } else {
            &raw mut PyCFunction_Type
        };
        let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(callable)
            .expect("callable runtime identity")
            .bits();
        assert_eq!(crate::type_of_bits(&py, bits), expected_class);
        assert_eq!((*callable).ob_type, expected_view);
        let reported = typeobj::PyObject_Type(callable);
        assert_eq!(reported, expected_view.cast());
        refcount::Py_DECREF(reported);
        let reported = object::PyObject_CallOneArg((&raw mut PyType_Type).cast(), callable);
        assert_eq!(reported, expected_view.cast());
        refcount::Py_DECREF(reported);
        assert_eq!(
            typeobj::PyObject_TypeCheck(callable, &raw mut PyCFunction_Type),
            1,
        );
        assert_eq!(
            typeobj::PyObject_TypeCheck(callable, &raw mut PyCMethod_Type),
            i32::from(is_method),
        );
        for hidden in [b"__code__".as_slice(), b"__closure__".as_slice()] {
            let name = alloc_string(&py, hidden);
            assert!(!name.is_null());
            let name_bits = MoltObject::from_ptr(name).bits();
            let result = hook_object_get_attr(bits, name_bits);
            dec_ref_bits(&py, name_bits);
            assert!(matches!(result.decode(), DecodedHandleResult::Error));
            assert!(crate::exception_pending(&py));
            crate::clear_exception(&py);
        }
        assert!(!crate::exception_pending(&py));
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn runtime_builtin_callable_has_safe_semantic_vectorcall_storage() {
    use molt_cpython_abi::abi_types::{
        MoltManaged_Type, Py_TPFLAGS_HAVE_VECTORCALL, PyCFunction_Type, PyVectorcallFunc,
    };
    use molt_cpython_abi::api::{object, refcount, typeobj};

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        // A genuine runtime builtin has no PyMethodDef/native C callback.
        let bits = crate::builtins::functions::python_builtin_function_bits(&py, "len")
            .expect("canonical len builtin");
        assert_eq!(hook_classify_heap(bits), MoltTypeTag::BuiltinCallable as u8);
        let callable = molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits);
        assert!(!callable.is_null());
        assert_eq!((*callable).ob_type, &raw mut MoltManaged_Type);
        let semantic = typeobj::PyObject_Type(callable);
        assert_eq!(semantic, (&raw mut PyCFunction_Type).cast());
        assert_eq!(typeobj::PyCallable_Check(callable), 1);
        assert!(
            object::PyCFunction_GetFunction(callable).is_none(),
            "a runtime callable must not fabricate a native method pointer",
        );
        assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        molt_cpython_abi::api::errors::PyErr_Clear();

        // This deliberately mirrors a Cython extension's semantic Py_TYPE
        // vectorcall probe, not our own storage-aware API accessor.
        let class = semantic.cast::<PyTypeObject>();
        assert_ne!((*class).tp_flags & Py_TPFLAGS_HAVE_VECTORCALL, 0);
        let offset = (*class).tp_vectorcall_offset;
        assert!(offset > 0);
        let vector = ptr::read_unaligned(
            callable
                .cast::<u8>()
                .add(offset as usize)
                .cast::<Option<PyVectorcallFunc>>(),
        )
        .expect("semantic vectorcall offset addresses a real initialized field");
        let value = strings::PyUnicode_FromString(c"abc".as_ptr());
        assert!(!value.is_null());
        let mut arguments = [value];
        let result = vector(callable, arguments.as_mut_ptr(), 1, ptr::null_mut());
        assert!(!result.is_null());
        assert_eq!(numeric::PyLong_AsLongLong(result), 3);
        refcount::Py_DECREF(result);
        // A consumer invoking the advertised tp_call has the same authority.
        let tuple = sequences::PyTuple_New(1);
        assert!(!tuple.is_null());
        refcount::Py_INCREF(value);
        assert_eq!(sequences::PyTuple_SetItem(tuple, 0, value), 0);
        let result = object::PyVectorcall_Call(callable, tuple, ptr::null_mut());
        assert!(
            !result.is_null(),
            "public vectorcall entry uses managed storage"
        );
        assert_eq!(numeric::PyLong_AsLongLong(result), 3);
        refcount::Py_DECREF(result);
        let result =
            ((*class).tp_call.expect("callable class tp_call"))(callable, tuple, ptr::null_mut());
        assert!(!result.is_null());
        assert_eq!(numeric::PyLong_AsLongLong(result), 3);
        let value_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(value)
            .expect("string runtime identity")
            .bits();
        let bound_bits = crate::molt_bound_method_new(bits, value_bits);
        assert!(!crate::exception_pending(&py));
        assert_eq!(
            hook_classify_heap(bound_bits),
            MoltTypeTag::BuiltinCallable as u8
        );
        let bound = molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bound_bits);
        assert!(!bound.is_null());
        let bound_vector = ptr::read_unaligned(
            bound
                .cast::<u8>()
                .add(offset as usize)
                .cast::<Option<PyVectorcallFunc>>(),
        )
        .expect("bound builtin has the same complete callable carrier");
        let bound_result = bound_vector(bound, ptr::null_mut(), 0, ptr::null_mut());
        assert!(!bound_result.is_null());
        assert_eq!(numeric::PyLong_AsLongLong(bound_result), 3);
        refcount::Py_DECREF(bound_result);
        refcount::Py_DECREF(bound);
        for value in [result, tuple, value, semantic, callable] {
            refcount::Py_DECREF(value);
        }
        assert!(!crate::exception_pending(&py));
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn cext_all_conventions_use_runtime_binding_and_ordered_keyword_transport() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    register_cpython_hooks();
    let class = defining_class();
    let cases = [
        (METH_NOARGS, noargs as *const (), 0, false),
        (METH_O, one as *const (), 1, false),
        (METH_VARARGS, varargs as *const (), 3, false),
        (
            METH_VARARGS | METH_KEYWORDS,
            varargs_keywords as *const (),
            3,
            true,
        ),
        (METH_FASTCALL, fastcall as *const (), 3, false),
        (
            METH_FASTCALL | METH_KEYWORDS,
            fastcall_keywords as *const (),
            3,
            true,
        ),
        (
            METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
            method as *const (),
            3,
            true,
        ),
        (METH_NOARGS | METH_STATIC, noargs as *const (), 0, false),
    ];
    for (flags, target, count, accepts_keywords) in cases {
        let defining_class = if flags & METH_METHOD != 0 {
            class
        } else {
            MoltObject::none().bits()
        };
        let function = unsafe { register(target, flags, defining_class) };
        assert_ne!(function, 0, "flags={flags:#x}");
        with_gil(|py| {
            let classes = crate::builtin_classes(&py);
            let actual = crate::type_of_bits(&py, function);
            assert_eq!(
                actual,
                if flags & METH_METHOD != 0 {
                    classes.builtin_method
                } else {
                    classes.builtin_function_or_method
                },
            );
            assert!(classes.is_builtin_callable_class(actual));
        });
        // Empty keyword spans and tuple/vector adapters hit real consumers.
        for keywords in [&[][..], &[("zeta", 40), ("alpha", 50)][..]] {
            if !accepts_keywords && !keywords.is_empty() {
                continue;
            }
            OBSERVED.with(|slot| *slot.borrow_mut() = None);
            let result = bind(function, &[10, 20, 30][..count], keywords);
            assert!(
                !with_gil(|py| crate::exception_pending(&py)),
                "flags={flags:#x}"
            );
            assert_eq!(MoltObject::from_bits(result).as_int(), Some(197));
            let observed = OBSERVED
                .with(|slot| slot.borrow_mut().take())
                .expect("callback executed");
            assert_eq!(
                observed,
                ObservedCall {
                    null_self: flags & METH_STATIC != 0,
                    receiver: (flags & METH_STATIC == 0).then_some(MoltObject::none().bits()),
                    defining_class: (flags & METH_METHOD != 0).then_some(class),
                    positional: vec![10, 20, 30][..count].to_vec(),
                    keywords: keywords
                        .iter()
                        .map(|&(key, value)| (key.to_owned(), value))
                        .collect(),
                },
                "flags={flags:#x}"
            );
            unsafe { hook_dec_ref(result) };
        }
        if !accepts_keywords {
            OBSERVED.with(|slot| *slot.borrow_mut() = None);
            let result = bind(function, &[10, 20, 30][..count], &[("unexpected", 2)]);
            assert!(with_gil(|py| crate::exception_pending(&py)));
            assert!(OBSERVED.with(|slot| slot.borrow().is_none()));
            unsafe { hook_dec_ref(result) };
            let _ = crate::molt_exception_clear();
            unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        }
        if flags & (METH_NOARGS | METH_O) != 0 {
            OBSERVED.with(|slot| *slot.borrow_mut() = None);
            let result = bind(function, &[1, 2], &[]);
            assert!(with_gil(|py| crate::exception_pending(&py)));
            assert!(OBSERVED.with(|slot| slot.borrow().is_none()));
            unsafe { hook_dec_ref(result) };
            let _ = crate::molt_exception_clear();
            unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        }
        if accepts_keywords {
            let result = bind(function, &[1, 2, 3, 4, 5, 6, 7, 8, 9], &[("tail", 10)]);
            assert!(!with_gil(|py| crate::exception_pending(&py)));
            assert_eq!(MoltObject::from_bits(result).as_int(), Some(197));
            let observed = OBSERVED.with(|slot| slot.borrow_mut().take()).unwrap();
            assert_eq!(observed.positional, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
            assert_eq!(observed.keywords, vec![("tail".to_owned(), 10)]);
            unsafe { hook_dec_ref(result) };
        }
        unsafe { hook_dec_ref(function) };
    }
    unsafe { hook_dec_ref(class) };
}

#[test]
fn cext_method_defining_class_is_validated_and_traced() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    register_cpython_hooks();
    with_gil(|py| unsafe {
        let class = defining_class();
        let class_ptr = MoltObject::from_bits(class).as_ptr().unwrap();
        let flags = METH_METHOD | METH_FASTCALL | METH_KEYWORDS;
        let count = with_cext_callable_registry(|registry| registry.len());
        for (bad_flags, bad_class) in [
            (flags, MoltObject::none().bits()),
            (flags, MoltObject::from_int(7).bits()),
            (METH_NOARGS, class),
        ] {
            assert_eq!(register(method as *const (), bad_flags, bad_class), 0);
            assert!(crate::exception_pending(&py));
            crate::clear_exception(&py);
            assert_eq!(
                with_cext_callable_registry(|registry| registry.len()),
                count
            );
        }
        let baseline = (*header_from_obj_ptr(class_ptr)).ref_count_snapshot();
        let function = register(method as *const (), flags, class);
        assert_ne!(function, 0);
        let function_ptr = MoltObject::from_bits(function).as_ptr().unwrap();
        let closure = crate::function_execution_closure_bits(function_ptr);
        let context = CExtCallableContext::from_bits(closure).unwrap();
        assert_eq!(context.defining_class, class);
        let mut class_edges = 0;
        crate::object::heap_lifecycle::visit_owned_edges(
            &py,
            MoltObject::from_bits(closure).as_ptr().unwrap(),
            &mut |child| {
                class_edges += usize::from(child == class_ptr);
            },
        );
        assert_eq!(class_edges, 1);
        assert_eq!(
            (*header_from_obj_ptr(class_ptr)).ref_count_snapshot(),
            baseline + 1
        );
        dec_ref_bits(&py, function);
        assert_eq!(
            (*header_from_obj_ptr(class_ptr)).ref_count_snapshot(),
            baseline
        );
        dec_ref_bits(&py, class);
    });
}

thread_local! {
    static ORIGINAL_ERROR: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

unsafe extern "C" fn runtime_error_with_tuple(
    _receiver: *mut PyObject,
    _args: *mut PyObject,
) -> *mut PyObject {
    with_gil(|py| {
        crate::raise_exception::<u64>(&py, "ValueError", "runtime-only C convention error");
        let bits = crate::exception_last_bits_noinc(&py).unwrap();
        inc_ref_bits(&py, bits);
        ORIGINAL_ERROR.with(|slot| slot.set(bits));
    });
    ptr::null_mut()
}

#[test]
fn cext_tuple_adapter_preserves_exact_runtime_only_exception() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    register_cpython_hooks();
    let function = unsafe {
        register(
            runtime_error_with_tuple as *const (),
            METH_VARARGS,
            MoltObject::none().bits(),
        )
    };
    assert_ne!(function, 0);
    let result = bind(function, &[1, 2], &[]);
    assert_eq!(result, 0);
    let original = ORIGINAL_ERROR.with(|slot| slot.replace(0));
    assert_ne!(original, 0);
    with_gil(|py| {
        assert_eq!(crate::exception_last_bits_noinc(&py), Some(original));
        crate::clear_exception(&py);
        dec_ref_bits(&py, original);
        dec_ref_bits(&py, function);
    });
}

#[test]
fn cext_raw_cmethod_releases_runtime_closure_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    register_cpython_hooks();
    with_gil(|py| unsafe {
        let class = defining_class();
        let receiver = alloc_dict_with_pairs(&py, &[]);
        assert!(!receiver.is_null());
        let receiver_bits = MoltObject::from_ptr(receiver).bits();
        let class_ptr = MoltObject::from_bits(class).as_ptr().unwrap();
        let receiver_view = cext_new_pyobject_from_borrowed_bits(receiver_bits);
        let class_view = cext_new_pyobject_from_borrowed_bits(class);
        assert!(!receiver_view.is_null() && !class_view.is_null());
        let receiver_refs = (*header_from_obj_ptr(receiver)).ref_count_snapshot();
        let class_refs = (*header_from_obj_ptr(class_ptr)).ref_count_snapshot();
        let mut definition = molt_cpython_abi::abi_types::PyMethodDef {
            ml_name: c"owned_method".as_ptr(),
            ml_meth: Some(std::mem::transmute::<
                *const (),
                molt_cpython_abi::abi_types::PyCFunction,
            >(method as *const ())),
            ml_flags: METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
            ml_doc: ptr::null(),
        };
        let callable = molt_cpython_abi::api::object::PyCMethod_New(
            &raw mut definition,
            receiver_view,
            ptr::null_mut(),
            class_view.cast(),
        );
        assert!(!callable.is_null());
        assert_callable_class_identity(callable, true);
        assert_eq!(
            molt_cpython_abi::api::object::PyCFunction_GetFunction(callable)
                .map(|function| function as *const ()),
            Some(method as *const ()),
        );
        assert_eq!(
            molt_cpython_abi::api::object::PyCFunction_GetSelf(callable),
            receiver_view,
        );
        assert_eq!(
            molt_cpython_abi::api::object::PyCFunction_GetFlags(callable),
            definition.ml_flags,
        );
        let function = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(callable)
            .expect("runtime function published")
            .bits();
        let result = bind(function, &[10], &[("argument", 20)]);
        assert_eq!(MoltObject::from_bits(result).as_int(), Some(197));
        dec_ref_bits(&py, result);
        molt_cpython_abi::api::refcount::Py_DECREF(callable);
        assert_eq!(
            (*header_from_obj_ptr(receiver)).ref_count_snapshot(),
            receiver_refs,
            "managed callable retirement must release both ABI receiver and runtime closure edges"
        );
        assert_eq!(
            (*header_from_obj_ptr(class_ptr)).ref_count_snapshot(),
            class_refs,
            "managed CMethod retirement must release both defining-class edges"
        );
        molt_cpython_abi::api::refcount::Py_DECREF(receiver_view);
        molt_cpython_abi::api::refcount::Py_DECREF(class_view);
        dec_ref_bits(&py, receiver_bits);
        dec_ref_bits(&py, class);
    });
}

#[test]
fn cext_null_none_and_zero_receivers_remain_distinct_across_raw_and_runtime_calls() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    register_cpython_hooks();
    unsafe {
        let zero = numeric::PyFloat_FromDouble(0.0);
        assert!(!zero.is_null());
        for (receiver, expected) in [
            (ptr::null_mut(), None),
            (
                &raw mut molt_cpython_abi::abi_types::Py_None,
                Some(MoltObject::none().bits()),
            ),
            (zero, Some(MoltObject::from_float(0.0).bits())),
        ] {
            let mut definition = molt_cpython_abi::abi_types::PyMethodDef {
                ml_name: c"nullable_receiver".as_ptr(),
                ml_meth: Some(noargs),
                ml_flags: METH_NOARGS,
                ml_doc: ptr::null(),
            };
            let callable =
                molt_cpython_abi::api::object::PyCFunction_New(&raw mut definition, receiver);
            assert!(!callable.is_null());
            assert_callable_class_identity(callable, false);
            assert_eq!(
                molt_cpython_abi::api::object::PyCFunction_GetFunction(callable)
                    .map(|function| function as *const ()),
                Some(noargs as *const ()),
            );
            assert_eq!(
                molt_cpython_abi::api::object::PyCFunction_GetSelf(callable),
                receiver,
            );
            let result = molt_cpython_abi::api::object::PyObject_Vectorcall(
                callable,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
            );
            assert!(!result.is_null());
            molt_cpython_abi::api::refcount::Py_DECREF(result);
            let raw = OBSERVED.with(|slot| slot.borrow_mut().take()).unwrap();
            assert_eq!(raw.receiver, expected);
            assert_eq!(raw.null_self, receiver.is_null());
            let function = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(callable)
                .unwrap()
                .bits();
            let result = bind(function, &[], &[]);
            assert_eq!(MoltObject::from_bits(result).as_int(), Some(197));
            hook_dec_ref(result);
            let runtime = OBSERVED.with(|slot| slot.borrow_mut().take()).unwrap();
            assert_eq!(runtime, raw);
            molt_cpython_abi::api::refcount::Py_DECREF(callable);
        }
        molt_cpython_abi::api::refcount::Py_DECREF(zero);
    }
}

#[test]
fn cext_method_accepts_and_preserves_physical_extension_defining_class() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    register_cpython_hooks();
    unsafe extern "C" fn return_class(
        _receiver: *mut PyObject,
        class: *mut PyTypeObject,
        _args: *mut *mut PyObject,
        _count: usize,
        _names: *mut PyObject,
    ) -> *mut PyObject {
        unsafe { molt_cpython_abi::api::object::Py_NewRef(class.cast()) }
    }
    unsafe {
        let mut class: Box<PyTypeObject> = Box::new(std::mem::zeroed());
        class.ob_base.ob_base.ob_refcnt = 1;
        class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        class.tp_name = c"extension.DefiningClass".as_ptr();
        let class_ptr = &raw mut *class;
        let mut definition = molt_cpython_abi::abi_types::PyMethodDef {
            ml_name: c"physical_defining_class".as_ptr(),
            ml_meth: Some(std::mem::transmute::<
                *const (),
                molt_cpython_abi::abi_types::PyCFunction,
            >(return_class as *const ())),
            ml_flags: METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
            ml_doc: ptr::null(),
        };
        let callable = molt_cpython_abi::api::object::PyCMethod_New(
            &raw mut definition,
            ptr::null_mut(),
            ptr::null_mut(),
            class_ptr,
        );
        assert!(
            !callable.is_null(),
            "genuine extension classes use foreign runtime wrappers"
        );
        let function = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(callable)
            .unwrap()
            .bits();
        let context = CExtCallableContext::from_bits(crate::function_execution_closure_bits(
            MoltObject::from_bits(function).as_ptr().unwrap(),
        ))
        .unwrap();
        assert_eq!(
            object_type_id(
                MoltObject::from_bits(context.defining_class)
                    .as_ptr()
                    .unwrap()
            ),
            crate::TYPE_ID_FOREIGN
        );
        let result = bind(function, &[1], &[("keyword", 2)]);
        assert!(!with_gil(|py| crate::exception_pending(&py)));
        assert_eq!(result, context.defining_class);
        hook_dec_ref(result);
        let raw_result = molt_cpython_abi::api::object::PyObject_Vectorcall(
            callable,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
        );
        assert_eq!(raw_result, class_ptr.cast());
        molt_cpython_abi::api::refcount::Py_DECREF(raw_result);
        molt_cpython_abi::api::refcount::Py_DECREF(callable);
        assert_eq!(
            class.ob_base.ob_base.ob_refcnt, 1,
            "both class custody edges must retire"
        );
    }
}
