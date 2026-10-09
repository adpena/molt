//! Exercise C convention transport and managed calls through the real runtime binder.

use super::*;
use molt_cpython_abi::abi_types::{METH_O, METH_STATIC, METH_VARARGS};
use molt_cpython_abi::api::{mapping, numbers as numeric, sequences, strings};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
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
    with_gil(|py| unsafe {
        let name = alloc_string(&py, b"DefiningClass");
        assert!(!name.is_null());
        let name_bits = MoltObject::from_ptr(name).bits();
        let class = crate::molt_class_new(name_bits);
        dec_ref_bits(&py, name_bits);
        let class_ptr = MoltObject::from_bits(class).as_ptr().unwrap();
        crate::molt_class_set_base(class, crate::builtin_classes(&py).object);
        crate::object::class_finish_definition(&py, class_ptr).unwrap();
        assert!(!crate::exception_pending(&py));
        class
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
            let result =
                hook_object_get_attr(bits, name_bits, AttributeAccess::Normal, ptr::null(), false);
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
        let _provider = crate::test_support::NativeProviderTestNamespace::new(&py, "builtins");
        // A genuine runtime builtin has no PyMethodDef/native C callback.
        let bits = crate::builtins::functions::lookup_builtin_name(&py, "len")
            .expect("canonical len builtin");
        assert_eq!(hook_classify_heap(bits), MoltTypeTag::RuntimeCallable as u8);
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
            MoltTypeTag::RuntimeCallable as u8
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
fn cext_fixed_conventions_reject_wrong_widths_across_call_routes() {
    use crate::call::function;
    use crate::concurrency::execution::{
        RuntimeExecutionGuard, current_thread_has_c_extension_execution_context,
    };
    use molt_cpython_abi::api::{errors, object, refcount, typeobj};

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        for (flags, target, width, expected) in [
            (
                METH_NOARGS,
                noargs as *const (),
                1,
                "convention_probe() takes no arguments (1 given)",
            ),
            (
                METH_O,
                one as *const (),
                0,
                "convention_probe() takes exactly one argument (0 given)",
            ),
            (
                METH_O,
                one as *const (),
                2,
                "convention_probe() takes exactly one argument (2 given)",
            ),
        ] {
            let callable_bits = register(target, flags, MoltObject::none().bits());
            assert_ne!(callable_bits, 0);
            let callable = refcount::OwnedPyObject::from_owned(
                cext_new_pyobject_from_borrowed_bits(callable_bits),
            );
            assert!(!callable.as_ptr().is_null());
            let positional = [10, 20];
            let args: Vec<_> = positional[..width]
                .iter()
                .map(|&value| MoltObject::from_int(value).bits())
                .collect();
            let tuple =
                refcount::OwnedPyObject::from_owned(sequences::PyTuple_New(width as Py_ssize_t));
            assert!(!tuple.as_ptr().is_null());
            for (index, &value) in positional[..width].iter().enumerate() {
                let value = numeric::PyLong_FromLongLong(value);
                assert!(!value.is_null());
                assert_eq!(
                    sequences::PyTuple_SetItem(tuple.as_ptr(), index as Py_ssize_t, value),
                    0
                );
            }
            let mut c_args: Vec<_> = (0..width)
                .map(|index| sequences::PyTuple_GetItem(tuple.as_ptr(), index as Py_ssize_t))
                .collect();
            for admitted in [false, true] {
                let _execution = admitted.then(RuntimeExecutionGuard::enter);
                for route in [
                    "fixed",
                    "bound-vector",
                    "runtime-vector",
                    "trampoline",
                    "binder",
                    "abi-tuple",
                    "abi-vector",
                    "abi-vectorcall-tuple",
                ] {
                    OBSERVED.with(|slot| *slot.borrow_mut() = None);
                    let runtime_result = match route {
                        "fixed" => Some(match width {
                            0 => function::call_function_obj0(&py, callable_bits),
                            1 => function::call_function_obj1(&py, callable_bits, args[0]),
                            2 => function::call_function_obj2(&py, callable_bits, args[0], args[1]),
                            _ => unreachable!(),
                        }),
                        "bound-vector" => Some(function::call_function_obj_bound_vec(
                            &py,
                            callable_bits,
                            &args,
                        )),
                        "runtime-vector" => {
                            Some(function::call_function_obj_vec(&py, callable_bits, &args))
                        }
                        "trampoline" => Some(function::call_function_obj_trampoline(
                            &py,
                            callable_bits,
                            &args,
                        )),
                        "binder" => Some(bind(callable_bits, &positional[..width], &[])),
                        _ => {
                            let result = match route {
                                "abi-tuple" => object::PyObject_Call(
                                    callable.as_ptr(),
                                    tuple.as_ptr(),
                                    ptr::null_mut(),
                                ),
                                "abi-vector" => object::PyObject_Vectorcall(
                                    callable.as_ptr(),
                                    c_args.as_mut_ptr(),
                                    width,
                                    ptr::null_mut(),
                                ),
                                "abi-vectorcall-tuple" => object::PyVectorcall_Call(
                                    callable.as_ptr(),
                                    tuple.as_ptr(),
                                    ptr::null_mut(),
                                ),
                                _ => unreachable!(),
                            };
                            assert!(
                                result.is_null(),
                                "{route}: flags={flags:#x}, admitted={admitted}"
                            );
                            None
                        }
                    };
                    if let Some(result) = runtime_result {
                        dec_ref_bits(&py, result);
                    }
                    assert!(
                        OBSERVED.with(|slot| slot.borrow().is_none()),
                        "{route}: flags={flags:#x}, admitted={admitted}",
                    );
                    {
                        let error =
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert!(
                            !error.as_ptr().is_null(),
                            "{route}: flags={flags:#x}, admitted={admitted}"
                        );
                        let class = refcount::OwnedPyObject::from_owned(typeobj::PyObject_Type(
                            error.as_ptr(),
                        ));
                        assert_eq!(
                            class.as_ptr(),
                            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                        );
                        let message = refcount::OwnedPyObject::from_owned(typeobj::PyObject_Str(
                            error.as_ptr(),
                        ));
                        assert!(!message.as_ptr().is_null());
                        let text = strings::PyUnicode_AsUTF8(message.as_ptr());
                        assert!(!text.is_null());
                        assert_eq!(
                            CStr::from_ptr(text).to_bytes(),
                            expected.as_bytes(),
                            "{route}: flags={flags:#x}, admitted={admitted}",
                        );
                    }
                    assert!(errors::PyErr_Occurred().is_null());
                    assert!(!crate::exception_pending(&py));
                    assert_eq!(current_thread_has_c_extension_execution_context(), admitted);
                }
            }
            drop(tuple);
            drop(callable);
            dec_ref_bits(&py, callable_bits);
        }
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
        assert!(!current_thread_has_c_extension_execution_context());
    });
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

thread_local! {
    static INGRESS_VECTOR_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static INGRESS_HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static INGRESS_EQUALS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static INGRESS_MAPPING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn record_vector_hook_entry() {
    INGRESS_VECTOR_ENTRIES.set(INGRESS_VECTOR_ENTRIES.get() + 1);
}
unsafe extern "C" fn ingress_vector_override(
    _callable: *mut PyObject,
    _values: *mut *mut PyObject,
    _nargsf: usize,
    _names: *mut PyObject,
) -> *mut PyObject {
    unsafe { numeric::PyLong_FromLong(619) }
}

extern "C" fn ingress_bound_python_target(_receiver: u64, left: u64, right: u64) -> u64 {
    ingress_python_target(left, right)
}

extern "C" fn ingress_keyword_hash(_key: u64) -> u64 {
    INGRESS_HASHES.set(INGRESS_HASHES.get() + 1);
    MoltObject::from_int(29).bits()
}
extern "C" fn ingress_keyword_equal(_left: u64, _right: u64) -> u64 {
    INGRESS_EQUALS.set(INGRESS_EQUALS.get() + 1);
    MoltObject::from_bool(false).bits()
}
extern "C" fn ingress_python_target(left: u64, right: u64) -> u64 {
    MoltObject::from_int(
        MoltObject::from_bits(left).as_int().unwrap()
            + MoltObject::from_bits(right).as_int().unwrap(),
    )
    .bits()
}
unsafe extern "C" fn ingress_dictionary_target(
    _self: *mut PyObject,
    _args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    INGRESS_MAPPING.set(kwargs as usize);
    unsafe { numeric::PyLong_FromLongLong(197) }
}

#[test]
fn capi_dictionary_transport_does_not_rehash_subclass_keys_but_vector_construction_does() {
    use molt_cpython_abi::abi_types::{PyCFunction, PyMethodDef};
    use molt_cpython_abi::api::{errors, object, refcount};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        let hash = crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::provenance::abi::expose_function_address(ingress_keyword_hash as *const ()),
            1,
        );
        let equal = crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::provenance::abi::expose_function_address(ingress_keyword_equal as *const ()),
            2,
        );
        let hash_bits = MoltObject::from_ptr(hash).bits();
        let equal_bits = MoltObject::from_ptr(equal).bits();
        let hash_name = MoltObject::from_ptr(alloc_string(&py, b"__hash__")).bits();
        let eq_name = MoltObject::from_ptr(alloc_string(&py, b"__eq__")).bits();
        let namespace =
            crate::alloc_dict_with_pairs(&py, &[hash_name, hash_bits, eq_name, equal_bits]);
        assert!(!namespace.is_null());
        let namespace_bits = MoltObject::from_ptr(namespace).bits();
        let class_name = MoltObject::from_ptr(alloc_string(&py, b"IngressName")).bits();
        let builtins = crate::builtin_classes(&py);
        let class = crate::builtins::types::molt_type_new(
            builtins.type_obj,
            class_name,
            builtins.str,
            namespace_bits,
            MoltObject::none().bits(),
        );
        assert!(!crate::exception_pending(&py));
        let keys = [b"first".as_slice(), b"second".as_slice()].map(|text| {
            let key = crate::object::builders::alloc_native_inline_bytes(
                &py,
                class,
                crate::object::native_instance::NativePayload::String,
                text,
            );
            assert!(!key.is_null());
            MoltObject::from_ptr(key).bits()
        });
        let values = [
            MoltObject::from_int(31).bits(),
            MoltObject::from_int(47).bits(),
        ];
        let dict = crate::alloc_dict_with_pairs(&py, &[keys[0], values[0], keys[1], values[1]]);
        assert!(!dict.is_null());
        assert!(
            INGRESS_HASHES.get() >= 2 && INGRESS_EQUALS.get() > 0,
            "the independent setup must actually exercise colliding subclass callbacks"
        );
        let dict_bits = MoltObject::from_ptr(dict).bits();
        let kwargs =
            refcount::OwnedPyObject::from_owned(cext_new_pyobject_from_borrowed_bits(dict_bits));
        let positional = refcount::OwnedPyObject::from_owned(sequences::PyTuple_New(0));
        let names_bits = MoltObject::from_ptr(crate::alloc_tuple(&py, &keys)).bits();
        let names = refcount::OwnedPyObject::from_owned(cext_owned_pyobject_from_bits(names_bits));
        let c_values = values.map(|value| {
            refcount::OwnedPyObject::from_owned(cext_new_pyobject_from_borrowed_bits(value))
        });
        let mut flat = c_values.each_ref().map(|value| value.as_ptr());
        // A managed Python function consumes the vector directly. Its parameter
        // names use these same valid string objects, making the pointer-match
        // path independent of user equality during the binder's own lookup.
        let python = crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::provenance::abi::expose_function_address(ingress_python_target as *const ()),
            2,
        );
        assert!(!python.is_null());
        let argument_names = MoltObject::from_ptr(crate::alloc_tuple(&py, &keys)).bits();
        let metadata_name = MoltObject::from_ptr(alloc_string(&py, b"__molt_arg_names__")).bits();
        assert!(crate::call::class_init::function_set_attr_bits(
            &py,
            python,
            metadata_name,
            argument_names
        ));
        dec_ref_bits(&py, argument_names);
        dec_ref_bits(&py, metadata_name);
        let python_callable = refcount::OwnedPyObject::from_owned(cext_owned_pyobject_from_bits(
            MoltObject::from_ptr(python).bits(),
        ));
        let bound_python = crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::provenance::abi::expose_function_address(
                ingress_bound_python_target as *const (),
            ),
            3,
        );
        assert!(!bound_python.is_null());
        let self_name = MoltObject::from_ptr(alloc_string(&py, b"self")).bits();
        let bound_names =
            MoltObject::from_ptr(crate::alloc_tuple(&py, &[self_name, keys[0], keys[1]])).bits();
        let metadata_name = MoltObject::from_ptr(alloc_string(&py, b"__molt_arg_names__")).bits();
        assert!(crate::call::class_init::function_set_attr_bits(
            &py,
            bound_python,
            metadata_name,
            bound_names
        ));
        let bound_python_bits = MoltObject::from_ptr(bound_python).bits();
        let bound_bits =
            crate::molt_bound_method_new(bound_python_bits, MoltObject::from_int(7).bits());
        assert!(!crate::exception_pending(&py));
        let bound_callable =
            refcount::OwnedPyObject::from_owned(cext_owned_pyobject_from_bits(bound_bits));
        let physical_func = refcount::OwnedPyObject::from_owned(
            cext_new_pyobject_from_borrowed_bits(bound_python_bits),
        );
        let physical_self = refcount::OwnedPyObject::from_owned(numeric::PyLong_FromLong(7));
        let physical_method = refcount::OwnedPyObject::from_owned(object::PyMethod_New(
            physical_func.as_ptr(),
            physical_self.as_ptr(),
        ));
        assert!(!physical_method.as_ptr().is_null());
        let call_name = MoltObject::from_ptr(alloc_string(&py, b"__call__")).bits();
        let callable_namespace = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
            &py,
            &[call_name, bound_python_bits],
        ))
        .bits();
        let callable_name = MoltObject::from_ptr(alloc_string(&py, b"NonVectorCallable")).bits();
        let callable_class = crate::builtins::types::molt_type_new(
            builtins.type_obj,
            callable_name,
            builtins.object,
            callable_namespace,
            MoltObject::none().bits(),
        );
        assert!(!crate::exception_pending(&py));
        let callable_instance = crate::call::class_init::alloc_instance_for_class(
            &py,
            MoltObject::from_bits(callable_class).as_ptr().unwrap(),
        );
        let custom_callable =
            refcount::OwnedPyObject::from_owned(cext_owned_pyobject_from_bits(callable_instance));
        let custom_type = refcount::OwnedPyObject::from_owned(
            cext_new_pyobject_from_borrowed_bits(callable_class),
        );
        assert!(object::PyVectorcall_Function(custom_callable.as_ptr()).is_none());
        assert!(object::PyVectorcall_Function(custom_type.as_ptr()).is_none());
        INGRESS_HASHES.set(0);
        INGRESS_EQUALS.set(0);
        INGRESS_VECTOR_ENTRIES.set(0);
        let custom_result = refcount::OwnedPyObject::from_owned(object::PyObject_Vectorcall(
            custom_callable.as_ptr(),
            flat.as_mut_ptr(),
            0,
            names.as_ptr(),
        ));
        assert!(!custom_result.as_ptr().is_null());
        assert_eq!(numeric::PyLong_AsLongLong(custom_result.as_ptr()), 78);
        assert!(
            INGRESS_HASHES.get() >= 2 && INGRESS_EQUALS.get() > 0,
            "a custom instance's actual nonvector route constructs kwargs"
        );
        assert_eq!(INGRESS_VECTOR_ENTRIES.get(), 0);
        drop(custom_result);
        drop(custom_callable);
        drop(custom_type);
        for bits in [call_name, callable_namespace, callable_name, callable_class] {
            dec_ref_bits(&py, bits);
        }
        for bits in [self_name, bound_names, metadata_name, bound_python_bits] {
            dec_ref_bits(&py, bits);
        }
        for (label, callable) in [
            ("function", python_callable.as_ptr()),
            ("bound method", bound_callable.as_ptr()),
            ("C-API method", physical_method.as_ptr()),
        ] {
            assert_eq!(
                object::PyCFunction_Check(callable),
                0,
                "{label} retains Python identity"
            );
            assert!(
                object::PyVectorcall_Function(callable).is_some(),
                "{label} owns actual vector storage"
            );
            // Public ingress alternatives all converge through the real slot.
            // The dictionary API keeps its original mapping route; all vector
            // alternatives must reach object_vectorcall without reconstructing.
            for ingress in 0..5 {
                INGRESS_HASHES.set(0);
                INGRESS_EQUALS.set(0);
                INGRESS_VECTOR_ENTRIES.set(0);
                let result = refcount::OwnedPyObject::from_owned(match ingress {
                    0 => object::PyObject_Call(callable, positional.as_ptr(), kwargs.as_ptr()),
                    1 => {
                        object::PyObject_Vectorcall(callable, flat.as_mut_ptr(), 0, names.as_ptr())
                    }
                    2 => {
                        object::_PyObject_Vectorcall(callable, flat.as_mut_ptr(), 0, names.as_ptr())
                    }
                    3 => object::PyObject_VectorcallDict(
                        callable,
                        ptr::null_mut(),
                        0,
                        kwargs.as_ptr(),
                    ),
                    _ => object::PyVectorcall_Call(callable, positional.as_ptr(), kwargs.as_ptr()),
                });
                assert!(!result.as_ptr().is_null(), "{label} ingress={ingress}");
                assert_eq!(numeric::PyLong_AsLongLong(result.as_ptr()), 78);
                assert_eq!(
                    (INGRESS_HASHES.get(), INGRESS_EQUALS.get()),
                    (0, 0),
                    "Python {label} ingress={ingress}"
                );
                assert_eq!(
                    INGRESS_VECTOR_ENTRIES.get(),
                    usize::from(ingress != 0),
                    "actual vector hook route: {label} ingress={ingress}"
                );
            }
        }
        // An actual mutable runtime carrier slot overrides the default adapter;
        // public dispatch must not bypass it merely because ownership is managed.
        let carrier = python_callable
            .as_ptr()
            .cast::<molt_cpython_abi::abi_types::PyCFunctionObject>();
        let original = (*carrier).vectorcall;
        (*carrier).vectorcall = Some(ingress_vector_override);
        INGRESS_VECTOR_ENTRIES.set(0);
        let overridden = object::PyObject_Vectorcall(
            python_callable.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
        );
        (*carrier).vectorcall = original;
        assert!(!overridden.is_null());
        assert_eq!(numeric::PyLong_AsLongLong(overridden), 619);
        assert_eq!(INGRESS_VECTOR_ENTRIES.get(), 0);
        refcount::Py_DECREF(overridden);
        drop(physical_method);
        drop(physical_self);
        drop(physical_func);
        drop(bound_callable);
        // Exact wrapper construction rejects these keywords without rebuilding
        // the supplied mapping. The old snapshot->dict lane invoked both user
        // callbacks before the native initializer could reject the call.
        let wrapper = refcount::OwnedPyObject::from_owned(cext_new_pyobject_from_borrowed_bits(
            builtins.staticmethod,
        ));
        INGRESS_HASHES.set(0);
        INGRESS_EQUALS.set(0);
        let rejected =
            object::PyObject_Call(wrapper.as_ptr(), positional.as_ptr(), kwargs.as_ptr());
        assert!(rejected.is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        assert_eq!((INGRESS_HASHES.get(), INGRESS_EQUALS.get()), (0, 0));
        errors::PyErr_Clear();
        drop(wrapper);
        drop(python_callable);
        for physical_cfunction in [false, true] {
            for (flags, target) in [
                (
                    METH_VARARGS | METH_KEYWORDS,
                    ingress_dictionary_target as *const (),
                ),
                (
                    METH_FASTCALL | METH_KEYWORDS,
                    fastcall_keywords as *const (),
                ),
            ] {
                let mut def = PyMethodDef {
                    ml_name: c"ingress_probe".as_ptr(),
                    ml_meth: Some(std::mem::transmute::<*const (), PyCFunction>(target)),
                    ml_flags: flags,
                    ml_doc: ptr::null(),
                };
                let callable = refcount::OwnedPyObject::from_owned(if physical_cfunction {
                    object::PyCFunction_NewEx(&raw mut def, ptr::null_mut(), ptr::null_mut())
                } else {
                    let bits = register(target, flags, MoltObject::none().bits());
                    cext_owned_pyobject_from_bits(bits)
                });
                assert!(!callable.as_ptr().is_null());
                INGRESS_HASHES.set(0);
                INGRESS_EQUALS.set(0);
                INGRESS_MAPPING.set(0);
                let result = refcount::OwnedPyObject::from_owned(object::PyObject_Call(
                    callable.as_ptr(),
                    positional.as_ptr(),
                    kwargs.as_ptr(),
                ));
                assert!(!result.as_ptr().is_null());
                assert!(errors::PyErr_Occurred().is_null());
                assert_eq!(
                    (INGRESS_HASHES.get(), INGRESS_EQUALS.get()),
                    (0, 0),
                    "dictionary ingress physical={physical_cfunction}, flags={flags}"
                );
                if flags & METH_VARARGS != 0 {
                    assert_eq!(
                        INGRESS_MAPPING.get(),
                        kwargs.as_ptr() as usize,
                        "the actual callback observes the original mapping identity"
                    );
                    let vector_result =
                        refcount::OwnedPyObject::from_owned(object::PyObject_Vectorcall(
                            callable.as_ptr(),
                            flat.as_mut_ptr(),
                            0,
                            names.as_ptr(),
                        ));
                    assert!(!vector_result.as_ptr().is_null());
                    assert!(
                        INGRESS_HASHES.get() >= 2 && INGRESS_EQUALS.get() > 0,
                        "vector ingress must construct its own dictionary"
                    );
                    assert_ne!(INGRESS_MAPPING.get(), kwargs.as_ptr() as usize);
                } else {
                    let result = refcount::OwnedPyObject::from_owned(object::PyObject_Vectorcall(
                        callable.as_ptr(),
                        flat.as_mut_ptr(),
                        0,
                        names.as_ptr(),
                    ));
                    assert!(!result.as_ptr().is_null());
                    assert_eq!(
                        (INGRESS_HASHES.get(), INGRESS_EQUALS.get()),
                        (0, 0),
                        "FASTCALL vector physical={physical_cfunction} must never construct a dictionary"
                    );
                }
            }
        }
        drop(kwargs);
        drop(names);
        drop(c_values);
        drop(positional);
        for bits in keys.into_iter().chain([
            dict_bits,
            class,
            class_name,
            namespace_bits,
            hash_name,
            eq_name,
            hash_bits,
            equal_bits,
        ]) {
            dec_ref_bits(&py, bits);
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn vector_hook_validates_complete_spans_before_reading_and_accepts_empty_null_spans() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    unsafe {
        let function = register(noargs as *const (), METH_NOARGS, MoltObject::none().bits());
        let dangling = std::ptr::NonNull::<u64>::dangling().as_ptr();
        let too_many = isize::MAX as usize / std::mem::size_of::<u64>() + 1;
        for (values, positional, names, keywords) in [
            (ptr::null(), 1, ptr::null(), 0),
            (dangling.cast_const(), 0, ptr::null(), 1),
            (dangling.cast_const(), usize::MAX, dangling.cast_const(), 1),
            (dangling.cast_const(), too_many, ptr::null(), 0),
        ] {
            assert!(matches!(
                hook_object_vectorcall(function, values, positional, names, keywords).decode(),
                DecodedHandleResult::Error
            ));
            assert!(with_gil(|py| crate::exception_pending(&py)));
            crate::molt_exception_clear();
            molt_cpython_abi::api::errors::PyErr_Clear();
        }
        let result = hook_object_vectorcall(function, ptr::null(), 0, ptr::null(), 0);
        let DecodedHandleResult::Ok(value) = result.decode() else {
            panic!("empty vector must call the target");
        };
        assert_eq!(MoltObject::from_bits(value).as_int(), Some(197));
        hook_dec_ref(value);
        hook_dec_ref(function);
    }
}

thread_local! {
    static CONSTRUCTOR_MAPPING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CONSTRUCTOR_PHASES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
unsafe extern "C" fn ingress_constructor_new(
    _self: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        assert_eq!(kwargs as usize, CONSTRUCTOR_MAPPING.get());
        CONSTRUCTOR_PHASES.set(CONSTRUCTOR_PHASES.get() + 1);
        let value = numeric::PyLong_FromLongLong(83);
        assert_eq!(
            mapping::PyDict_SetItemString(kwargs, c"from_new".as_ptr(), value),
            0
        );
        molt_cpython_abi::api::refcount::Py_DECREF(value);
        let class = sequences::PyTuple_GetItem(args, 0);
        let class = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_value_for_pyobj(class)
            .unwrap();
        with_gil(|py| {
            let result = crate::call::class_init::alloc_instance_for_class(
                &py,
                MoltObject::from_bits(class).as_ptr().unwrap(),
            );
            dec_ref_bits(&py, class);
            cext_owned_pyobject_from_bits(result)
        })
    }
}
unsafe extern "C" fn ingress_constructor_init(
    _self: *mut PyObject,
    _args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        assert_eq!(kwargs as usize, CONSTRUCTOR_MAPPING.get());
        assert_eq!(CONSTRUCTOR_PHASES.get(), 1);
        let value = mapping::PyDict_GetItemString(kwargs, c"from_new".as_ptr());
        assert!(!value.is_null());
        assert_eq!(numeric::PyLong_AsLongLong(value), 83);
        CONSTRUCTOR_PHASES.set(2);
        cext_new_pyobject_from_borrowed_bits(MoltObject::none().bits())
    }
}

#[test]
fn constructor_phases_preserve_original_mapping_and_observe_new_mutation() {
    use molt_cpython_abi::api::{object, refcount};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        let new = register(
            ingress_constructor_new as *const (),
            METH_VARARGS | METH_KEYWORDS,
            MoltObject::none().bits(),
        );
        let init = register(
            ingress_constructor_init as *const (),
            METH_VARARGS | METH_KEYWORDS,
            MoltObject::none().bits(),
        );
        let new_name = MoltObject::from_ptr(alloc_string(&py, b"__new__")).bits();
        let init_name = MoltObject::from_ptr(alloc_string(&py, b"__init__")).bits();
        let namespace = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
            &py,
            &[new_name, new, init_name, init],
        ))
        .bits();
        let name = MoltObject::from_ptr(alloc_string(&py, b"MappingConstructor")).bits();
        let builtins = crate::builtin_classes(&py);
        let class = crate::builtins::types::molt_type_new(
            builtins.type_obj,
            name,
            builtins.object,
            namespace,
            MoltObject::none().bits(),
        );
        // The real metaclass, rather than a helper-only target, owns type_call.
        assert!(!crate::exception_pending(&py));
        let callable =
            refcount::OwnedPyObject::from_owned(cext_new_pyobject_from_borrowed_bits(class));
        let args = refcount::OwnedPyObject::from_owned(sequences::PyTuple_New(0));
        let kwargs = refcount::OwnedPyObject::from_owned(mapping::PyDict_New());
        CONSTRUCTOR_MAPPING.set(kwargs.as_ptr() as usize);
        CONSTRUCTOR_PHASES.set(0);
        let result = refcount::OwnedPyObject::from_owned(object::PyObject_Call(
            callable.as_ptr(),
            args.as_ptr(),
            kwargs.as_ptr(),
        ));
        assert!(!result.as_ptr().is_null());
        assert_eq!(CONSTRUCTOR_PHASES.get(), 2);
        drop(result);
        drop(callable);
        drop(args);
        drop(kwargs);
        for bits in [class, name, namespace, new_name, init_name, new, init] {
            dec_ref_bits(&py, bits);
        }
        assert!(!crate::exception_pending(&py));
    });
}

thread_local! {
    static PUBLICATION_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
unsafe fn publication_write_list(list: *mut PyObject) -> *mut PyObject {
    unsafe {
        assert!(!list.is_null());
        let physical = list.cast::<molt_cpython_abi::abi_types::PyListObject>();
        assert!(!(*physical).ob_item.is_null());
        // Match direct Cython list storage: owned replacement pointer, no API
        // setter or subsequent C inquiry that could accidentally publish it.
        let replacement = numeric::PyLong_FromLong(907);
        assert!(!replacement.is_null());
        *(*physical).ob_item = replacement;
        if PUBLICATION_FAIL.get() {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                c"publication callback failure".as_ptr(),
            );
            ptr::null_mut()
        } else {
            cext_new_pyobject_from_borrowed_bits(MoltObject::none().bits())
        }
    }
}
unsafe extern "C" fn publication_positional(
    _self: *mut PyObject,
    list: *mut PyObject,
) -> *mut PyObject {
    unsafe { publication_write_list(list) }
}
unsafe extern "C" fn publication_self(
    list: *mut PyObject,
    _ignored: *mut PyObject,
) -> *mut PyObject {
    unsafe { publication_write_list(list) }
}
unsafe extern "C" fn publication_vector(
    _self: *mut PyObject,
    values: *mut *mut PyObject,
    count: Py_ssize_t,
    _names: *mut PyObject,
) -> *mut PyObject {
    assert_eq!(count, 0);
    unsafe { publication_write_list(*values) }
}
unsafe extern "C" fn publication_mapping(
    _self: *mut PyObject,
    _args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        let list = mapping::PyDict_GetItemString(kwargs, c"payload".as_ptr());
        assert!(!list.is_null());
        // Completion must own the pre-call direct operand even after removal.
        mapping::PyDict_Clear(kwargs);
        publication_write_list(list)
    }
}

#[test]
fn runtime_cext_completion_publishes_direct_operands_on_success_and_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        for fail in [false, true] {
            for channel in 0..4 {
                let list = crate::alloc_list(&py, &[MoltObject::from_int(101).bits()]);
                assert!(!list.is_null());
                let list_bits = MoltObject::from_ptr(list).bits();
                let name = MoltObject::from_ptr(alloc_string(&py, b"payload")).bits();
                let (target, flags) = match channel {
                    0 => (publication_positional as *const (), METH_O),
                    1 => (publication_self as *const (), METH_NOARGS),
                    2 => (
                        publication_vector as *const (),
                        METH_FASTCALL | METH_KEYWORDS,
                    ),
                    _ => (
                        publication_mapping as *const (),
                        METH_VARARGS | METH_KEYWORDS,
                    ),
                };
                let function = hook_register_c_function(
                    crate::provenance::abi::expose_function_address(target),
                    flags,
                    if channel == 1 {
                        list_bits
                    } else {
                        MoltObject::none().bits()
                    },
                    channel != 1,
                    MoltObject::none().bits(),
                    b"publication_probe".as_ptr(),
                    b"publication_probe".len(),
                );
                assert_ne!(function, 0);
                PUBLICATION_FAIL.set(fail);
                let result = if channel >= 2 {
                    let dict = crate::alloc_dict_with_pairs(&py, &[name, list_bits]);
                    let dict_bits = MoltObject::from_ptr(dict).bits();
                    let result =
                        crate::call::bind::call_bind_capi(&py, function, None, &[], dict_bits);
                    if channel == 3 {
                        assert_eq!(crate::dict_len(dict), 0);
                    }
                    dec_ref_bits(&py, dict_bits);
                    result
                } else {
                    let positional = [list_bits];
                    crate::call::bind::call_bind_capi_vector(
                        &py,
                        function,
                        if channel == 0 { &positional } else { &[] },
                        &[],
                        &[],
                    )
                };
                // This reads the tracked runtime Vec directly. It deliberately
                // never rereads the C view or calls bridge observation first.
                let actual = crate::object::seq_access::pin_item(&py, list, 0).unwrap();
                assert_eq!(
                    MoltObject::from_bits(actual.bits()).as_int(),
                    Some(907),
                    "channel={channel}, fail={fail}"
                );
                assert_eq!(crate::exception_pending(&py), fail);
                if fail {
                    let error = crate::exception_last_bits_noinc(&py).unwrap();
                    let ptr = MoltObject::from_bits(error).as_ptr().unwrap();
                    assert_eq!(
                        crate::format_exception_message(&py, ptr),
                        "publication callback failure"
                    );
                    crate::molt_exception_clear();
                }
                if result != 0 {
                    dec_ref_bits(&py, result);
                }
                drop(actual);
                for bits in [function, name, list_bits] {
                    dec_ref_bits(&py, bits);
                }
                assert!(!crate::exception_pending(&py));
            }
        }
    });
}

extern "C" fn method_nested_target(first: u64, second: u64, third: u64) -> u64 {
    let value = |bits| MoltObject::from_bits(bits).as_int().unwrap();
    MoltObject::from_int(value(first) * 100 + value(second) * 10 + value(third)).bits()
}

#[test]
fn capi_method_constructor_reuses_runtime_owner_and_preserves_explicit_binding() {
    use molt_cpython_abi::api::{errors, object, refcount, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        let target = crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::provenance::abi::expose_function_address(method_nested_target as *const ()),
            3,
        );
        assert!(!target.is_null());
        let function = refcount::OwnedPyObject::from_owned(cext_owned_pyobject_from_bits(
            MoltObject::from_ptr(target).bits(),
        ));
        let first = refcount::OwnedPyObject::from_owned(numeric::PyLong_FromLong(1));
        let second = refcount::OwnedPyObject::from_owned(numeric::PyLong_FromLong(2));
        let third = refcount::OwnedPyObject::from_owned(numeric::PyLong_FromLong(3));
        let inner = refcount::OwnedPyObject::from_owned(object::PyMethod_New(
            function.as_ptr(),
            first.as_ptr(),
        ));
        assert!(!inner.as_ptr().is_null());
        let outer = refcount::OwnedPyObject::from_owned(object::PyMethod_New(
            inner.as_ptr(),
            second.as_ptr(),
        ));
        assert!(!outer.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .managed_handle_for_pyobj(outer.as_ptr())
                .is_some()
        );
        assert_eq!(object::PyMethod_Check(outer.as_ptr()), 1);
        assert_eq!(object::PyCFunction_Check(outer.as_ptr()), 0);
        assert_eq!(
            object::PyMethod_GET_FUNCTION(outer.as_ptr()),
            inner.as_ptr()
        );
        assert_eq!(object::PyMethod_GET_SELF(outer.as_ptr()), second.as_ptr());
        let class = refcount::OwnedPyObject::from_owned(typeobj::PyObject_Type(outer.as_ptr()));
        assert_eq!(
            class.as_ptr(),
            (&raw mut molt_cpython_abi::abi_types::PyMethod_Type).cast()
        );
        for (name, expected) in [
            (c"__func__", inner.as_ptr()),
            (c"__self__", second.as_ptr()),
        ] {
            let value = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                outer.as_ptr(),
                name.as_ptr(),
            ));
            assert_eq!(value.as_ptr(), expected);
        }
        let shadow = crate::attr_name_bits_from_bytes(&py, b"__hash__").unwrap();
        assert!(crate::call::class_init::function_set_attr_bits(
            &py,
            target,
            shadow,
            MoltObject::from_int(991).bits()
        ));
        dec_ref_bits(&py, shadow);
        let function_hash = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            function.as_ptr(),
            c"__hash__".as_ptr(),
        ));
        assert_eq!(numeric::PyLong_AsLong(function_hash.as_ptr()), 991);
        let method_hash = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            outer.as_ptr(),
            c"__hash__".as_ptr(),
        ));
        assert_eq!(
            typeobj::PyCallable_Check(method_hash.as_ptr()),
            1,
            "method descriptor precedes function shadow"
        );
        let hash =
            refcount::OwnedPyObject::from_owned(object::PyObject_CallNoArgs(method_hash.as_ptr()));
        assert!(!hash.as_ptr().is_null());
        assert_eq!(
            numeric::PyLong_AsLongLong(hash.as_ptr()),
            typeobj::PyObject_Hash(outer.as_ptr()) as i64
        );
        let method_eq = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            outer.as_ptr(),
            c"__eq__".as_ptr(),
        ));
        let equal = refcount::OwnedPyObject::from_owned(object::PyObject_CallOneArg(
            method_eq.as_ptr(),
            outer.as_ptr(),
        ));
        assert_eq!(
            equal.as_ptr(),
            (&raw mut molt_cpython_abi::abi_types::Py_True).cast()
        );
        let method_class = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            outer.as_ptr(),
            c"__class__".as_ptr(),
        ));
        assert_eq!(method_class.as_ptr(), class.as_ptr());
        let mut arguments = [third.as_ptr()];
        let result = refcount::OwnedPyObject::from_owned(object::PyObject_Vectorcall(
            outer.as_ptr(),
            arguments.as_mut_ptr(),
            1,
            ptr::null_mut(),
        ));
        assert!(!result.as_ptr().is_null());
        assert_eq!(
            numeric::PyLong_AsLong(result.as_ptr()),
            123,
            "both explicit receivers survive nested binding"
        );

        // C constructor permits Python None and a noncallable function; only
        // invocation rejects it. Both immutable getters remain available.
        let none = &raw mut molt_cpython_abi::abi_types::Py_None;
        let noncallable =
            refcount::OwnedPyObject::from_owned(object::PyMethod_New(third.as_ptr(), none));
        assert!(!noncallable.as_ptr().is_null());
        assert_eq!(
            object::PyMethod_GET_FUNCTION(noncallable.as_ptr()),
            third.as_ptr()
        );
        assert_eq!(object::PyMethod_GET_SELF(noncallable.as_ptr()), none);
        for (name, expected) in [(c"__func__", third.as_ptr()), (c"__self__", none)] {
            let value = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                noncallable.as_ptr(),
                name.as_ptr(),
            ));
            assert_eq!(value.as_ptr(), expected);
        }
        let real = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            noncallable.as_ptr(),
            c"real".as_ptr(),
        ));
        assert!(
            !real.as_ptr().is_null(),
            "noncallable inline func uses ordinary scalar lookup"
        );
        assert_eq!(numeric::PyLong_AsLong(real.as_ptr()), 3);
        assert!(object::PyMethod_New(function.as_ptr(), ptr::null_mut()).is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        let function_bits = GLOBAL_BRIDGE
            .managed_handle_for_pyobj(function.as_ptr())
            .unwrap();
        let cls = crate::builtins::types::method_class(&py);
        for (func, receiver) in [
            (function_bits, MoltObject::none().bits()),
            (
                MoltObject::from_int(3).bits(),
                MoltObject::from_int(2).bits(),
            ),
        ] {
            let rejected = crate::builtins::types::molt_types_method_new(cls, func, receiver);
            assert!(crate::exception_pending(&py));
            dec_ref_bits(&py, rejected);
            crate::clear_exception(&py);
        }
        let inner_bits = GLOBAL_BRIDGE
            .managed_handle_for_pyobj(inner.as_ptr())
            .unwrap();
        let explicit = crate::builtins::types::molt_types_method_new(
            cls,
            inner_bits,
            MoltObject::from_int(2).bits(),
        );
        assert!(!crate::exception_pending(&py));
        assert_eq!(
            crate::bound_method_func_bits(MoltObject::from_bits(explicit).as_ptr().unwrap()),
            inner_bits
        );
        dec_ref_bits(&py, explicit);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn capi_numeric_readonly_members_use_runtime_descriptors() {
    use molt_cpython_abi::abi_types::{PyComplex_Type, PyFloat_Type, PyLong_Type};
    use molt_cpython_abi::api::{errors, object, refcount};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    unsafe {
        for (value, owner, fields) in [
            (
                numeric::PyLong_FromLong(37),
                (&raw mut PyLong_Type).cast(),
                &[
                    (c"real", 37.0f64),
                    (c"imag", 0.0),
                    (c"numerator", 37.0),
                    (c"denominator", 1.0),
                ][..],
            ),
            (
                numeric::PyFloat_FromDouble(-0.0),
                (&raw mut PyFloat_Type).cast(),
                &[(c"real", -0.0f64), (c"imag", 0.0)][..],
            ),
            (
                numeric::PyComplex_FromDoubles(3.0, -0.0),
                (&raw mut PyComplex_Type).cast(),
                &[(c"real", 3.0f64), (c"imag", -0.0)][..],
            ),
        ] {
            let value = refcount::OwnedPyObject::from_owned(value);
            assert!(!value.as_ptr().is_null());
            for &(field, expected) in fields {
                let result = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                    value.as_ptr(),
                    field.as_ptr(),
                ));
                assert!(!result.as_ptr().is_null(), "numeric C member read");
                assert_eq!(
                    numeric::PyFloat_AsDouble(result.as_ptr()).to_bits(),
                    expected.to_bits()
                );
                let descriptor = refcount::OwnedPyObject::from_owned(
                    object::PyObject_GetAttrString(owner, field.as_ptr()),
                );
                assert!(
                    !descriptor.as_ptr().is_null(),
                    "class exposes actual descriptor"
                );
                let get = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                    descriptor.as_ptr(),
                    c"__get__".as_ptr(),
                ));
                assert!(!get.as_ptr().is_null());
                let direct = refcount::OwnedPyObject::from_owned(object::PyObject_CallOneArg(
                    get.as_ptr(),
                    value.as_ptr(),
                ));
                assert!(!direct.as_ptr().is_null());
                assert_eq!(
                    numeric::PyFloat_AsDouble(direct.as_ptr()).to_bits(),
                    expected.to_bits()
                );
                assert_eq!(
                    object::PyObject_SetAttrString(value.as_ptr(), field.as_ptr(), result.as_ptr()),
                    -1
                );
                assert!(!errors::PyErr_Occurred().is_null());
                errors::PyErr_Clear();
            }
        }
        assert!(errors::PyErr_Occurred().is_null());
    }
}

#[test]
fn capi_numeric_origins_survive_method_container_and_attribute_owners() {
    use molt_cpython_abi::api::{errors, object, refcount, typeobj};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    unsafe {
        for floating in [false, true] {
            let new = || {
                if floating {
                    numeric::PyFloat_FromDouble(1.25)
                } else {
                    numeric::PyLong_FromLong(1000)
                }
            };
            let first = new();
            let second = new();
            assert!(!first.is_null() && !second.is_null());
            assert_ne!(first, second);
            let first_bits = molt_cpython_abi::bridge::molt_capi_pyobj_to_handle(first);
            let second_bits = molt_cpython_abi::bridge::molt_capi_pyobj_to_handle(second);
            assert!(MoltObject::from_bits(first_bits).is_ptr());
            assert_ne!(
                first_bits, second_bits,
                "equal C origins have different runtime identities"
            );
            let function = numeric::PyLong_FromLong(7); // C PyMethod_New permits a noncallable function.
            let one = object::PyMethod_New(function, first);
            let same = object::PyMethod_New(function, first);
            let other = object::PyMethod_New(function, second);
            assert!(!one.is_null() && !same.is_null() && !other.is_null());
            assert_eq!(typeobj::PyObject_RichCompareBool(one, same, 2), 1);
            assert_eq!(typeobj::PyObject_RichCompareBool(one, other, 2), 0);
            let original_hash = typeobj::PyObject_Hash(one);
            assert_eq!(typeobj::PyObject_Hash(same), original_hash);
            let tuple = sequences::PyTuple_New(1);
            refcount::Py_INCREF(first);
            assert_eq!(sequences::PyTuple_SetItem(tuple, 0, first), 0);
            refcount::Py_DECREF(first);
            refcount::Py_DECREF(second);
            // Only actual method/tuple owners retain these original C origins.
            assert_eq!(object::PyMethod_GET_SELF(one), first);
            let self_attr = object::PyObject_GetAttrString(one, c"__self__".as_ptr());
            assert_eq!(self_attr, first);
            let real_attr = object::PyObject_GetAttrString(self_attr, c"real".as_ptr());
            assert_eq!(
                real_attr, first,
                "exact numeric real descriptor retains its receiver identity"
            );
            assert_eq!(sequences::PyTuple_GetItem(tuple, 0), first);
            assert_eq!(typeobj::PyObject_Hash(one), original_hash);
            refcount::Py_DECREF(real_attr);
            refcount::Py_DECREF(self_attr);
            for value in [tuple, one, same, other, function] {
                refcount::Py_DECREF(value);
            }
            assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(first).is_none());
            assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(second).is_none());
            assert!(errors::PyErr_Occurred().is_null());
        }
    }
}

#[test]
fn capi_direct_list_commit_retains_self_and_mutual_cycles_without_recursive_observation() {
    use molt_cpython_abi::abi_types::PyListObject;
    use molt_cpython_abi::api::{errors, object, refcount};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| unsafe {
        for mutual in [false, true] {
            let first = sequences::PyList_New(1);
            let second = if mutual {
                sequences::PyList_New(1)
            } else {
                first
            };
            assert!(!first.is_null() && !second.is_null());
            refcount::Py_INCREF(second); // direct ob_item owns one stolen C reference
            *(*first.cast::<PyListObject>()).ob_item = second;
            if mutual {
                refcount::Py_INCREF(first);
                *(*second.cast::<PyListObject>()).ob_item = first;
            }
            let first_bits = molt_cpython_abi::bridge::molt_capi_pyobj_to_handle(first);
            assert_ne!(first_bits, 0);
            let second_bits = molt_cpython_abi::bridge::molt_capi_pyobj_to_handle(second);
            assert_ne!(second_bits, 0);
            {
                let item = crate::object::seq_access::pin_item(
                    &py,
                    MoltObject::from_bits(first_bits).as_ptr().unwrap(),
                    0,
                )
                .unwrap();
                assert_eq!(item.bits(), second_bits);
                let item = crate::object::seq_access::pin_item(
                    &py,
                    MoltObject::from_bits(second_bits).as_ptr().unwrap(),
                    0,
                )
                .unwrap();
                assert_eq!(item.bits(), first_bits);
            }
            let none = &raw mut molt_cpython_abi::abi_types::Py_None;
            assert_eq!(
                sequences::PyList_SetItem(first, 0, object::Py_NewRef(none)),
                0
            );
            if mutual {
                assert_eq!(
                    sequences::PyList_SetItem(second, 0, object::Py_NewRef(none)),
                    0
                );
            }
            refcount::Py_DECREF(first);
            if mutual {
                refcount::Py_DECREF(second);
            }
            assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(first).is_none());
            assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(second).is_none());
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn owned_hook_result_error_retires_transferred_owner_and_preserves_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    with_gil(|py| {
        use crate::builtins::exceptions::ExceptionValue;
        let child_ptr = crate::alloc_list(&py, &[]);
        assert!(!child_ptr.is_null());
        let child_bits = MoltObject::from_ptr(child_ptr).bits();
        let child = ExceptionValue::adopt(&py, child_bits);
        let count = || unsafe { (*crate::header_from_obj_ptr(child_ptr)).ref_count_snapshot() };
        assert_eq!(count(), 1);
        let result_ptr = crate::alloc_list(&py, &[child_bits]);
        assert!(!result_ptr.is_null());
        assert_eq!(count(), 2);
        let not_implemented = crate::not_implemented_bits(&py);
        let not_implemented_ptr = MoltObject::from_bits(not_implemented).as_ptr().unwrap();
        let immortal_count =
            unsafe { (*crate::header_from_obj_ptr(not_implemented_ptr)).ref_count_snapshot() };
        crate::raise_exception::<()>(&py, "ValueError", "incoming owned result error");
        let original = crate::exception_last_bits_noinc(&py).unwrap();
        assert!(matches!(
            owned_result_from_pending(MoltObject::from_ptr(result_ptr).bits()).decode(),
            molt_cpython_abi::hooks::DecodedHandleResult::Error
        ));
        assert_eq!(
            count(),
            1,
            "failed result releases its sole container owner and child edge"
        );
        assert_eq!(crate::exception_last_bits_noinc(&py), Some(original));
        for immediate in [
            MoltObject::none().bits(),
            MoltObject::from_float(0.0).bits(),
        ] {
            assert!(matches!(
                owned_result_from_pending(immediate).decode(),
                molt_cpython_abi::hooks::DecodedHandleResult::Error
            ));
            assert_eq!(count(), 1);
            assert_eq!(crate::exception_last_bits_noinc(&py), Some(original));
        }
        assert!(matches!(
            owned_result_from_pending(not_implemented).decode(),
            molt_cpython_abi::hooks::DecodedHandleResult::Error
        ));
        assert_eq!(
            unsafe { (*crate::header_from_obj_ptr(not_implemented_ptr)).ref_count_snapshot() },
            immortal_count
        );
        assert_eq!(crate::not_implemented_bits(&py), not_implemented);
        assert_eq!(crate::exception_last_bits_noinc(&py), Some(original));
        crate::clear_exception(&py);
        let success_ptr = crate::alloc_list(&py, &[child_bits]);
        assert!(!success_ptr.is_null());
        let success_bits = MoltObject::from_ptr(success_ptr).bits();
        assert_eq!(count(), 2);
        let returned = match owned_result_from_pending(success_bits).decode() {
            molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => {
                ExceptionValue::adopt(&py, bits)
            }
            _ => panic!("successful result must transfer its existing owner"),
        };
        assert_eq!(returned.bits(), success_bits);
        assert_eq!(
            count(),
            2,
            "success is not retained or retired by the status wrapper"
        );
        drop(returned);
        assert_eq!(count(), 1);
        assert!(
            matches!(
                owned_result_from_pending(0).decode(),
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(0)
            ),
            "zero is a valid inline float result"
        );
        drop(child);
        assert!(!crate::exception_pending(&py));
    });
}
