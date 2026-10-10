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
use std::os::raw::c_int;
use std::ptr;
use std::sync::Mutex;

// The shared fixture supplies real string/numeric payload and edge ownership.
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1234) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(py) },
        1234
    );
}

#[test]
fn hash_of_unhashable_foreign_raises_typeerror() {
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
const T_STRING: c_int = 5;
const T_STRING_INPLACE: c_int = 13;
const T_OBJECT: c_int = 6;
const T_OBJECT_EX: c_int = 16;

unsafe fn assert_member_error(exception: *mut PyObject, message: &str) {
    assert_eq!(unsafe { errors::PyErr_ExceptionMatches(exception) }, 1);
    assert_eq!(support::take_current_error_text().as_deref(), Some(message));
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

#[test]
fn member_relative_offset_is_rejected_before_other_admission() {
    let _abi_test = install();
    unsafe {
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|target| target.set(minor));
            // All addresses are in bounds even for an implementation missing
            // the guard. NONE and unknown types also prove type-independent admission.
            for ty in [T_INT, T_STRING_INPLACE, 20, 999] {
                for flags in [
                    member_abi::Py_RELATIVE_OFFSET,
                    member_abi::Py_RELATIVE_OFFSET | 1,
                ] {
                    let mut storage = [0u64; 4];
                    let before = storage;
                    let mut definition = member(ty, 8);
                    definition.flags = flags;
                    assert!(
                        typeobj::PyMember_GetOne(storage.as_ptr().cast(), &raw mut definition,)
                            .is_null()
                    );
                    assert_member_error(
                        (&raw mut member_abi::PyExc_SystemError).cast(),
                        "PyMember_GetOne used with Py_RELATIVE_OFFSET",
                    );
                    for value in [(&raw mut Py_True).cast::<PyObject>(), ptr::null_mut()] {
                        assert_eq!(
                            typeobj::PyMember_SetOne(
                                storage.as_mut_ptr().cast(),
                                &raw mut definition,
                                value,
                            ),
                            -1
                        );
                        assert_member_error(
                            (&raw mut member_abi::PyExc_SystemError).cast(),
                            "PyMember_SetOne used with Py_RELATIVE_OFFSET",
                        );
                        assert_eq!(storage, before);
                    }
                }
            }
            // Clearing the error leaves ordinary member access usable.
            let mut field: c_int = 41;
            let mut definition = member(T_INT, 0);
            assert_eq!(
                typeobj::PyMember_SetOne(
                    (&raw mut field).cast(),
                    &raw mut definition,
                    (&raw mut Py_True).cast()
                ),
                0
            );
            assert_eq!(field, 1);
        }
    }
}

#[test]
fn member_inline_string_reads_storage_and_preserves_readonly_precedence() {
    let _abi_test = install();
    unsafe {
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|target| target.set(minor));
            for payload in [b"\0".as_slice(), b"h\xc3\xa9\0ignored".as_slice()] {
                let mut storage = [0xa5u8; 24];
                storage[1..1 + payload.len()].copy_from_slice(payload);
                let before = storage;
                let mut definition = member(T_STRING_INPLACE, 1);
                let value = refcount::OwnedPyObject::from_owned(typeobj::PyMember_GetOne(
                    storage.as_ptr().cast(),
                    &raw mut definition,
                ));
                assert!(!value.as_ptr().is_null());
                let end = payload.iter().position(|byte| *byte == 0).unwrap();
                assert_eq!(read_str(value.as_ptr()), &payload[..end]);
                assert_eq!(storage, before);
                for flags in [0, 1] {
                    definition.flags = flags;
                    for replacement in [(&raw mut Py_True).cast::<PyObject>(), ptr::null_mut()] {
                        assert_eq!(
                            typeobj::PyMember_SetOne(
                                storage.as_mut_ptr().cast(),
                                &raw mut definition,
                                replacement,
                            ),
                            -1
                        );
                        let (exception, message) = if flags != 0 {
                            (
                                (&raw mut member_abi::PyExc_AttributeError).cast(),
                                "readonly attribute",
                            )
                        } else if replacement.is_null() {
                            (
                                (&raw mut member_abi::PyExc_TypeError).cast(),
                                "can't delete numeric/char attribute",
                            )
                        } else {
                            (
                                (&raw mut member_abi::PyExc_TypeError).cast(),
                                "readonly attribute",
                            )
                        };
                        assert_member_error(exception, message);
                        assert_eq!(storage, before);
                    }
                }
            }
            // STRING still dereferences a pointer; STRING_INPLACE reads the
            // bytes at the field. Both use the canonical UTF-8 string constructor.
            let indirect = c"indirect".as_ptr();
            let mut definition = member(T_STRING, 0);
            let result = refcount::OwnedPyObject::from_owned(typeobj::PyMember_GetOne(
                (&raw const indirect).cast(),
                &raw mut definition,
            ));
            assert!(!result.as_ptr().is_null());
            assert_eq!(read_str(result.as_ptr()), b"indirect");
            let invalid = [0xffu8, 0];
            definition.type_ = T_STRING_INPLACE;
            assert!(
                typeobj::PyMember_GetOne(invalid.as_ptr().cast(), &raw mut definition).is_null()
            );
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut member_abi::PyExc_UnicodeDecodeError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
        }
    }
}

#[test]
fn member_missing_object_and_unknown_type_use_pinned_diagnostics() {
    let _abi_test = install();
    #[repr(C)]
    struct Record {
        base: PyObject,
        value: *mut PyObject,
    }
    let name = std::ffi::CString::new(format!("pkg.{}", "Q".repeat(205))).unwrap();
    let mut class = new_type();
    class.ob_base.ob_base = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut member_abi::PyType_Type,
    };
    class.tp_base = &raw mut member_abi::PyBaseObject_Type;
    class.tp_name = name.as_ptr();
    let mut record = Record {
        base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *class,
        },
        value: ptr::null_mut(),
    };
    let mut definition = member(T_OBJECT_EX, std::mem::offset_of!(Record, value) as isize);
    unsafe {
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|target| target.set(minor));
            assert!(
                typeobj::PyMember_GetOne((&raw const record).cast(), &raw mut definition,)
                    .is_null()
            );
            let type_name = if minor == 12 {
                &name.to_bytes()[..200]
            } else {
                name.to_bytes()
            };
            assert_member_error(
                (&raw mut member_abi::PyExc_AttributeError).cast(),
                &format!(
                    "'{}' object has no attribute 'field'",
                    std::str::from_utf8(type_name).unwrap()
                ),
            );
            assert_eq!(
                typeobj::PyMember_SetOne(
                    (&raw mut record).cast(),
                    &raw mut definition,
                    ptr::null_mut(),
                ),
                -1
            );
            assert_member_error((&raw mut member_abi::PyExc_AttributeError).cast(), "field");
            definition.type_ = T_OBJECT;
            let none = refcount::OwnedPyObject::from_owned(typeobj::PyMember_GetOne(
                (&raw const record).cast(),
                &raw mut definition,
            ));
            assert_eq!(none.as_ptr(), &raw mut member_abi::Py_None);
            assert!(errors::PyErr_Occurred().is_null());
            definition.type_ = 999;
            assert!(
                typeobj::PyMember_GetOne((&raw const record).cast(), &raw mut definition,)
                    .is_null()
            );
            assert_member_error(
                (&raw mut member_abi::PyExc_SystemError).cast(),
                "bad memberdescr type",
            );
            assert_eq!(
                typeobj::PyMember_SetOne(
                    (&raw mut record).cast(),
                    &raw mut definition,
                    (&raw mut Py_True).cast(),
                ),
                -1
            );
            assert_member_error(
                (&raw mut member_abi::PyExc_SystemError).cast(),
                "bad memberdescr type for field",
            );
            assert!(record.value.is_null());
            assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
            definition.type_ = T_OBJECT_EX;
        }
    }
}

thread_local! {
    static MEMBER_TARGET: Cell<i64> = const { Cell::new(12) };
    static MEMBER_INDEX_CALLS: Cell<usize> = const { Cell::new(0) };
    static MEMBER_RESULT: Cell<usize> = const { Cell::new(0) };
    static MEMBER_ERROR: Cell<usize> = const { Cell::new(0) };
    static MEMBER_TRANSFER_RESULT: Cell<bool> = const { Cell::new(false) };
    static MEMBER_STORAGE: Cell<usize> = const { Cell::new(0) };
    static MEMBER_WARNING_BYTES: RefCell<Vec<[u8; 24]>> = const { RefCell::new(Vec::new()) };
    static MEMBER_WARNING_FINALIZERS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static MEMBER_FINALIZERS: Cell<usize> = const { Cell::new(0) };
    static MEMBER_FINALIZER_BYTES: Cell<[u8; 24]> = const { Cell::new([0; 24]) };
    static MEMBER_FINALIZER_PENDING: Cell<(usize, usize)> = const { Cell::new((usize::MAX, usize::MAX)) };
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
    let storage = MEMBER_STORAGE.with(Cell::get) as *const [u8; 24];
    if !storage.is_null() {
        MEMBER_WARNING_BYTES.with(|observations| {
            observations.borrow_mut().push(unsafe { *storage });
        });
    }
    MEMBER_WARNING_FINALIZERS.with(|observations| {
        observations
            .borrow_mut()
            .push(MEMBER_FINALIZERS.with(Cell::get));
    });
}

// Declare after a stack-backed traceback so a failing assertion clears the
// pending exception before that traceback's fixture storage leaves scope.
struct PendingErrorReset;

impl Drop for PendingErrorReset {
    fn drop(&mut self) {
        unsafe { errors::PyErr_Clear() };
    }
}

/// A conforming native deallocator may inspect the active error, but must
/// restore the same exception owner without normalizing or replacing it.
unsafe fn retirement_pending_identity() -> (usize, usize) {
    let error = unsafe { errors::PyErr_GetRaisedException() };
    let traceback = if error.is_null() {
        ptr::null_mut()
    } else {
        unsafe { errors::PyException_GetTraceback(error) }
    };
    let identity = (error as usize, traceback as usize);
    unsafe {
        refcount::Py_XDECREF(traceback);
        errors::PyErr_SetRaisedException(error);
    }
    identity
}

unsafe extern "C" fn member_result_finalizer(_object: *mut PyObject) {
    MEMBER_FINALIZERS.with(|count| count.set(count.get() + 1));
    let storage = MEMBER_STORAGE.with(Cell::get) as *const [u8; 24];
    if !storage.is_null() {
        MEMBER_FINALIZER_BYTES.with(|bytes| bytes.set(unsafe { *storage }));
    }
    MEMBER_FINALIZER_PENDING.with(|value| value.set(unsafe { retirement_pending_identity() }));
}

fn install() -> support::AbiTestThreadStateTransaction {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    // Structured UnicodeDecodeError owns the offending bytes as well as text.
    hooks.alloc_bytes = support::fake_runtime::alloc_bytes;
    hooks.bytes_data = support::fake_runtime::bytes_data;
    hooks.target_python_minor = member_target_minor;
    hooks.import_module = support::warnings::import_module;
    let transaction = support::enter_runtime_class_abi_test(hooks);
    MEMBER_TARGET.with(|value| value.set(12));
    MEMBER_RESULT.with(|value| value.set(0));
    MEMBER_ERROR.with(|value| value.set(0));
    MEMBER_TRANSFER_RESULT.with(|value| value.set(false));
    MEMBER_FINALIZERS.with(|value| value.set(0));
    MEMBER_FINALIZER_PENDING.with(|value| value.set((usize::MAX, usize::MAX)));
    transaction
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
    MEMBER_WARNING_FINALIZERS.with(|value| value.borrow_mut().clear());
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
        let mut traceback = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut member_abi::PyTraceBack_Type,
        };
        let _pending_reset = PendingErrorReset;
        errors::PyErr_SetString(
            (&raw mut member_abi::PyExc_DeprecationWarning).cast(),
            c"__index__ returned non-int (type MemberIndexResult).  The ability to return an instance of a strict subclass of int is deprecated, and may be removed in a future version of Python.".as_ptr(),
        );
        let warning = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!warning.as_ptr().is_null());
        assert_eq!(
            errors::PyException_SetTraceback(warning.as_ptr(), &raw mut traceback),
            0
        );
        support::warnings::with_error_instance(warning.as_ptr(), || {
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
                    MEMBER_FINALIZER_PENDING.with(|value| value.set((usize::MAX, usize::MAX)));
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
                        let raised =
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(raised.as_ptr(), warning.as_ptr());
                        assert_eq!(
                            MEMBER_FINALIZER_PENDING.with(Cell::get),
                            (warning.as_ptr() as usize, (&raw mut traceback) as usize)
                        );
                        let observed_traceback = refcount::OwnedPyObject::from_owned(
                            errors::PyException_GetTraceback(raised.as_ptr()),
                        );
                        assert_eq!(observed_traceback.as_ptr(), &raw mut traceback);
                        errors::PyErr_SetRaisedException(raised.into_ptr());
                        assert_eq!(
                            support::take_current_error_text().as_deref(),
                            Some(warnings[0].message.as_str())
                        );
                        MEMBER_WARNING_BYTES
                            .with(|value| assert_eq!(*value.borrow(), vec![[0xa5; 24]]));
                        MEMBER_WARNING_FINALIZERS
                            .with(|value| assert_eq!(*value.borrow(), vec![0]));
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
                        MEMBER_WARNING_FINALIZERS
                            .with(|value| assert_eq!(*value.borrow(), vec![0, 1]));
                        assert_eq!(MEMBER_FINALIZER_PENDING.with(Cell::get), (0, 0));
                        assert!(errors::PyErr_Occurred().is_null());
                    }
                    assert_eq!(
                        result.ob_base.ob_refcnt, 0,
                        "the protocol transfers its sole result owner"
                    );
                }
            }
        });
        drop(warning);
        assert_eq!(traceback.ob_refcnt, 1);

        // A genuine subtype result with magnitude 2**120 overflows every C
        // carrier used by these branches, on both LP64 and LLP64 targets.
        #[repr(C)]
        struct WideInteger {
            ob_base: PyObject,
            lv_tag: usize,
            digits: [u32; 5],
        }
        support::warnings::set_as_error(false);
        for minor in [12, 13, 14] {
            MEMBER_TARGET.with(|value| value.set(minor));
            for ty in [T_UINT, T_ULONG, T_ULONGLONG] {
                for negative in [false, true] {
                    let mut result = WideInteger {
                        ob_base: PyObject {
                            ob_refcnt: 1,
                            ob_type: subtype.as_ptr(),
                        },
                        lv_tag: (5 << 3) | if negative { 2 } else { 0 },
                        digits: [0, 0, 0, 0, 1],
                    };
                    MEMBER_RESULT.with(|value| value.set((&raw mut result) as usize));
                    MEMBER_TRANSFER_RESULT.with(|value| value.set(true));
                    MEMBER_FINALIZERS.with(|value| value.set(0));
                    MEMBER_FINALIZER_PENDING.with(|value| value.set((usize::MAX, usize::MAX)));
                    support::warnings::clear();
                    let offset = usize::from(ty == T_ULONGLONG);
                    let (status, bytes) = write_member(ty, &raw mut input, offset);
                    assert_eq!(status, -1);
                    assert_eq!(bytes, [0xa5; 24]);
                    assert_eq!(MEMBER_INDEX_CALLS.with(Cell::get), 1);
                    assert_eq!(MEMBER_FINALIZERS.with(Cell::get), 1);
                    assert_eq!(MEMBER_FINALIZER_BYTES.with(Cell::get), [0xa5; 24]);
                    let raised =
                        refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(!raised.as_ptr().is_null());
                    assert_eq!(
                        object::Py_TYPE(raised.as_ptr()),
                        &raw mut member_abi::PyExc_OverflowError
                    );
                    assert_eq!(
                        MEMBER_FINALIZER_PENDING.with(Cell::get),
                        (raised.as_ptr() as usize, 0)
                    );
                    assert_eq!(support::warnings::emissions().len(), 1);
                    MEMBER_WARNING_BYTES
                        .with(|value| assert_eq!(*value.borrow(), vec![[0xa5; 24]]));
                    MEMBER_WARNING_FINALIZERS.with(|value| assert_eq!(*value.borrow(), vec![0]));
                    assert_eq!(result.ob_base.ob_refcnt, 0);
                    assert!(errors::PyErr_Occurred().is_null());
                }
            }
        }
        MEMBER_RESULT.with(|value| value.set(0));
        MEMBER_TRANSFER_RESULT.with(|value| value.set(false));
        assert_eq!(input.ob_refcnt, 1);
    });
}

thread_local! {
    static SEMANTIC_RESULT: Cell<usize> = const { Cell::new(0) };
    static SEMANTIC_BOOL_ERROR: Cell<usize> = const { Cell::new(0) };
    static SEMANTIC_BOOL_VALUE: Cell<c_int> = const { Cell::new(0) };
    static SEMANTIC_BOOL_CALLS: Cell<usize> = const { Cell::new(0) };
    static SEMANTIC_RETIREMENTS: Cell<usize> = const { Cell::new(0) };
    static SEMANTIC_PENDING: Cell<(usize, usize)> = const { Cell::new((usize::MAX, usize::MAX)) };
}

unsafe extern "C" fn semantic_result(_object: *mut PyObject) -> *mut PyObject {
    SEMANTIC_RESULT.with(|value| value.replace(0)) as *mut PyObject
}

unsafe extern "C" fn semantic_compare_result(
    object: *mut PyObject,
    _other: *mut PyObject,
    _op: c_int,
) -> *mut PyObject {
    unsafe { semantic_result(object) }
}

unsafe extern "C" fn semantic_result_bool(_object: *mut PyObject) -> c_int {
    SEMANTIC_BOOL_CALLS.with(|value| value.set(value.get() + 1));
    let error = SEMANTIC_BOOL_ERROR.with(Cell::get) as *mut PyObject;
    if !error.is_null() {
        unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(error)) };
        -1
    } else {
        SEMANTIC_BOOL_VALUE.with(Cell::get)
    }
}

unsafe extern "C" fn semantic_result_finalizer(_object: *mut PyObject) {
    SEMANTIC_RETIREMENTS.with(|value| value.set(value.get() + 1));
    SEMANTIC_PENDING.with(|value| value.set(unsafe { retirement_pending_identity() }));
}

#[test]
fn stringifier_and_richcompare_results_retire_with_the_exact_active_error() {
    let _abi_test = install();
    unsafe {
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        class.tp_name = c"SemanticResultProducer".as_ptr();
        class.tp_str = Some(semantic_result);
        class.tp_repr = Some(semantic_result);
        class.tp_richcompare = Some(semantic_compare_result);
        let mut input = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        let mut other = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        let mut result_slots: member_abi::PyNumberMethods = std::mem::zeroed();
        result_slots.nb_bool = semantic_result_bool as *const () as *mut c_void;
        let mut result_class = support::StaticType::new();
        result_class.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        result_class.tp_name = c"SemanticResult".as_ptr();
        result_class.tp_as_number = (&raw mut result_slots).cast();
        result_class.tp_dealloc = Some(semantic_result_finalizer);
        for (stringify, message) in [
            (
                typeobj::PyObject_Str as unsafe extern "C" fn(*mut PyObject) -> *mut PyObject,
                "__str__ returned non-string (type SemanticResult)",
            ),
            (
                typeobj::PyObject_Repr as unsafe extern "C" fn(*mut PyObject) -> *mut PyObject,
                "__repr__ returned non-string (type SemanticResult)",
            ),
        ] {
            let mut result = PyObject {
                ob_refcnt: 1,
                ob_type: result_class.as_ptr(),
            };
            SEMANTIC_RESULT.with(|value| value.set((&raw mut result) as usize));
            SEMANTIC_RETIREMENTS.with(|value| value.set(0));
            SEMANTIC_PENDING.with(|value| value.set((usize::MAX, usize::MAX)));
            assert!(stringify(&raw mut input).is_null());
            let raised = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert!(!raised.as_ptr().is_null());
            assert_eq!(
                object::Py_TYPE(raised.as_ptr()),
                &raw mut member_abi::PyExc_TypeError
            );
            assert_eq!(SEMANTIC_RETIREMENTS.with(Cell::get), 1);
            assert_eq!(
                SEMANTIC_PENDING.with(Cell::get),
                (raised.as_ptr() as usize, 0)
            );
            assert_eq!(result.ob_refcnt, 0);
            errors::PyErr_SetRaisedException(raised.into_ptr());
            assert_eq!(support::take_current_error_text().as_deref(), Some(message));
        }
        let mut traceback = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut member_abi::PyTraceBack_Type,
        };
        let _pending_reset = PendingErrorReset;
        errors::PyErr_SetString(
            (&raw mut member_abi::PyExc_ValueError).cast(),
            c"truth conversion failed".as_ptr(),
        );
        let failure = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!failure.as_ptr().is_null());
        assert_eq!(
            errors::PyException_SetTraceback(failure.as_ptr(), &raw mut traceback),
            0
        );
        for truth in [-1, 0, 1] {
            let mut result = PyObject {
                ob_refcnt: 1,
                ob_type: result_class.as_ptr(),
            };
            SEMANTIC_RESULT.with(|value| value.set((&raw mut result) as usize));
            SEMANTIC_BOOL_ERROR.with(|value| {
                value.set(if truth < 0 {
                    failure.as_ptr() as usize
                } else {
                    0
                })
            });
            SEMANTIC_BOOL_VALUE.with(|value| value.set(truth));
            SEMANTIC_BOOL_CALLS.with(|value| value.set(0));
            SEMANTIC_RETIREMENTS.with(|value| value.set(0));
            SEMANTIC_PENDING.with(|value| value.set((usize::MAX, usize::MAX)));
            assert_eq!(
                typeobj::PyObject_RichCompareBool(&raw mut input, &raw mut other, PY_EQ),
                truth
            );
            assert_eq!(SEMANTIC_BOOL_CALLS.with(Cell::get), 1);
            assert_eq!(SEMANTIC_RETIREMENTS.with(Cell::get), 1);
            assert_eq!(result.ob_refcnt, 0);
            if truth < 0 {
                let raised =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(raised.as_ptr(), failure.as_ptr());
                assert_eq!(
                    SEMANTIC_PENDING.with(Cell::get),
                    (failure.as_ptr() as usize, (&raw mut traceback) as usize)
                );
                let observed_traceback = refcount::OwnedPyObject::from_owned(
                    errors::PyException_GetTraceback(raised.as_ptr()),
                );
                assert_eq!(observed_traceback.as_ptr(), &raw mut traceback);
            } else {
                assert_eq!(SEMANTIC_PENDING.with(Cell::get), (0, 0));
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
        SEMANTIC_BOOL_ERROR.with(|value| value.set(0));
        drop(failure);
        assert_eq!(traceback.ob_refcnt, 1);
        assert_eq!(input.ob_refcnt, 1);
        assert_eq!(other.ob_refcnt, 1);
    }
}

thread_local! {
    static TYPE_ATTRIBUTE_META: Cell<usize> = const { Cell::new(0) };
    static TYPE_ATTRIBUTE_DICT: Cell<usize> = const { Cell::new(0) };
    static TYPE_ATTRIBUTE_FAILURE: Cell<usize> = const { Cell::new(0) };
    static TYPE_ATTRIBUTE_OBSERVED_REFS: Cell<isize> = const { Cell::new(-1) };
    static TYPE_ATTRIBUTE_GET_CALLS: Cell<usize> = const { Cell::new(0) };
    static TYPE_ATTRIBUTE_DROPS: Cell<usize> = const { Cell::new(0) };
    static TYPE_ATTRIBUTE_PENDING: Cell<(usize, usize)> = const { Cell::new((usize::MAX, usize::MAX)) };
}

unsafe extern "C" fn local_attribute_get(
    _descriptor: *mut PyObject,
    _instance: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    TYPE_ATTRIBUTE_GET_CALLS.with(|value| value.set(value.get() + 1));
    let meta = TYPE_ATTRIBUTE_META.with(Cell::get) as *mut PyObject;
    if !meta.is_null() {
        TYPE_ATTRIBUTE_OBSERVED_REFS.with(|value| value.set(unsafe { (*meta).ob_refcnt }));
    }
    unsafe { object::Py_NewRef(&raw mut member_abi::Py_None) }
}

unsafe extern "C" fn removing_meta_attribute_get(
    _descriptor: *mut PyObject,
    _instance: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    TYPE_ATTRIBUTE_GET_CALLS.with(|value| value.set(value.get() + 1));
    let dict = TYPE_ATTRIBUTE_DICT.with(Cell::get) as *mut PyObject;
    if unsafe { molt_cpython_abi::api::mapping::PyDict_DelItemString(dict, c"field".as_ptr()) } < 0
    {
        return ptr::null_mut();
    }
    let error = TYPE_ATTRIBUTE_FAILURE.with(Cell::get) as *mut PyObject;
    unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(error)) };
    ptr::null_mut()
}

unsafe extern "C" fn meta_attribute_finalizer(object: *mut PyObject) {
    TYPE_ATTRIBUTE_DROPS.with(|value| value.set(value.get() + 1));
    TYPE_ATTRIBUTE_PENDING.with(|value| value.set(unsafe { retirement_pending_identity() }));
    unsafe { drop(Box::from_raw(object)) };
}

unsafe extern "C" fn local_attribute_finalizer(object: *mut PyObject) {
    unsafe { drop(Box::from_raw(object)) };
}

#[test]
fn type_attribute_descriptors_retire_at_their_public_lookup_boundaries() {
    let _abi_test = install();
    unsafe {
        use molt_cpython_abi::api::mapping;
        let mut meta_descriptor_type = support::StaticType::new();
        meta_descriptor_type.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        meta_descriptor_type.tp_name = c"MetaAttributeDescriptor".as_ptr();
        meta_descriptor_type.tp_descr_get = Some(removing_meta_attribute_get);
        meta_descriptor_type.tp_dealloc = Some(meta_attribute_finalizer);
        let mut local_descriptor_type = support::StaticType::new();
        local_descriptor_type.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        local_descriptor_type.tp_name = c"LocalAttributeDescriptor".as_ptr();
        local_descriptor_type.tp_descr_get = Some(local_attribute_get);
        local_descriptor_type.tp_dealloc = Some(local_attribute_finalizer);
        let mut metaclass = support::StaticType::new();
        metaclass.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        metaclass.tp_base = &raw mut member_abi::PyType_Type;
        metaclass.tp_name = c"AttributeMetaclass".as_ptr();
        metaclass.tp_flags = member_abi::Py_TPFLAGS_READY;
        metaclass.tp_getattro = member_abi::PyType_Type.tp_getattro;
        assert!(metaclass.tp_getattro.is_some());
        metaclass.tp_dict = mapping::PyDict_New();
        assert!(!metaclass.tp_dict.is_null());
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = metaclass.as_ptr();
        class.tp_base = &raw mut member_abi::PyBaseObject_Type;
        class.tp_name = c"AttributeOwner".as_ptr();
        class.tp_flags = member_abi::Py_TPFLAGS_READY;
        class.tp_dict = mapping::PyDict_New();
        assert!(!class.tp_dict.is_null());
        let meta_descriptor =
            refcount::OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
                ob_refcnt: 1,
                ob_type: meta_descriptor_type.as_ptr(),
            })));
        let local_descriptor =
            refcount::OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
                ob_refcnt: 1,
                ob_type: local_descriptor_type.as_ptr(),
            })));
        assert_eq!(
            mapping::PyDict_SetItemString(
                metaclass.tp_dict,
                c"field".as_ptr(),
                meta_descriptor.as_ptr()
            ),
            0
        );
        assert_eq!(
            mapping::PyDict_SetItemString(
                class.tp_dict,
                c"field".as_ptr(),
                local_descriptor.as_ptr()
            ),
            0
        );
        let mut traceback = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut member_abi::PyTraceBack_Type,
        };
        let _pending_reset = PendingErrorReset;
        errors::PyErr_SetString(
            (&raw mut member_abi::PyExc_LookupError).cast(),
            c"metatype descriptor failed".as_ptr(),
        );
        let failure = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!failure.as_ptr().is_null());
        assert_eq!(
            errors::PyException_SetTraceback(failure.as_ptr(), &raw mut traceback),
            0
        );
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                TYPE_ATTRIBUTE_META.with(|value| value.set(0));
                TYPE_ATTRIBUTE_DICT.with(|value| value.set(0));
                TYPE_ATTRIBUTE_FAILURE.with(|value| value.set(0));
            }
        }
        let _reset = Reset;
        TYPE_ATTRIBUTE_META.with(|value| value.set(meta_descriptor.as_ptr() as usize));
        TYPE_ATTRIBUTE_DICT.with(|value| value.set(metaclass.tp_dict as usize));
        TYPE_ATTRIBUTE_FAILURE.with(|value| value.set(failure.as_ptr() as usize));
        TYPE_ATTRIBUTE_GET_CALLS.with(|value| value.set(0));
        TYPE_ATTRIBUTE_OBSERVED_REFS.with(|value| value.set(-1));
        TYPE_ATTRIBUTE_DROPS.with(|value| value.set(0));
        let baseline = (*meta_descriptor.as_ptr()).ob_refcnt;
        let found = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            class.as_ptr().cast(),
            c"field".as_ptr(),
        ));
        assert_eq!(found.as_ptr(), &raw mut member_abi::Py_None);
        assert_eq!(TYPE_ATTRIBUTE_GET_CALLS.with(Cell::get), 1);
        assert_eq!(
            TYPE_ATTRIBUTE_OBSERVED_REFS.with(Cell::get),
            baseline,
            "the unused metatype snapshot must retire before the local descriptor is invoked"
        );
        assert_eq!((*meta_descriptor.as_ptr()).ob_refcnt, baseline);
        assert_eq!(TYPE_ATTRIBUTE_DROPS.with(Cell::get), 0);
        assert!(errors::PyErr_Occurred().is_null());
        drop(found);

        // With the local declaration removed, the metatype descriptor runs.
        // It removes its own published edge; the lookup's owner is then sole.
        assert_eq!(
            mapping::PyDict_DelItemString(class.tp_dict, c"field".as_ptr()),
            0
        );
        drop(meta_descriptor);
        TYPE_ATTRIBUTE_GET_CALLS.with(|value| value.set(0));
        TYPE_ATTRIBUTE_PENDING.with(|value| value.set((usize::MAX, usize::MAX)));
        assert!(object::PyObject_GetAttrString(class.as_ptr().cast(), c"field".as_ptr()).is_null());
        let raised = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert_eq!(raised.as_ptr(), failure.as_ptr());
        assert_eq!(TYPE_ATTRIBUTE_GET_CALLS.with(Cell::get), 1);
        assert_eq!(TYPE_ATTRIBUTE_DROPS.with(Cell::get), 1);
        assert_eq!(
            TYPE_ATTRIBUTE_PENDING.with(Cell::get),
            (failure.as_ptr() as usize, (&raw mut traceback) as usize)
        );
        let observed_traceback =
            refcount::OwnedPyObject::from_owned(errors::PyException_GetTraceback(raised.as_ptr()));
        assert_eq!(observed_traceback.as_ptr(), &raw mut traceback);
        drop(observed_traceback);
        drop(raised);
        drop(failure);
        assert_eq!(traceback.ob_refcnt, 1);
        assert!(errors::PyErr_Occurred().is_null());
    }
}

thread_local! {
    static REPLACEMENT_METACLASS: Cell<usize> = const { Cell::new(0) };
    static METACLASS_FAILURE: Cell<usize> = const { Cell::new(0) };
    static METACLASS_REPLACEMENTS: Cell<usize> = const { Cell::new(0) };
    static METACLASS_RETIREMENTS: Cell<usize> = const { Cell::new(0) };
    static METACLASS_PENDING: Cell<(usize, usize)> = const { Cell::new((usize::MAX, usize::MAX)) };
}

unsafe extern "C" fn replacing_metaclass_attribute_get(
    _descriptor: *mut PyObject,
    instance: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    let replacement = REPLACEMENT_METACLASS.with(Cell::get) as *mut PyObject;
    // The root __class__ descriptor's real native storage delegate admits the
    // same mutable, compatible metaclass transition as ordinary instances.
    if unsafe {
        typeobj::native_type_attribute_set(
            instance,
            typeobj::TypeAttributeField::Class,
            replacement,
            false,
        )
    } < 0
    {
        return ptr::null_mut();
    }
    METACLASS_REPLACEMENTS.with(|value| value.set(value.get() + 1));
    let error = METACLASS_FAILURE.with(Cell::get) as *mut PyObject;
    unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(error)) };
    ptr::null_mut()
}

unsafe extern "C" fn retired_metaclass(_metaclass: *mut PyObject) {
    // StaticType owns the physical test allocation. This real tp_dealloc hook
    // observes its sole C reference reaching zero without freeing that storage.
    METACLASS_RETIREMENTS.with(|value| value.set(value.get() + 1));
    METACLASS_PENDING.with(|value| value.set(unsafe { retirement_pending_identity() }));
}

#[test]
fn type_lookup_retires_a_replaced_metaclass_with_the_exact_active_error() {
    let _abi_test = install();
    unsafe {
        use molt_cpython_abi::api::mapping;
        let mut metatype = support::StaticType::new();
        metatype.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        metatype.tp_base = &raw mut member_abi::PyType_Type;
        metatype.tp_name = c"RetirementMetatype".as_ptr();
        metatype.tp_dealloc = Some(retired_metaclass);
        let mut descriptor_type = support::StaticType::new();
        descriptor_type.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        descriptor_type.tp_name = c"ReplacingMetaclassDescriptor".as_ptr();
        descriptor_type.tp_descr_get = Some(replacing_metaclass_attribute_get);
        descriptor_type.tp_dealloc = Some(local_attribute_finalizer);
        let mut original = support::StaticType::new();
        let mut replacement = support::StaticType::new();
        for meta in [&mut original, &mut replacement] {
            meta.ob_base.ob_base.ob_type = metatype.as_ptr();
            meta.tp_base = &raw mut member_abi::PyType_Type;
            meta.tp_name = c"ReplaceableMetaclass".as_ptr();
            meta.tp_flags = member_abi::Py_TPFLAGS_READY | member_abi::Py_TPFLAGS_HEAPTYPE;
            meta.tp_basicsize = member_abi::PyType_Type.tp_basicsize;
            meta.tp_itemsize = member_abi::PyType_Type.tp_itemsize;
            meta.tp_getattro = member_abi::PyType_Type.tp_getattro;
        }
        original.tp_dict = mapping::PyDict_New();
        assert!(!original.tp_dict.is_null());
        let mut class = support::StaticType::new();
        // Original's initial reference is the class's metaclass edge, not an
        // extra test owner. Replacing the edge leaves only the lookup pin.
        class.ob_base.ob_base.ob_type = original.as_ptr();
        class.tp_base = &raw mut member_abi::PyBaseObject_Type;
        class.tp_name = c"MetaclassReplacementOwner".as_ptr();
        class.tp_flags = member_abi::Py_TPFLAGS_READY;
        let descriptor = refcount::OwnedPyObject::from_owned(Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: descriptor_type.as_ptr(),
        })));
        assert_eq!(
            mapping::PyDict_SetItemString(original.tp_dict, c"field".as_ptr(), descriptor.as_ptr()),
            0
        );
        let name = refcount::OwnedPyObject::from_owned(
            molt_cpython_abi::api::strings::PyUnicode_FromString(c"field".as_ptr()),
        );
        assert!(!name.as_ptr().is_null());
        let mut traceback = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut member_abi::PyTraceBack_Type,
        };
        let _pending_reset = PendingErrorReset;
        errors::PyErr_SetString(
            (&raw mut member_abi::PyExc_LookupError).cast(),
            c"metaclass replacement failed".as_ptr(),
        );
        let failure = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!failure.as_ptr().is_null());
        assert_eq!(
            errors::PyException_SetTraceback(failure.as_ptr(), &raw mut traceback),
            0
        );
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                REPLACEMENT_METACLASS.with(|value| value.set(0));
                METACLASS_FAILURE.with(|value| value.set(0));
            }
        }
        let _reset = Reset;
        REPLACEMENT_METACLASS.with(|value| value.set(replacement.as_ptr() as usize));
        METACLASS_FAILURE.with(|value| value.set(failure.as_ptr() as usize));
        METACLASS_REPLACEMENTS.with(|value| value.set(0));
        METACLASS_RETIREMENTS.with(|value| value.set(0));
        METACLASS_PENDING.with(|value| value.set((usize::MAX, usize::MAX)));
        let replacement_refs = replacement.ob_base.ob_base.ob_refcnt;
        // Enter the public native slot itself. PyObject_GetAttr's outer type
        // pin would otherwise mask whether this inner pin uses ordinary Drop.
        let get = original.tp_getattro.expect("type getter installed");
        assert!(get(class.as_ptr().cast(), name.as_ptr()).is_null());
        let raised = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert_eq!(METACLASS_REPLACEMENTS.with(Cell::get), 1);
        assert_eq!(class.ob_base.ob_base.ob_type, replacement.as_ptr());
        assert_eq!(original.ob_base.ob_base.ob_refcnt, 0);
        assert_eq!(METACLASS_RETIREMENTS.with(Cell::get), 1);
        assert_eq!(raised.as_ptr(), failure.as_ptr());
        assert_eq!(
            METACLASS_PENDING.with(Cell::get),
            (failure.as_ptr() as usize, (&raw mut traceback) as usize)
        );
        let observed_traceback =
            refcount::OwnedPyObject::from_owned(errors::PyException_GetTraceback(raised.as_ptr()));
        assert_eq!(observed_traceback.as_ptr(), &raw mut traceback);
        assert_eq!(replacement.ob_base.ob_base.ob_refcnt, replacement_refs + 1);
        class.ob_base.ob_base.ob_type = &raw mut member_abi::PyType_Type;
        refcount::Py_DECREF(replacement.as_ptr().cast());
        drop(observed_traceback);
        drop(raised);
        drop(failure);
        assert_eq!(traceback.ob_refcnt, 1);
        assert!(errors::PyErr_Occurred().is_null());
    }
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
    let _abi_test = install();
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
        let surrogate = 0xd800_u16;
        heap.ht_name = strings::PyUnicode_FromKindAndData(2, (&raw const surrogate).cast(), 1);
        let renamed = typeobj::PyType_GetName(tp);
        assert_eq!(renamed, heap.ht_name);
        assert_eq!(strings::PyUnicode_GetLength(renamed), 1);
        assert_eq!(
            strings::PyUnicode_ReadChar(renamed, 0),
            u32::from(surrogate)
        );
        refcount::Py_DECREF(renamed);
        // Release the allocated heap type and all owned roots through its
        // clear/deallocation slots, not raw storage.
        let object = (heap as *mut PyHeapTypeObject).cast();
        assert_eq!(typeobj::molt_type_clear(object), 0);
        refcount::Py_DECREF(object);
    }
}
