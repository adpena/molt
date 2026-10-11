//! Run the real managed consumer in both primary and isolated runtimes. Expected
//! values follow CPython 3.12 Objects/exceptions.c (BaseException, group, KeyError,
//! SyntaxError, ImportError, OSError and all Unicode str slots); the existing
//! exception_message_storage.py differential case independently covers Python.

use super::*;
use std::cell::{Cell, RefCell};

thread_local! {
    static OWNER: Cell<u64> = const { Cell::new(0) };
    static REPLACEMENT: Cell<u64> = const { Cell::new(0) };
    static RESULT: Cell<u64> = const { Cell::new(0) };
    static ENCODING: Cell<u64> = const { Cell::new(0) };
    static FAILURE: Cell<u64> = const { Cell::new(0) };
    static EVENTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

struct CallbackScope;
impl Drop for CallbackScope {
    fn drop(&mut self) {
        for slot in [&OWNER, &REPLACEMENT, &RESULT, &ENCODING, &FAILURE] {
            slot.with(|slot| slot.set(0));
        }
        EVENTS.with(|events| events.borrow_mut().clear());
    }
}

fn text<'a, 'py>(py: &'a PyToken<'py>, bytes: &[u8]) -> ExceptionValue<'a, 'py> {
    let pointer = alloc_string(py, bytes);
    assert!(!pointer.is_null());
    ExceptionValue::adopt(py, MoltObject::from_ptr(pointer).bits())
}

fn tuple<'a, 'py>(py: &'a PyToken<'py>, values: &[u64]) -> ExceptionValue<'a, 'py> {
    let pointer = alloc_tuple(py, values);
    assert!(!pointer.is_null());
    ExceptionValue::adopt(py, MoltObject::from_ptr(pointer).bits())
}

fn exception<'a, 'py>(py: &'a PyToken<'py>, class: u64, args: &[u64]) -> ExceptionValue<'a, 'py> {
    let args = tuple(py, args);
    let pointer = alloc_exception_from_class_bits(py, class, args.bits());
    assert!(!pointer.is_null());
    assert!(!exception_pending(py));
    ExceptionValue::adopt(py, MoltObject::from_ptr(pointer).bits())
}

fn builtin<'a, 'py>(py: &'a PyToken<'py>, name: &str, args: &[u64]) -> ExceptionValue<'a, 'py> {
    exception(py, exception_type_bits_from_name(py, name), args)
}

fn assert_text(py: &PyToken<'_>, result: u64, expected: &[u8]) -> u64 {
    let value = ExceptionValue::adopt(py, result);
    assert!(!exception_pending(py), "render unexpectedly raised");
    assert_eq!(
        crate::object::ops_format::string_obj_bytes(obj_from_bits(value.bits())).as_deref(),
        Some(expected)
    );
    value.bits()
}

fn str_eq(py: &PyToken<'_>, error: u64, expected: &[u8]) -> u64 {
    assert_text(py, molt_exception_message(error), expected)
}

fn set_field(py: &PyToken<'_>, error: u64, field: ExceptionTypedField, value: u64) {
    assert_eq!(
        exception_typed_field_replace(py, error, field, value),
        Some(Ok(()))
    );
    assert!(!exception_pending(py));
}

fn call_declared(py: &PyToken<'_>, owner: &str, name: &str, error: u64) -> u64 {
    let descriptor =
        exception_render_method_bits(py, exception_type_bits_from_name(py, owner), name)
            .expect("method lookup succeeds")
            .expect("declaring descriptor must exist before C publication");
    unsafe { call_callable1(py, descriptor, error) }
}

fn callback_value(py: &PyToken<'_>) -> u64 {
    let result = RESULT.with(Cell::get);
    inc_ref_bits(py, result);
    result
}

extern "C" fn overridden_str(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        EVENTS.with(|events| events.borrow_mut().push("str"));
        let owner = OWNER.with(Cell::get);
        if owner != 0 {
            exception_replace_field_bits(
                py,
                owner,
                ExceptionFieldSlot::Args,
                REPLACEMENT.with(Cell::get),
            )
            .unwrap();
        }
        callback_value(py)
    })
}

extern "C" fn overridden_repr(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        EVENTS.with(|events| events.borrow_mut().push("repr"));
        let owner = OWNER.with(Cell::get);
        if owner != 0 {
            exception_replace_field_bits(
                py,
                owner,
                ExceptionFieldSlot::Args,
                REPLACEMENT.with(Cell::get),
            )
            .unwrap();
        }
        callback_value(py)
    })
}

extern "C" fn failed_str(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        EVENTS.with(|events| events.borrow_mut().push("failure"));
        record_exception(py, obj_from_bits(FAILURE.with(Cell::get)).as_ptr().unwrap());
        MoltObject::none().bits()
    })
}

extern "C" fn unicode_reason(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        EVENTS.with(|events| events.borrow_mut().push("reason"));
        set_field(
            py,
            OWNER.with(Cell::get),
            ExceptionTypedField::UnicodeEncoding,
            ENCODING.with(Cell::get),
        );
        text(py, b"reason").into_bits()
    })
}

extern "C" fn unicode_encoding(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        EVENTS.with(|events| events.borrow_mut().push("encoding"));
        let owner = OWNER.with(Cell::get);
        set_field(
            py,
            owner,
            ExceptionTypedField::UnicodeObject,
            REPLACEMENT.with(Cell::get),
        );
        set_field(
            py,
            owner,
            ExceptionTypedField::UnicodeStart,
            MoltObject::from_int(0).bits(),
        );
        set_field(
            py,
            owner,
            ExceptionTypedField::UnicodeEnd,
            MoltObject::from_int(1).bits(),
        );
        text(py, b"changed").into_bits()
    })
}

fn subclass<'a, 'py>(
    py: &'a PyToken<'py>,
    name: &[u8],
    base: u64,
    methods: &[(&[u8], u64)],
) -> ExceptionValue<'a, 'py> {
    let name = text(py, name);
    let class = ExceptionValue::adopt(py, crate::molt_class_new(name.bits()));
    crate::molt_class_set_base(class.bits(), base);
    let pointer = obj_from_bits(class.bits()).as_ptr().unwrap();
    unsafe { crate::object::class_finish_definition(py, pointer) }.unwrap();
    for &(name, address) in methods {
        let name = text(py, name);
        let pointer = crate::builtins::functions::alloc_runtime_function_obj(py, address, 1);
        assert!(!pointer.is_null());
        let function = ExceptionValue::adopt(py, MoltObject::from_ptr(pointer).bits());
        crate::molt_set_attr_name(class.bits(), name.bits(), function.bits());
    }
    assert!(!exception_pending(py));
    class
}

fn events(expected: &[&str]) {
    EVENTS.with(|events| {
        assert_eq!(&*events.borrow(), expected);
        events.borrow_mut().clear();
    });
}

/// Exposed only to the existing isolate lifecycle suite, so the same fixtures
/// exercise both runtime ownership coordinates without copying expectations.
pub(crate) fn assert_managed_rendering_contract(py: &PyToken<'_>) {
    let _callbacks = CallbackScope;
    let message = text(py, b"message");
    let new_message = text(py, b"new args");
    for name in [
        "BaseException",
        "Exception",
        "RuntimeError",
        "ValueError",
        "LookupError",
        "NotImplementedError",
        "RecursionError",
        "SystemExit",
        "StopIteration",
        "AttributeError",
        "NameError",
        "Warning",
        "UserWarning",
        "ImportError",
        "SyntaxError",
    ] {
        let error = builtin(py, name, &[message.bits()]);
        assert_eq!(
            str_eq(py, error.bits(), b"message"),
            message.bits(),
            "{name} preserves string identity"
        );
        assert_text(
            py,
            molt_repr_from_obj(error.bits()),
            format!("{name}('message')").as_bytes(),
        );
    }
    let empty = builtin(py, "RuntimeError", &[]);
    str_eq(py, empty.bits(), b"");
    assert_text(py, molt_repr_from_obj(empty.bits()), b"RuntimeError()");
    let pair = builtin(
        py,
        "RuntimeError",
        &[message.bits(), MoltObject::from_int(7).bits()],
    );
    str_eq(py, pair.bits(), b"('message', 7)");
    assert_text(
        py,
        molt_repr_from_obj(pair.bits()),
        b"RuntimeError('message', 7)",
    );
    let key = builtin(py, "KeyError", &[message.bits()]);
    str_eq(py, key.bits(), b"'message'");
    assert_eq!(
        assert_text(
            py,
            call_declared(py, "BaseException", "__str__", key.bits()),
            b"message"
        ),
        message.bits()
    );
    assert_text(
        py,
        call_declared(py, "BaseException", "__repr__", key.bits()),
        b"KeyError('message')",
    );
    let invalid = ExceptionValue::adopt(py, call_declared(py, "KeyError", "__str__", empty.bits()));
    assert!(
        exception_pending(py),
        "explicit derived descriptor must reject a sibling receiver"
    );
    assert!(exception_matches_builtin_name(
        py,
        exception_last_bits_noinc(py).unwrap(),
        "TypeError"
    ));
    clear_exception(py);
    drop(invalid);
    for name in [
        "RuntimeError",
        "ValueError",
        "ExceptionGroup",
        "IndentationError",
        "ModuleNotFoundError",
    ] {
        let class = exception_type_bits_from_name(py, name);
        assert!(
            exception_render_method_bits(py, class, "__str__")
                .expect("renderer lookup succeeds")
                .is_none(),
            "{name} inherits str"
        );
        assert!(
            exception_render_method_bits(py, class, "__repr__")
                .expect("renderer lookup succeeds")
                .is_none(),
            "{name} inherits repr"
        );
    }
    for name in [
        "BaseException",
        "BaseExceptionGroup",
        "KeyError",
        "SyntaxError",
        "ImportError",
        "OSError",
        "UnicodeDecodeError",
        "UnicodeEncodeError",
        "UnicodeTranslateError",
    ] {
        let class = exception_type_bits_from_name(py, name);
        let name_bits = text(py, b"__str__");
        let dictionary =
            obj_from_bits(unsafe { class_dict_bits(obj_from_bits(class).as_ptr().unwrap()) })
                .as_ptr()
                .unwrap();
        assert!(
            unsafe {
                dict_get_in_place(py, dictionary, name_bits.bits())
                    .expect("dictionary lookup succeeds")
            }
            .is_some(),
            "own namespace {name}.__str__"
        );
    }
    for name in ["SyntaxError", "ImportError"] {
        let error = builtin(py, name, &[message.bits()]);
        let replacement = tuple(py, &[new_message.bits()]);
        exception_replace_field_bits(
            py,
            error.bits(),
            ExceptionFieldSlot::Args,
            replacement.bits(),
        )
        .unwrap();
        assert_eq!(str_eq(py, error.bits(), b"message"), message.bits());
        assert_text(
            py,
            call_declared(py, "BaseException", "__str__", error.bits()),
            b"new args",
        );
    }
    let os = builtin(
        py,
        "OSError",
        &[MoltObject::from_int(5).bits(), message.bits()],
    );
    str_eq(py, os.bits(), b"[Errno 5] message");
    set_field(
        py,
        os.bits(),
        ExceptionTypedField::OSErrorFilename,
        MoltObject::none().bits(),
    );
    str_eq(py, os.bits(), b"[Errno 5] message: None");
    set_field(
        py,
        os.bits(),
        ExceptionTypedField::OSErrorFilename2,
        MoltObject::none().bits(),
    );
    str_eq(py, os.bits(), b"[Errno 5] message: None -> None");
    assert_eq!(
        exception_typed_field_delete(py, os.bits(), ExceptionTypedField::OSErrorFilename),
        Some(Ok(()))
    );
    str_eq(py, os.bits(), b"[Errno 5] message");
    let filename = text(py, b"sample.py");
    let syntax = builtin(py, "SyntaxError", &[message.bits()]);
    set_field(
        py,
        syntax.bits(),
        ExceptionTypedField::SyntaxFilename,
        filename.bits(),
    );
    set_field(
        py,
        syntax.bits(),
        ExceptionTypedField::SyntaxLineNumber,
        MoltObject::from_int(12).bits(),
    );
    str_eq(py, syntax.bits(), b"message (sample.py, line 12)");
    set_field(
        py,
        syntax.bits(),
        ExceptionTypedField::SyntaxLineNumber,
        MoltObject::from_bool(true).bits(),
    );
    str_eq(py, syntax.bits(), b"message (sample.py)");
    let children = tuple(py, &[empty.bits()]);
    let group = builtin(py, "ExceptionGroup", &[message.bits(), children.bits()]);
    let replacement = tuple(py, &[new_message.bits()]);
    exception_replace_field_bits(
        py,
        group.bits(),
        ExceptionFieldSlot::Args,
        replacement.bits(),
    )
    .unwrap();
    str_eq(py, group.bits(), b"message (1 sub-exception)");
    let children = tuple(py, &[empty.bits(), key.bits()]);
    let group = builtin(py, "ExceptionGroup", &[message.bits(), children.bits()]);
    str_eq(py, group.bits(), b"message (2 sub-exceptions)");

    let surrogate = text(py, b"\xed\xa0\x80");
    let error = builtin(py, "ValueError", &[surrogate.bits()]);
    assert_eq!(str_eq(py, error.bits(), b"\xed\xa0\x80"), surrogate.bits());
    assert_text(
        py,
        molt_repr_from_obj(error.bits()),
        b"ValueError('\\ud800')",
    );
    let encoding = text(py, b"ascii");
    let reason = text(py, b"bad");
    let bytes = alloc_bytes(py, &[0xff]);
    assert!(!bytes.is_null());
    let bytes = ExceptionValue::adopt(py, MoltObject::from_ptr(bytes).bits());
    let decode = builtin(
        py,
        "UnicodeDecodeError",
        &[
            encoding.bits(),
            bytes.bits(),
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(1).bits(),
            reason.bits(),
        ],
    );
    str_eq(
        py,
        decode.bits(),
        b"'ascii' codec can't decode byte 0xff in position 0: bad",
    );
    assert_eq!(
        exception_typed_field_delete(py, decode.bits(), ExceptionTypedField::UnicodeReason),
        Some(Ok(()))
    );
    str_eq(
        py,
        decode.bits(),
        b"'ascii' codec can't decode byte 0xff in position 0: <NULL>",
    );
    assert_eq!(
        exception_typed_field_delete(py, decode.bits(), ExceptionTypedField::UnicodeEncoding),
        Some(Ok(()))
    );
    str_eq(
        py,
        decode.bits(),
        b"'<NULL>' codec can't decode byte 0xff in position 0: <NULL>",
    );
    assert_eq!(
        exception_typed_field_delete(py, decode.bits(), ExceptionTypedField::UnicodeObject),
        Some(Ok(()))
    );
    str_eq(py, decode.bits(), b"");
    let encode = builtin(
        py,
        "UnicodeEncodeError",
        &[
            encoding.bits(),
            surrogate.bits(),
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(1).bits(),
            reason.bits(),
        ],
    );
    str_eq(
        py,
        encode.bits(),
        b"'ascii' codec can't encode character '\\ud800' in position 0: bad",
    );
    let translate = builtin(
        py,
        "UnicodeTranslateError",
        &[
            surrogate.bits(),
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(1).bits(),
            reason.bits(),
        ],
    );
    str_eq(
        py,
        translate.bits(),
        b"can't translate character '\\ud800' in position 0: bad",
    );

    let base = exception_type_bits_from_name(py, "ValueError");
    let payload_class = subclass(
        py,
        b"RenderPayload",
        base,
        &[
            (b"__str__", fn_addr!(overridden_str)),
            (b"__repr__", fn_addr!(overridden_repr)),
        ],
    );
    let payload = exception(py, payload_class.bits(), &[]);
    RESULT.with(|slot| slot.set(message.bits()));
    assert_eq!(str_eq(py, payload.bits(), b"message"), message.bits());
    events(&["str"]);
    assert_text(
        py,
        call_declared(py, "BaseException", "__str__", payload.bits()),
        b"",
    );
    events(&[]);
    let renamed = text(py, b"outer.Named");
    assert!(unsafe {
        crate::class_set_name_bits(
            py,
            obj_from_bits(payload_class.bits()).as_ptr().unwrap(),
            renamed.bits(),
        )
    });
    assert_text(
        py,
        call_declared(py, "BaseException", "__repr__", payload.bits()),
        b"Named()",
    );
    let single = builtin(py, "ValueError", &[payload.bits()]);
    OWNER.with(|slot| slot.set(single.bits()));
    REPLACEMENT.with(|slot| slot.set(replacement.bits()));
    assert_eq!(str_eq(py, single.bits(), b"message"), message.bits());
    events(&["str"]);
    str_eq(py, single.bits(), b"new args");
    let outer = builtin(py, "ValueError", &[payload.bits(), new_message.bits()]);
    OWNER.with(|slot| slot.set(outer.bits()));
    REPLACEMENT.with(|slot| slot.set(replacement.bits()));
    assert_text(
        py,
        molt_repr_from_obj(outer.bits()),
        b"ValueError(message, 'new args')",
    );
    events(&["repr"]);
    str_eq(py, outer.bits(), b"new args");
    OWNER.with(|slot| slot.set(0));

    let failure_class = subclass(
        py,
        b"FailingPayload",
        base,
        &[(b"__str__", fn_addr!(failed_str))],
    );
    let failing_payload = exception(py, failure_class.bits(), &[]);
    let failure = builtin(py, "LookupError", &[message.bits()]);
    FAILURE.with(|slot| slot.set(failure.bits()));
    let failing_outer = builtin(py, "ValueError", &[failing_payload.bits()]);
    let result = ExceptionValue::adopt(py, molt_exception_message(failing_outer.bits()));
    assert!(exception_pending(py));
    assert_eq!(
        exception_last_bits_noinc(py),
        Some(failure.bits()),
        "exact callback failure survives snapshot cleanup"
    );
    events(&["failure"]);
    clear_exception(py);
    drop(result);

    let reason_class = subclass(
        py,
        b"ReasonPayload",
        base,
        &[(b"__str__", fn_addr!(unicode_reason))],
    );
    let reason = exception(py, reason_class.bits(), &[]);
    let encoding_class = subclass(
        py,
        b"EncodingPayload",
        base,
        &[(b"__str__", fn_addr!(unicode_encoding))],
    );
    let encoding = exception(py, encoding_class.bits(), &[]);
    let changed_input = text(py, b"z");
    OWNER.with(|slot| slot.set(encode.bits()));
    ENCODING.with(|slot| slot.set(encoding.bits()));
    REPLACEMENT.with(|slot| slot.set(changed_input.bits()));
    set_field(
        py,
        encode.bits(),
        ExceptionTypedField::UnicodeReason,
        reason.bits(),
    );
    str_eq(
        py,
        encode.bits(),
        b"'changed' codec can't encode character '\\x7a' in position 0: reason",
    );
    events(&["reason", "encoding"]);
    assert!(!exception_pending(py));
}

#[test]
fn managed_exception_rendering_uses_declared_slots_and_owned_callback_order() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        assert_managed_rendering_contract(py);
    });
}

/// These fixtures allocate genuine C storage, then commit a public typed C
/// field into an otherwise managed exception. Expected text and exact-only
/// admission follow CPython Objects/exceptions.c, not another Molt renderer.
mod native_fields {
    use super::*;
    use molt_cpython_abi::abi_types::{
        Py_TPFLAGS_HEAPTYPE, Py_TPFLAGS_IMMUTABLETYPE, PyBool_Type, PyLong_Type, PyLongObject,
        PyNumberMethods, PyObject, PySequenceMethods, PyTuple_Type, PyType_Type, PyTypeObject,
        PyUnicode_Type,
    };
    use molt_cpython_abi::api::{errors, object, refcount, sequences, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    use std::ffi::CStr;

    thread_local! {
        static OVERRIDES: Cell<usize> = const { Cell::new(0) };
    }

    unsafe extern "C" fn overridden_length(_object: *mut PyObject) -> isize {
        OVERRIDES.with(|count| count.set(count.get() + 1));
        99
    }

    unsafe extern "C" fn overridden_index(_object: *mut PyObject) -> *mut PyObject {
        OVERRIDES.with(|count| count.set(count.get() + 1));
        unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(99) }
    }

    // Same real-native fixture shape as exception_identity_tests: a live
    // non-heap type with inherited physical allocation/deallocation authority.
    // No managed class registration and no copied dictionary/MRO owners.
    unsafe fn native_subclass(base: *mut PyTypeObject, name: &'static CStr) -> Box<PyTypeObject> {
        let mut class: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
        unsafe {
            class.ob_base.ob_base.ob_refcnt = 1;
            class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            class.tp_name = name.as_ptr();
            class.tp_base = base;
            class.tp_basicsize = (*base).tp_basicsize;
            class.tp_itemsize = (*base).tp_itemsize;
            class.tp_flags = (*base).tp_flags & !(Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_IMMUTABLETYPE);
            class.tp_alloc = (*base).tp_alloc;
            class.tp_free = (*base).tp_free;
            class.tp_dealloc = (*base).tp_dealloc;
            class.tp_traverse = (*base).tp_traverse;
            class.tp_clear = (*base).tp_clear;
            class.tp_is_gc = (*base).tp_is_gc;
            class.tp_getattro = (*base).tp_getattro;
            class.tp_setattro = (*base).tp_setattro;
        }
        class
    }

    unsafe fn native_tuple(
        class: *mut PyTypeObject,
        items: &[*mut PyObject],
    ) -> refcount::OwnedPyObject {
        let value = unsafe {
            refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                class,
                items.len() as isize,
            ))
        };
        assert!(!value.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(value.as_ptr())
                .is_none()
        );
        for (index, &item) in items.iter().enumerate() {
            unsafe { refcount::Py_INCREF(item) };
            assert_eq!(
                unsafe { sequences::PyTuple_SetItem(value.as_ptr(), index as isize, item) },
                0
            );
        }
        value
    }

    /// Construct CPython's public base-2**30 PyLong digits in a real native
    /// allocation. PyLong_FromLong would instead canonicalize to managed bits.
    unsafe fn native_long(class: *mut PyTypeObject, value: i128) -> refcount::OwnedPyObject {
        let magnitude = value.unsigned_abs();
        let count = (128 - magnitude.leading_zeros() as usize).div_ceil(30);
        let number = unsafe {
            refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                class,
                count.max(1) as isize,
            ))
        };
        assert!(!number.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(number.as_ptr())
                .is_none()
        );
        let pointer = number.as_ptr().cast::<PyLongObject>();
        unsafe {
            std::ptr::write_unaligned(
                &raw mut (*pointer).long_value.lv_tag,
                (count << 3)
                    | if value == 0 {
                        1
                    } else if value < 0 {
                        2
                    } else {
                        0
                    },
            );
            let digits = (&raw mut (*pointer).long_value.ob_digit).cast::<u32>();
            for index in 0..count.max(1) {
                digits
                    .add(index)
                    .write_unaligned(((magnitude >> (30 * index)) & 0x3fff_ffff) as u32);
            }
        }
        number
    }

    unsafe fn commit_native_field(
        py: &PyToken<'_>,
        error: u64,
        field: ExceptionTypedField,
        value: *mut PyObject,
    ) -> u64 {
        assert!(
            GLOBAL_BRIDGE.molt_handle_for_pyobj(value).is_none(),
            "native producer required"
        );
        let view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(error) };
        assert!(!view.is_null());
        let layout =
            unsafe { molt_cpython_abi::abi_types::exception_layout_for_type((*view).ob_type) }
                .unwrap();
        let slot = unsafe {
            molt_cpython_abi::abi_types::exception_typed_object_slot(view.cast(), layout, field)
        }
        .unwrap();
        unsafe { refcount::Py_INCREF(value) };
        let old = unsafe { std::ptr::replace(slot, value) };
        errors::with_preserved_error(|| unsafe { refcount::Py_XDECREF(old) });
        let handle = GLOBAL_BRIDGE.managed_handle_for_pyobj(view).unwrap();
        assert!(GLOBAL_BRIDGE.commit_exception_view(handle));
        assert!(!exception_pending(py));
        assert!(unsafe { errors::PyErr_Occurred() }.is_null());
        let bits = unsafe {
            exception_typed_field_raw_bits(obj_from_bits(error).as_ptr().unwrap(), field)
        }
        .unwrap();
        let wrapper = obj_from_bits(bits).as_ptr().unwrap();
        assert_eq!(unsafe { object_type_id(wrapper) }, crate::TYPE_ID_FOREIGN);
        assert_eq!(
            unsafe { crate::object::foreign::foreign_ptr_from_obj(wrapper) },
            value.addr()
        );
        bits
    }

    #[test]
    fn managed_group_counts_real_native_tuple_fields_without_subtype_dispatch() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            let _callbacks = CallbackScope;
            unsafe {
                let mut slots: Box<PySequenceMethods> = Box::new(std::mem::zeroed());
                slots.sq_length = overridden_length as *const () as *mut _;
                let mut class = native_subclass(&raw mut PyTuple_Type, c"rendering.NativeTuple");
                class.tp_as_sequence = (&raw mut *slots).cast();
                let message = text(py, b"group");
                let child = builtin(py, "ValueError", &[message.bits()]);
                let children = tuple(py, &[child.bits()]);
                let group = builtin(py, "ExceptionGroup", &[message.bits(), children.bits()]);
                let child_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(child.bits());
                assert!(!child_view.is_null());
                for (native_class, count, expected) in [
                    (
                        &raw mut PyTuple_Type,
                        1,
                        b"group (1 sub-exception)".as_slice(),
                    ),
                    (&raw mut *class, 2, b"group (2 sub-exceptions)".as_slice()),
                ] {
                    let native = native_tuple(native_class, &vec![child_view; count]);
                    if native_class == &raw mut *class {
                        assert_eq!(object::PyObject_Size(native.as_ptr()), 99);
                    }
                    OVERRIDES.with(|count| count.set(0));
                    commit_native_field(
                        py,
                        group.bits(),
                        ExceptionTypedField::GroupExceptions,
                        native.as_ptr(),
                    );
                    let refs = (*native.as_ptr()).ob_refcnt;
                    assert_text(py, super::super::group_str(group.bits()), expected);
                    assert_text(
                        py,
                        call_declared(py, "BaseExceptionGroup", "__str__", group.bits()),
                        expected,
                    );
                    assert_eq!((*native.as_ptr()).ob_refcnt, refs);
                    assert_eq!(
                        OVERRIDES.with(Cell::get),
                        0,
                        "count physical children, not __len__"
                    );
                }
                // A failed message callback must retain its original exception
                // through native tuple/snapshot retirement.
                let failed = subclass(
                    py,
                    b"NativeFieldFailure",
                    builtin_classes(py).base_exception,
                    &[(b"__str__", fn_addr!(failed_str))],
                );
                let payload = exception(py, failed.bits(), &[]);
                let failure = builtin(py, "LookupError", &[message.bits()]);
                FAILURE.with(|slot| slot.set(failure.bits()));
                // Group fields are read-only to Python; the C typed snapshot
                // may nevertheless publish arbitrary message objects.
                exception_typed_field_replace_internal(
                    py,
                    group.bits(),
                    ExceptionTypedField::GroupMessage,
                    payload.bits(),
                )
                .unwrap();
                let result = ExceptionValue::adopt(py, super::super::group_str(group.bits()));
                assert_eq!(exception_last_bits_noinc(py), Some(failure.bits()));
                events(&["failure"]);
                clear_exception(py);
                drop(result);
            }
        });
    }

    #[test]
    fn managed_syntax_line_reads_exact_native_long_and_c_long_overflow() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let mut number_slots: Box<PyNumberMethods> = Box::new(std::mem::zeroed());
                number_slots.nb_index = overridden_index as *const () as *mut _;
                let mut subclass = native_subclass(&raw mut PyLong_Type, c"rendering.NativeLong");
                subclass.tp_as_number = (&raw mut *number_slots).cast();
                let message = text(py, b"syntax");
                let error = builtin(py, "SyntaxError", &[message.bits()]);
                for (value, expected) in [
                    (0, b"syntax (line 0)".as_slice()),
                    (12, b"syntax (line 12)".as_slice()),
                    (-9, b"syntax (line -9)".as_slice()),
                    (
                        i128::from(libc::c_long::MAX) + 1,
                        b"syntax (line -1)".as_slice(),
                    ),
                    (
                        i128::from(libc::c_long::MIN) - 1,
                        b"syntax (line -1)".as_slice(),
                    ),
                ] {
                    let native = native_long(&raw mut PyLong_Type, value);
                    commit_native_field(
                        py,
                        error.bits(),
                        ExceptionTypedField::SyntaxLineNumber,
                        native.as_ptr(),
                    );
                    assert_text(py, super::super::syntax_str(error.bits()), expected);
                    assert!(
                        errors::PyErr_Occurred().is_null(),
                        "C-long overflow is not an error"
                    );
                }
                for class in [&raw mut *subclass, &raw mut PyBool_Type] {
                    let native = native_long(class, 1);
                    commit_native_field(
                        py,
                        error.bits(),
                        ExceptionTypedField::SyntaxLineNumber,
                        native.as_ptr(),
                    );
                    OVERRIDES.with(|count| count.set(0));
                    assert_eq!(
                        assert_text(py, super::super::syntax_str(error.bits()), b"syntax"),
                        message.bits()
                    );
                    assert_eq!(
                        OVERRIDES.with(Cell::get),
                        0,
                        "exact-only read must not invoke __index__"
                    );
                }
            }
        });
    }

    #[test]
    fn managed_unicode_decode_preserves_native_input_type_diagnostic() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let encoding = text(py, b"codec");
                let reason = text(py, b"reason");
                let pointer = alloc_bytes(py, b"x");
                assert!(!pointer.is_null());
                let bytes = ExceptionValue::adopt(py, MoltObject::from_ptr(pointer).bits());
                let error = builtin(
                    py,
                    "UnicodeDecodeError",
                    &[
                        encoding.bits(),
                        bytes.bits(),
                        MoltObject::from_int(0).bits(),
                        MoltObject::from_int(1).bits(),
                        reason.bits(),
                    ],
                );
                let native = native_long(&raw mut PyLong_Type, 12);
                commit_native_field(
                    py,
                    error.bits(),
                    ExceptionTypedField::UnicodeObject,
                    native.as_ptr(),
                );
                let result =
                    ExceptionValue::adopt(py, super::super::unicode_decode_str(error.bits()));
                let failure =
                    ExceptionValue::pin(py, exception_last_bits_noinc(py).expect("typed error"));
                assert!(exception_matches_type(
                    py,
                    failure.bits(),
                    exception_type_bits_from_name(py, "TypeError")
                ));
                clear_exception(py);
                assert_text(
                    py,
                    super::super::exception_str(failure.bits()),
                    b"expected bytes, int found",
                );
                assert!(errors::PyErr_Occurred().is_null());
                drop(result);
            }
        });
    }

    #[test]
    fn native_string_field_type_checks_use_physical_identity_without_value_projection() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let mut subclass =
                    native_subclass(&raw mut PyUnicode_Type, c"rendering.NativeString");
                let message = text(py, b"fallback");
                let error = builtin(py, "ImportError", &[message.bits()]);
                // No payload read is requested here: ImportError's exact-str
                // slot returns msg by identity. Raw native text extraction is
                // separately owned by the C unicode storage authority.
                for (class, exact) in [(&raw mut PyUnicode_Type, true), (&raw mut *subclass, false)]
                {
                    let native =
                        refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class, 0));
                    assert!(!native.as_ptr().is_null());
                    let bits = commit_native_field(
                        py,
                        error.bits(),
                        ExceptionTypedField::ImportMessage,
                        native.as_ptr(),
                    );
                    let context = ManagedRender(py);
                    assert!(context.is_string(bits, false));
                    assert_eq!(context.is_string(bits, true), exact);
                    let result = ExceptionValue::adopt(py, super::super::import_str(error.bits()));
                    assert!(!exception_pending(py));
                    assert_eq!(result.bits(), if exact { bits } else { message.bits() });
                }
            }
        });
    }
}
