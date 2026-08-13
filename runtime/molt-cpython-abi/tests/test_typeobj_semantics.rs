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
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::os::raw::c_int;
use std::ptr;
use std::sync::Mutex;

// ── Minimal native-string backend (for name/message round-trips) ─────────────
static STR_MAP: Mutex<Option<HashMap<u64, &'static [u8]>>> = Mutex::new(None);
fn str_map() -> std::sync::MutexGuard<'static, Option<HashMap<u64, &'static [u8]>>> {
    let mut g = STR_MAP.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}
unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes: Vec<u8> = if data.is_null() || len == 0 {
        Vec::from(&b"\0"[..])
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let handle = MoltObject::from_ptr(leaked.as_ptr() as *mut u8).bits();
    let view: &'static [u8] = if len == 0 { &leaked[..0] } else { leaked };
    str_map().as_mut().unwrap().insert(handle, view);
    handle
}
unsafe extern "C" fn fake_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    if let Some(&v) = str_map().as_ref().unwrap().get(&bits) {
        unsafe { *out_len = v.len() };
        return v.as_ptr();
    }
    unsafe { *out_len = 0 };
    ptr::null()
}
unsafe extern "C" fn fake_classify_heap(bits: u64) -> u8 {
    use molt_cpython_abi::abi_types::MoltTypeTag;
    if str_map().as_ref().unwrap().contains_key(&bits) {
        MoltTypeTag::Str as u8
    } else {
        MoltTypeTag::Other as u8
    }
}
unsafe extern "C" fn noop_ref(_: u64) {}
fn install() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_str = fake_alloc_str;
    hooks.str_data = fake_str_data;
    hooks.classify_heap = fake_classify_heap;
    hooks.inc_ref = noop_ref;
    hooks.dec_ref = noop_ref;
    support::prepare_abi_test_thread(hooks);
}
unsafe fn read_str(py: *mut PyObject) -> Vec<u8> {
    let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .pyobj_to_handle(py)
        .map(|identity| identity.as_handle())
        .expect("bridge str");
    str_map().as_ref().unwrap().get(&bits).unwrap().to_vec()
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
    // Foreign type with tp_hash == NULL — CPython raises "unhashable type".
    let mut ty = new_type();
    ty.tp_name = c"Unhashable".as_ptr();
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
