//! Mask-proof tests for the F4 typeobj.rs rows beyond Str/Repr:
//! PyType_IsSubtype (tp_mro / base-chain + object terminal), PyType_Check
//! (metatype subtype walk), PyType_GetName (dotted-prefix strip),
//! PyObject_Hash (native value hash + unhashable TypeError), PyType_GenericNew
//! (tp_alloc dispatch), PyMember_SetOne (numeric/bool/char writes + delete
//! rules), PyObject_RichCompare/Bool (reflected + both-NotImplemented + identity).

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    Py_False, Py_NotImplementedSentinel, Py_True, PyMemberDef, PyObject, PyTypeObject,
};
use molt_cpython_abi::hooks::RuntimeHooks;
use std::os::raw::c_int;
use std::ptr;
use std::sync::Mutex;

// The shared fixture supplies real string/numeric payload and edge ownership.
fn install() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    support::prepare_runtime_class_abi_test_thread(hooks);
}
unsafe fn read_str(py: *mut PyObject) -> Vec<u8> {
    let mut length = 0;
    let data =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8AndSize(py, &raw mut length) };
    assert!(!data.is_null() && length >= 0);
    unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length as usize) }.to_vec()
}

fn new_type() -> Box<PyTypeObject> {
    Box::new(unsafe { std::mem::zeroed() })
}
fn leak_type(t: Box<PyTypeObject>) -> *mut PyTypeObject {
    Box::into_raw(t)
}
fn make_instance(ty: *mut PyTypeObject) -> *mut PyObject {
    Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ty,
    }))
}

// ===========================================================================
// PyType_IsSubtype
// ===========================================================================

#[test]
fn issubtype_base_chain_and_object_terminal() {
    install();
    let object = &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type;
    let mut a = new_type();
    a.tp_base = object;
    let a = leak_type(a);
    let mut b = new_type();
    b.tp_base = a;
    let b = leak_type(b);

    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_IsSubtype(b, a) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_IsSubtype(b, object) },
        1,
        "every type is a subtype of object"
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_IsSubtype(a, b) },
        0
    );

    // Uninitialized type (tp_base == NULL, tp_mro == NULL): the base-chain
    // terminal must still report subtype-of-object.
    let orphan = leak_type(new_type());
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_IsSubtype(orphan, object) },
        1,
        "chain-end terminal: b == object"
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_IsSubtype(orphan, a) },
        0
    );
}

// ===========================================================================
// PyType_Check — metatype subtype walk (numpy DType metaclass shape)
// ===========================================================================

#[test]
fn type_check_accepts_metaclass_subclass_instances() {
    install();
    let type_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    // A metaclass M whose base is `type`.
    let mut meta = new_type();
    meta.tp_base = type_type;
    let meta = leak_type(meta);
    // A type object whose METAtype is M (i.e. Py_TYPE(op) == M).
    let mut cls = new_type();
    cls.ob_base.ob_base.ob_type = meta;
    let cls = leak_type(cls);
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_Check(cls.cast()) },
        1,
        "a metaclass instance must pass PyType_Check (PyType_CheckExact would fail)"
    );

    // A plain instance whose type does NOT subclass `type` is not a type.
    let mut plain_ty = new_type();
    plain_ty.tp_base = &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type;
    let plain_ty = leak_type(plain_ty);
    let inst = make_instance(plain_ty);
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_Check(inst) },
        0
    );
}

// ===========================================================================
// PyType_GetName — strip dotted prefix for non-heap types
// ===========================================================================

#[test]
fn get_name_strips_dotted_module_prefix() {
    install();
    let mut ty = new_type();
    ty.tp_name = c"numpy.dtypes.BoolDType".as_ptr();
    let ty = leak_type(ty);
    let name = unsafe { molt_cpython_abi::api::typeobj::PyType_GetName(ty) };
    assert!(!name.is_null());
    assert_eq!(unsafe { read_str(name) }, b"BoolDType");
    // Qualname delegates to the same short name.
    let qn = unsafe { molt_cpython_abi::api::typeobj::PyType_GetQualName(ty) };
    assert_eq!(unsafe { read_str(qn) }, b"BoolDType");
}

// ===========================================================================
// PyObject_Hash — native value hash + unhashable TypeError
// ===========================================================================

#[test]
fn hash_of_native_int_is_its_value() {
    install();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1234) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(py) },
        1234
    );
}

#[test]
fn hash_of_unhashable_foreign_raises_typeerror() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut ty = new_type();
    ty.tp_name = c"Unhashable".as_ptr();
    ty.tp_hash = Some(molt_cpython_abi::api::typeobj::PyObject_HashNotImplemented);
    let ty = leak_type(ty);
    let inst = make_instance(ty);
    let h = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(inst) };
    assert_eq!(h, -1, "unhashable must return -1, not a pointer hash");
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "must set a TypeError"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ===========================================================================
// PyType_GenericNew — dispatch the type's own tp_alloc
// ===========================================================================

static ALLOC_CALLED: Mutex<bool> = Mutex::new(false);
unsafe extern "C" fn custom_alloc(_t: *mut PyTypeObject, _n: isize) -> *mut PyObject {
    *ALLOC_CALLED.lock().unwrap() = true;
    Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    }))
}

#[test]
fn generic_new_dispatches_custom_tp_alloc() {
    install();
    *ALLOC_CALLED.lock().unwrap() = false;
    let mut ty = new_type();
    ty.tp_alloc = Some(custom_alloc);
    let ty = leak_type(ty);
    let obj = unsafe {
        molt_cpython_abi::api::typeobj::PyType_GenericNew(ty, ptr::null_mut(), ptr::null_mut())
    };
    assert!(!obj.is_null());
    assert!(
        *ALLOC_CALLED.lock().unwrap(),
        "PyType_GenericNew must call the type's own tp_alloc slot"
    );
}

// ===========================================================================
// PyMember_SetOne — numeric / bool / char writes + delete rules
// ===========================================================================

const T_INT: c_int = 1;
const T_BOOL: c_int = 14;
const T_CHAR: c_int = 7;

fn member(type_: c_int, offset: isize) -> PyMemberDef {
    let mut m: PyMemberDef = unsafe { std::mem::zeroed() };
    m.name = c"field".as_ptr();
    m.type_ = type_;
    m.offset = offset;
    m.flags = 0;
    m
}

#[test]
fn set_one_writes_int_member() {
    install();
    let mut storage: [u8; 32] = [0; 32];
    let mut m = member(T_INT, 0);
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(999) };
    let rc = unsafe {
        molt_cpython_abi::api::typeobj::PyMember_SetOne(storage.as_mut_ptr().cast(), &mut m, v)
    };
    assert_eq!(rc, 0);
    let got = i32::from_ne_bytes(storage[0..4].try_into().unwrap());
    assert_eq!(
        got, 999,
        "T_INT member must be written (was a fail-closed no-op)"
    );
}

#[test]
fn set_one_bool_rejects_non_bool() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut storage: [u8; 8] = [0; 8];
    let mut m = member(T_BOOL, 0);
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let rc = unsafe {
        molt_cpython_abi::api::typeobj::PyMember_SetOne(storage.as_mut_ptr().cast(), &mut m, v)
    };
    assert_eq!(rc, -1, "T_BOOL rejects a non-bool value");
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // A real bool is accepted.
    let rc2 = unsafe {
        molt_cpython_abi::api::typeobj::PyMember_SetOne(
            storage.as_mut_ptr().cast(),
            &mut m,
            (&raw mut Py_True).cast::<PyObject>(),
        )
    };
    assert_eq!(rc2, 0);
    assert_eq!(storage[0], 1);
}

#[test]
fn set_one_char_requires_single_char_string() {
    install();
    let mut storage: [u8; 8] = [0; 8];
    let mut m = member(T_CHAR, 0);
    let v = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"Q".as_ptr()) };
    let rc = unsafe {
        molt_cpython_abi::api::typeobj::PyMember_SetOne(storage.as_mut_ptr().cast(), &mut m, v)
    };
    assert_eq!(rc, 0);
    assert_eq!(storage[0], b'Q');
}

#[test]
fn set_one_delete_numeric_is_typeerror() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut storage: [u8; 8] = [0; 8];
    let mut m = member(T_INT, 0);
    let rc = unsafe {
        molt_cpython_abi::api::typeobj::PyMember_SetOne(
            storage.as_mut_ptr().cast(),
            &mut m,
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, -1, "deleting a numeric member is a TypeError");
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// The oracle is the pinned CPython 3.12/3.13/3.14 structmember contract. Test
// inputs use the public byte-array integer constructor; expected field bytes
// come from target C widths, not from PyMember_GetOne or another setter.
use molt_cpython_abi::abi_types as member_abi;
use molt_cpython_abi::api::{errors, numbers, object, refcount, typeobj};
use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_long, c_ulong, c_void};

const T_SHORT: c_int = 0;
const T_LONG: c_int = 2;
const T_FLOAT: c_int = 3;
const T_DOUBLE: c_int = 4;
const T_BYTE: c_int = 8;
const T_UBYTE: c_int = 9;
const T_USHORT: c_int = 10;
const T_UINT: c_int = 11;
const T_ULONG: c_int = 12;
const T_LONGLONG: c_int = 17;
const T_ULONGLONG: c_int = 18;
const T_PYSSIZET: c_int = 19;

thread_local! {
    static MEMBER_TARGET: Cell<i64> = const { Cell::new(12) };
    static MEMBER_INDEX_CALLS: Cell<usize> = const { Cell::new(0) };
    static MEMBER_RESULT: Cell<usize> = const { Cell::new(0) };
    static MEMBER_ERROR: Cell<usize> = const { Cell::new(0) };
    static MEMBER_TRANSFER_RESULT: Cell<bool> = const { Cell::new(false) };
    static MEMBER_STORAGE: Cell<usize> = const { Cell::new(0) };
    static MEMBER_WARNING_BYTES: RefCell<Vec<[u8; 24]>> = const { RefCell::new(Vec::new()) };
    static MEMBER_FINALIZERS: Cell<usize> = const { Cell::new(0) };
    static MEMBER_FINALIZER_BYTES: Cell<[u8; 24]> = const { Cell::new([0; 24]) };
}

unsafe extern "C" fn member_target_minor() -> i64 {
    MEMBER_TARGET.with(Cell::get)
}

unsafe extern "C" fn member_index(_object: *mut PyObject) -> *mut PyObject {
    MEMBER_INDEX_CALLS.with(|count| count.set(count.get() + 1));
    let error = MEMBER_ERROR.with(Cell::get) as *mut PyObject;
    if !error.is_null() {
        unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(error)) };
        return ptr::null_mut();
    }
    let result = MEMBER_RESULT.with(Cell::get) as *mut PyObject;
    if MEMBER_TRANSFER_RESULT.with(Cell::get) {
        result
    } else {
        unsafe { object::Py_NewRef(result) }
    }
}

fn member_storage_snapshot() -> [u8; 24] {
    let addr = MEMBER_STORAGE.with(Cell::get) as *const [u8; 24];
    assert!(!addr.is_null());
    unsafe { *addr }
}

fn observe_member_warning() {
    MEMBER_WARNING_BYTES.with(|observations| {
        observations.borrow_mut().push(member_storage_snapshot());
    });
}

unsafe extern "C" fn member_result_finalizer(_object: *mut PyObject) {
    MEMBER_FINALIZERS.with(|count| count.set(count.get() + 1));
    MEMBER_FINALIZER_BYTES.with(|bytes| bytes.set(member_storage_snapshot()));
    unsafe {
        errors::PyErr_SetString(
            (&raw mut member_abi::PyExc_LookupError).cast(),
            c"member temporary finalizer".as_ptr(),
        )
    };
}

fn install_member_protocol() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    hooks.target_python_minor = member_target_minor;
    hooks.import_module = support::warnings::import_module;
    support::prepare_runtime_class_abi_test_thread(hooks);
    MEMBER_TARGET.with(|value| value.set(12));
    MEMBER_RESULT.with(|value| value.set(0));
    MEMBER_ERROR.with(|value| value.set(0));
    MEMBER_TRANSFER_RESULT.with(|value| value.set(false));
    MEMBER_FINALIZERS.with(|value| value.set(0));
}

unsafe fn member_integer(value: i128) -> refcount::OwnedPyObject {
    let bytes = value.to_le_bytes();
    let result = unsafe { numbers::_PyLong_FromByteArray(bytes.as_ptr(), bytes.len(), 1, 1) };
    assert!(!result.is_null());
    unsafe { refcount::OwnedPyObject::from_owned(result) }
}

unsafe fn write_member(ty: c_int, value: *mut PyObject, offset: usize) -> (c_int, [u8; 24]) {
    // Wide fields deliberately use offset 1. Other fields start aligned.
    let mut storage = [u64::from_ne_bytes([0xa5; 8]); 3];
    let mut descriptor = member(ty, offset as isize);
    MEMBER_INDEX_CALLS.with(|value| value.set(0));
    MEMBER_WARNING_BYTES.with(|value| value.borrow_mut().clear());
    MEMBER_STORAGE.with(|value| value.set(storage.as_mut_ptr() as usize));
    let result = unsafe {
        typeobj::PyMember_SetOne(storage.as_mut_ptr().cast(), &raw mut descriptor, value)
    };
    let bytes = member_storage_snapshot();
    MEMBER_STORAGE.with(|value| value.set(0));
    (result, bytes)
}

fn expected_member_bytes(offset: usize, field: &[u8]) -> [u8; 24] {
    let mut expected = [0xa5; 24];
    expected[offset..offset + field.len()].copy_from_slice(field);
    expected
}

fn integer_field_bytes(ty: c_int, value: i128) -> Vec<u8> {
    match ty {
        T_BYTE | T_UBYTE => vec![value as u8],
        T_SHORT | T_USHORT => (value as u16).to_ne_bytes().to_vec(),
        T_INT | T_UINT => (value as u32).to_ne_bytes().to_vec(),
        T_LONG | T_ULONG => (value as c_ulong).to_ne_bytes().to_vec(),
        T_PYSSIZET => (value as usize).to_ne_bytes().to_vec(),
        T_LONGLONG | T_ULONGLONG => (value as u64).to_ne_bytes().to_vec(),
        _ => panic!("noninteger fixture type"),
    }
}

fn assert_member_warning(message: &str, expected: [u8; 24]) {
    let warnings = support::warnings::emissions();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].message, message);
    assert_eq!(
        warnings[0].category,
        (&raw mut member_abi::PyExc_RuntimeWarning) as usize
    );
    assert_eq!(warnings[0].stacklevel, 1);
    MEMBER_WARNING_BYTES.with(|value| assert_eq!(*value.borrow(), vec![expected]));
}

#[test]
fn member_narrow_boundaries_warn_after_write_including_warning_errors() {
    install_member_protocol();
    support::warnings::with_provider(|| unsafe {
        support::warnings::set_observer(Some(observe_member_warning));
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|value| value.set(minor));
            for (ty, minimum, maximum, message) in [
                (
                    T_BYTE,
                    c_char::MIN as i128,
                    c_char::MAX as i128,
                    "Truncation of value to char",
                ),
                (
                    T_UBYTE,
                    0,
                    u8::MAX as i128,
                    "Truncation of value to unsigned char",
                ),
                (
                    T_SHORT,
                    i16::MIN as i128,
                    i16::MAX as i128,
                    "Truncation of value to short",
                ),
                (
                    T_USHORT,
                    0,
                    u16::MAX as i128,
                    "Truncation of value to unsigned short",
                ),
                (
                    T_INT,
                    c_int::MIN as i128,
                    c_int::MAX as i128,
                    "Truncation of value to int",
                ),
            ] {
                for input in [minimum - 1, minimum, maximum, maximum + 1] {
                    let integer = member_integer(input);
                    for warning_error in [false, true] {
                        support::warnings::clear();
                        support::warnings::set_as_error(warning_error);
                        let (result, bytes) = write_member(ty, integer.as_ptr(), 0);
                        if input < c_long::MIN as i128 || input > c_long::MAX as i128 {
                            assert_eq!(result, -1);
                            assert_eq!(bytes, [0xa5; 24]);
                            assert_eq!(
                                errors::PyErr_ExceptionMatches(
                                    (&raw mut member_abi::PyExc_OverflowError).cast()
                                ),
                                1
                            );
                            assert!(support::warnings::emissions().is_empty());
                        } else {
                            let expected =
                                expected_member_bytes(0, &integer_field_bytes(ty, input));
                            assert_eq!(bytes, expected);
                            if input < minimum || input > maximum {
                                assert_eq!(result, if warning_error { -1 } else { 0 });
                                assert_member_warning(message, expected);
                                if warning_error {
                                    assert_eq!(
                                        errors::PyErr_ExceptionMatches(
                                            (&raw mut member_abi::PyExc_RuntimeWarning).cast()
                                        ),
                                        1
                                    );
                                    assert_eq!(
                                        support::take_current_error_text().as_deref(),
                                        Some(message)
                                    );
                                }
                            } else {
                                assert_eq!(result, 0);
                                assert!(support::warnings::emissions().is_empty());
                            }
                        }
                        if result == 0 {
                            assert!(errors::PyErr_Occurred().is_null());
                        }
                        errors::PyErr_Clear();
                    }
                }
            }
        }
    });
}

#[test]
fn member_unsigned_index_once_full_width_and_c_long_negative_boundary() {
    install_member_protocol();
    support::warnings::with_provider(|| unsafe {
        support::warnings::set_observer(Some(observe_member_warning));
        let mut slots: member_abi::PyNumberMethods = std::mem::zeroed();
        slots.nb_index = member_index as *const () as *mut c_void;
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        class.tp_name = c"MemberIndex".as_ptr();
        class.tp_as_number = (&raw mut slots).cast();
        let mut input_object = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|value| value.set(minor));
            for ty in [T_UINT, T_ULONG, T_ULONGLONG] {
                let maximum = if ty == T_ULONGLONG {
                    u64::MAX as i128
                } else {
                    c_ulong::MAX as i128
                };
                let offset = usize::from(ty == T_ULONGLONG);
                for input in [
                    -1,
                    c_long::MIN as i128,
                    c_long::MIN as i128 - 1,
                    0,
                    u32::MAX as i128,
                    u32::MAX as i128 + 1,
                    maximum,
                    maximum + 1,
                ] {
                    let integer = member_integer(input);
                    let refs = (*integer.as_ptr()).ob_refcnt;
                    MEMBER_RESULT.with(|value| value.set(integer.as_ptr() as usize));
                    for warning_error in [false, true] {
                        support::warnings::clear();
                        support::warnings::set_as_error(warning_error);
                        let (result, bytes) = write_member(ty, &raw mut input_object, offset);
                        assert_eq!(MEMBER_INDEX_CALLS.with(Cell::get), 1);
                        assert_eq!((*integer.as_ptr()).ob_refcnt, refs);
                        if input < c_long::MIN as i128 || input > maximum {
                            assert_eq!(result, -1);
                            assert_eq!(bytes, [0xa5; 24]);
                            assert_eq!(
                                errors::PyErr_ExceptionMatches(
                                    (&raw mut member_abi::PyExc_OverflowError).cast()
                                ),
                                1
                            );
                            assert!(support::warnings::emissions().is_empty());
                        } else {
                            let expected =
                                expected_member_bytes(offset, &integer_field_bytes(ty, input));
                            assert_eq!(bytes, expected);
                            let warning = if input < 0 {
                                Some("Writing negative value into unsigned field")
                            } else if ty == T_UINT && input > u32::MAX as i128 {
                                Some("Truncation of value to unsigned int")
                            } else {
                                None
                            };
                            if let Some(message) = warning {
                                assert_eq!(result, if warning_error { -1 } else { 0 });
                                assert_member_warning(message, expected);
                                if warning_error {
                                    assert_eq!(
                                        support::take_current_error_text().as_deref(),
                                        Some(message)
                                    );
                                }
                            } else {
                                assert_eq!(result, 0);
                                assert!(support::warnings::emissions().is_empty());
                            }
                        }
                        if result == 0 {
                            assert!(errors::PyErr_Occurred().is_null());
                        }
                        errors::PyErr_Clear();
                    }
                }
            }
        }
        MEMBER_RESULT.with(|value| value.set(0));
        assert_eq!(input_object.ob_refcnt, 1);
    });
}

#[test]
fn member_converter_errors_preserve_identity_and_versioned_write_order() {
    install_member_protocol();
    support::warnings::with_provider(|| unsafe {
        let mut slots: member_abi::PyNumberMethods = std::mem::zeroed();
        slots.nb_index = member_index as *const () as *mut c_void;
        slots.nb_float = member_index as *const () as *mut c_void;
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        class.tp_name = c"FailingMemberNumber".as_ptr();
        class.tp_as_number = (&raw mut slots).cast();
        let mut input = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        errors::PyErr_SetString(
            (&raw mut member_abi::PyExc_ValueError).cast(),
            c"member callback failed exactly".as_ptr(),
        );
        let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!error.as_ptr().is_null());
        MEMBER_ERROR.with(|value| value.set(error.as_ptr() as usize));
        let refs = (*error.as_ptr()).ob_refcnt;
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|value| value.set(minor));
            for ty in [
                T_BYTE,
                T_UBYTE,
                T_SHORT,
                T_USHORT,
                T_INT,
                T_UINT,
                T_LONG,
                T_ULONG,
                T_LONGLONG,
                T_ULONGLONG,
                T_FLOAT,
                T_DOUBLE,
            ] {
                let offset = usize::from(matches!(ty, T_LONGLONG | T_ULONGLONG | T_DOUBLE));
                let (result, bytes) = write_member(ty, &raw mut input, offset);
                assert_eq!(result, -1);
                assert_eq!(MEMBER_INDEX_CALLS.with(Cell::get), 1);
                let raised = errors::PyErr_GetRaisedException();
                assert_eq!(raised, error.as_ptr());
                refcount::Py_DECREF(raised);
                assert_eq!((*error.as_ptr()).ob_refcnt, refs);
                let expected = if minor == 12 && matches!(ty, T_LONG | T_LONGLONG | T_DOUBLE) {
                    let field = if ty == T_DOUBLE {
                        (-1.0_f64).to_ne_bytes().to_vec()
                    } else {
                        integer_field_bytes(ty, -1)
                    };
                    expected_member_bytes(offset, &field)
                } else {
                    [0xa5; 24]
                };
                assert_eq!(bytes, expected);
                assert!(support::warnings::emissions().is_empty());
            }
            // Ssize_t is intentionally strict, unlike the signed index users.
            let (result, bytes) = write_member(T_PYSSIZET, &raw mut input, 0);
            assert_eq!(result, -1);
            assert_eq!(MEMBER_INDEX_CALLS.with(Cell::get), 0);
            assert_eq!(
                errors::PyErr_ExceptionMatches((&raw mut member_abi::PyExc_TypeError).cast()),
                1
            );
            assert_eq!(
                bytes,
                if minor == 12 {
                    expected_member_bytes(0, &integer_field_bytes(T_PYSSIZET, -1))
                } else {
                    [0xa5; 24]
                }
            );
            errors::PyErr_Clear();
        }
        MEMBER_ERROR.with(|value| value.set(0));
        assert_eq!(input.ob_refcnt, 1);
    });
}

#[test]
fn member_wide_signed_boundaries_and_legitimate_minus_one() {
    install_member_protocol();
    support::warnings::with_provider(|| unsafe {
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|value| value.set(minor));
            for (ty, minimum, maximum) in [
                (T_LONG, c_long::MIN as i128, c_long::MAX as i128),
                (T_PYSSIZET, isize::MIN as i128, isize::MAX as i128),
                (T_LONGLONG, i64::MIN as i128, i64::MAX as i128),
            ] {
                for input in [minimum - 1, minimum, -1, maximum, maximum + 1] {
                    let integer = member_integer(input);
                    let offset = usize::from(ty == T_LONGLONG);
                    let (result, bytes) = write_member(ty, integer.as_ptr(), offset);
                    let overflow = input < minimum || input > maximum;
                    assert_eq!(result, if overflow { -1 } else { 0 });
                    let expected = if overflow && minor >= 13 {
                        [0xa5; 24]
                    } else {
                        expected_member_bytes(
                            offset,
                            &integer_field_bytes(ty, if overflow { -1 } else { input }),
                        )
                    };
                    assert_eq!(bytes, expected);
                    if overflow {
                        assert_eq!(
                            errors::PyErr_ExceptionMatches(
                                (&raw mut member_abi::PyExc_OverflowError).cast()
                            ),
                            1
                        );
                    } else {
                        assert!(errors::PyErr_Occurred().is_null());
                    }
                    assert!(support::warnings::emissions().is_empty());
                    errors::PyErr_Clear();
                }
            }
            let real = refcount::OwnedPyObject::from_owned(numbers::PyFloat_FromDouble(-1.0));
            for ty in [T_FLOAT, T_DOUBLE] {
                let offset = usize::from(ty == T_DOUBLE);
                let (result, bytes) = write_member(ty, real.as_ptr(), offset);
                assert_eq!(result, 0);
                let field = if ty == T_DOUBLE {
                    (-1.0_f64).to_ne_bytes().to_vec()
                } else {
                    (-1.0_f32).to_ne_bytes().to_vec()
                };
                assert_eq!(bytes, expected_member_bytes(offset, &field));
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}

#[test]
fn member_index_subtype_warning_precedes_release_write_and_negative_warning() {
    install_member_protocol();
    support::warnings::with_provider(|| unsafe {
        support::warnings::set_observer(Some(observe_member_warning));
        let mut slots: member_abi::PyNumberMethods = std::mem::zeroed();
        slots.nb_index = member_index as *const () as *mut c_void;
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        class.tp_name = c"MemberIndex".as_ptr();
        class.tp_as_number = (&raw mut slots).cast();
        let mut input = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        let mut subtype = support::StaticType::new();
        subtype.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        subtype.tp_base = &raw mut member_abi::PyLong_Type;
        subtype.tp_name = c"MemberIndexResult".as_ptr();
        subtype.tp_as_number = (&raw mut slots).cast();
        subtype.tp_dealloc = Some(member_result_finalizer);
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|value| value.set(minor));
            for warning_error in [false, true] {
                let mut result = member_abi::PyLongObject {
                    ob_base: PyObject {
                        ob_refcnt: 1,
                        ob_type: subtype.as_ptr(),
                    },
                    long_value: member_abi::PyLongValue {
                        lv_tag: 10,
                        ob_digit: [1],
                    },
                };
                MEMBER_RESULT.with(|value| value.set((&raw mut result) as usize));
                MEMBER_TRANSFER_RESULT.with(|value| value.set(true));
                MEMBER_FINALIZERS.with(|value| value.set(0));
                support::warnings::clear();
                support::warnings::set_as_error(warning_error);
                let (status, bytes) = write_member(T_UINT, &raw mut input, 0);
                assert_eq!(MEMBER_INDEX_CALLS.with(Cell::get), 1);
                assert_eq!(MEMBER_FINALIZERS.with(Cell::get), 1);
                assert_eq!(MEMBER_FINALIZER_BYTES.with(Cell::get), [0xa5; 24]);
                let warnings = support::warnings::emissions();
                assert_eq!(warnings.len(), if warning_error { 1 } else { 2 });
                assert!(
                    warnings[0]
                        .message
                        .starts_with("__index__ returned non-int (type MemberIndexResult)")
                );
                assert_eq!(
                    warnings[0].category,
                    (&raw mut member_abi::PyExc_DeprecationWarning) as usize
                );
                if warning_error {
                    assert_eq!(status, -1);
                    assert_eq!(bytes, [0xa5; 24]);
                    assert_eq!(warnings.len(), 1);
                    assert_eq!(
                        support::take_current_error_text().as_deref(),
                        Some(warnings[0].message.as_str())
                    );
                    MEMBER_WARNING_BYTES
                        .with(|value| assert_eq!(*value.borrow(), vec![[0xa5; 24]]));
                } else {
                    let expected = expected_member_bytes(0, &u32::MAX.to_ne_bytes());
                    assert_eq!(status, 0);
                    assert_eq!(bytes, expected);
                    assert_eq!(warnings.len(), 2);
                    assert_eq!(
                        warnings[1].message,
                        "Writing negative value into unsigned field"
                    );
                    assert_eq!(
                        warnings[1].category,
                        (&raw mut member_abi::PyExc_RuntimeWarning) as usize
                    );
                    MEMBER_WARNING_BYTES
                        .with(|value| assert_eq!(*value.borrow(), vec![[0xa5; 24], expected]));
                    assert!(
                        errors::PyErr_Occurred().is_null(),
                        "temporary finalizer error must not escape"
                    );
                }
            }
        }
        MEMBER_RESULT.with(|value| value.set(0));
        MEMBER_TRANSFER_RESULT.with(|value| value.set(false));
        assert_eq!(input.ob_refcnt, 1);
    });
}

// ===========================================================================
// PyObject_RichCompare / RichCompareBool
// ===========================================================================

unsafe extern "C" fn cmp_true(_v: *mut PyObject, _w: *mut PyObject, _op: c_int) -> *mut PyObject {
    (&raw mut Py_True).cast::<PyObject>()
}
unsafe extern "C" fn cmp_false(_v: *mut PyObject, _w: *mut PyObject, _op: c_int) -> *mut PyObject {
    (&raw mut Py_False).cast::<PyObject>()
}
unsafe extern "C" fn cmp_notimpl(
    _v: *mut PyObject,
    _w: *mut PyObject,
    _op: c_int,
) -> *mut PyObject {
    &raw mut Py_NotImplementedSentinel
}
unsafe extern "C" fn cmp_error(_v: *mut PyObject, _w: *mut PyObject, _op: c_int) -> *mut PyObject {
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>(),
            c"boom".as_ptr(),
        );
    }
    ptr::null_mut()
}

const PY_EQ: c_int = 2;
const PY_LT: c_int = 0;

#[test]
fn richcompare_reflected_subtype_priority() {
    install();
    // Base with a slot that says False; Sub (subtype of Base) with a slot that
    // says True. Comparing base_inst == sub_inst must consult Sub's reflected
    // slot FIRST (subtype priority), yielding True.
    let object = &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type;
    let mut base = new_type();
    base.tp_base = object;
    base.tp_richcompare = Some(cmp_false);
    let base = leak_type(base);
    let mut sub = new_type();
    sub.tp_base = base;
    sub.tp_richcompare = Some(cmp_true);
    let sub = leak_type(sub);

    let base_inst = make_instance(base);
    let sub_inst = make_instance(sub);
    let res =
        unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompare(base_inst, sub_inst, PY_EQ) };
    assert!(
        std::ptr::eq(res, (&raw mut Py_True).cast::<PyObject>()),
        "reflected subtype slot wins"
    );
}

#[test]
fn richcompare_both_notimplemented_resolves_identity_and_ordering() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut ty = new_type();
    ty.tp_base = &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type;
    ty.tp_richcompare = Some(cmp_notimpl);
    let ty = leak_type(ty);
    let a = make_instance(ty);
    let b = make_instance(ty);

    // EQ of two distinct objects: both NotImplemented -> identity -> False.
    let eq = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompare(a, b, PY_EQ) };
    assert!(
        std::ptr::eq(eq, (&raw mut Py_False).cast::<PyObject>()),
        "both-NotImplemented EQ resolves by identity, never leaks NotImplemented"
    );
    assert!(!std::ptr::eq(eq, &raw mut Py_NotImplementedSentinel));

    // Ordering with both NotImplemented -> TypeError + NULL.
    let lt = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompare(a, b, PY_LT) };
    assert!(
        lt.is_null(),
        "unsupported ordering must raise, not return NotImplemented"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn richcompare_propagates_slot_error() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut ty = new_type();
    ty.tp_base = &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type;
    ty.tp_richcompare = Some(cmp_error);
    let ty = leak_type(ty);
    let a = make_instance(ty);
    let b = make_instance(ty);
    let res = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompare(a, b, PY_EQ) };
    assert!(
        res.is_null(),
        "a NULL slot result must propagate, not mask the error"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn richcomparebool_identity_shortcut() {
    install();
    let mut ty = new_type();
    ty.tp_base = &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type;
    // A slot that would say NotEqual, to prove the identity shortcut wins.
    ty.tp_richcompare = Some(cmp_false);
    let ty = leak_type(ty);
    let a = make_instance(ty);
    // v == w identity: EQ -> 1 before any slot dispatch.
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(a, a, PY_EQ) },
        1
    );
}

#[test]
fn heap_names_use_distinct_live_unicode_fields() {
    use molt_cpython_abi::abi_types::{Py_TPFLAGS_HEAPTYPE, PyHeapTypeObject};
    use molt_cpython_abi::api::{refcount, strings, typeobj};
    install();
    let heap = unsafe {
        typeobj::PyType_GenericAlloc(&raw mut molt_cpython_abi::abi_types::PyType_Type, 0)
    }
    .cast::<PyHeapTypeObject>();
    assert!(!heap.is_null());
    let heap = unsafe { &mut *heap };
    heap.ht_type.tp_flags = Py_TPFLAGS_HEAPTYPE;
    heap.ht_type.tp_name = c"cached.Unrelated".as_ptr();
    unsafe {
        heap.ht_name = strings::PyUnicode_FromString(c"prefix.Name".as_ptr());
        heap.ht_qualname = strings::PyUnicode_FromString(c"Outer.Qualified".as_ptr());
        let tp = &raw mut heap.ht_type;
        let name = typeobj::PyType_GetName(tp);
        let qualified = typeobj::PyType_GetQualName(tp);
        assert_eq!(name, heap.ht_name);
        assert_eq!(qualified, heap.ht_qualname);
        assert_eq!(read_str(name), b"prefix.Name");
        assert_eq!(read_str(qualified), b"Outer.Qualified");
        refcount::Py_DECREF(name);
        refcount::Py_DECREF(qualified);
        refcount::Py_DECREF(heap.ht_name);
        let raw_name = [0xed, 0xa0, 0x80];
        let surrogate = 0xd800_u16;
        heap.ht_name = strings::PyUnicode_FromKindAndData(2, (&raw const surrogate).cast(), 1);
        let renamed = typeobj::PyType_GetName(tp);
        assert_eq!(read_str(renamed), raw_name);
        refcount::Py_DECREF(renamed);
        refcount::Py_DECREF(heap.ht_name);
        refcount::Py_DECREF(heap.ht_qualname);
        molt_cpython_abi::api::memory::PyObject_GC_Del((heap as *mut PyHeapTypeObject).cast());
    }
}
