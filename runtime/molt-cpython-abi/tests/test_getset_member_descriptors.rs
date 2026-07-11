//! `PyType_Ready` getset/member descriptor closure â€” the fast (`cargo test -p
//! molt-lang-cpython-abi`, no wasm) reproduction of the frontier numpy's
//! `_multiarray_umath_exec` hits after the scalar-type readiness gap was closed.
//!
//! numpy's `PyUFunc_Type`, `PyArrayDescr_Type`, and `PyArray_Type` â€” all readied
//! inside `_multiarray_umath_exec` â€” declare static `tp_members` and `tp_getset`
//! tables (e.g. `arraydescr_members`: `{"type", T_OBJECT, ...}`, `{"kind",
//! T_CHAR, ...}`, `{"num", T_INT, ...}`, `{"itemsize", T_PYSSIZET, ...}`, and
//! `arraydescr_getsets`: `{"names", getter, setter, ...}`). CPython's
//! `PyType_Ready` runs `type_add_members` + `type_add_getset`, turning each entry
//! into a real `member_descriptor` / `getset_descriptor` in `tp_dict` (built via
//! `PyDescr_NewMember` / `PyDescr_NewGetSet`). A runtime that instead stubs
//! `PyDescr_New*` to return `Py_None` and never populates the tables leaves the
//! numpy types without their documented attributes and diverges from CPython.
//!
//! These tests assert the end state:
//!   1. `PyType_Ready` on a type with `tp_members`/`tp_getset` succeeds and
//!      populates `tp_dict` with real descriptors (not `Py_None`).
//!   2. `PyDescr_NewGetSet`/`PyDescr_NewMember` mint real descriptor objects of
//!      the correct type, carrying the interned name and the borrowed def
//!      pointer, resolvable via `_PyType_Lookup`.
//!   3. The descriptor protocol works: `member_descriptor.__get__` reads the
//!      struct member at its offset; `getset_descriptor.__get__` invokes the
//!      underlying getter; read-only writes fail closed with `AttributeError`.
//!   4. On the failure path (`PyDescr_New*` given a NULL def) the primitive
//!      returns NULL and records a silent-failure so an exec-slot -1 is never
//!      contentless.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::RuntimeHooks;
use std::collections::HashMap;
use std::os::raw::{c_char, c_int, c_long};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

unsafe fn ready(tp: *mut PyTypeObject) -> c_int {
    unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(tp) }
}

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x5200_0000);
static DICTS: Mutex<Option<HashMap<u64, HashMap<u64, u64>>>> = Mutex::new(None);
static STRINGS: Mutex<Option<HashMap<Vec<u8>, u64>>> = Mutex::new(None);

fn fresh_handle() -> u64 {
    NEXT_HANDLE.fetch_add(0x10, Ordering::Relaxed)
}

fn dicts() -> std::sync::MutexGuard<'static, Option<HashMap<u64, HashMap<u64, u64>>>> {
    let mut guard = DICTS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

unsafe extern "C" fn fake_alloc_dict() -> u64 {
    let handle = fresh_handle();
    dicts().as_mut().unwrap().insert(handle, HashMap::new());
    handle
}

unsafe extern "C" fn fake_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) {
    if let Some(map) = dicts().as_mut().unwrap().get_mut(&dict_bits) {
        map.insert(key_bits, val_bits);
    }
}

unsafe extern "C" fn fake_dict_get(dict_bits: u64, key_bits: u64) -> u64 {
    dicts()
        .as_ref()
        .unwrap()
        .get(&dict_bits)
        .and_then(|map| map.get(&key_bits).copied())
        .unwrap_or(0)
}

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes = if data.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    let mut guard = STRINGS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    *guard
        .as_mut()
        .unwrap()
        .entry(bytes)
        .or_insert_with(fresh_handle)
}

unsafe extern "C" fn fake_classify_heap(_bits: u64) -> u8 {
    MoltTypeTag::Other as u8
}

unsafe extern "C" fn fake_noop_ref(_bits: u64) {}

unsafe extern "C" fn fake_foreign_new(_c_ptr: usize) -> u64 {
    fresh_handle()
}

fn install_runtime_hooks() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_dict = fake_alloc_dict;
    hooks.dict_set = fake_dict_set;
    hooks.dict_get = fake_dict_get;
    hooks.alloc_str = fake_alloc_str;
    hooks.classify_heap = fake_classify_heap;
    hooks.inc_ref = fake_noop_ref;
    hooks.dec_ref = fake_noop_ref;
    hooks.foreign_new = fake_foreign_new;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

/// A struct shaped like a numpy descriptor object: a `PyObject` header followed
/// by members that a `tp_members` table addresses by offset.
#[repr(C)]
struct FakeDescrObject {
    ob_base: PyObject,
    // T_OBJECT member (like arraydescr `typeobj`).
    type_obj: *mut PyObject,
    // T_CHAR member (like arraydescr `kind`).
    kind: c_char,
    // T_INT member (like arraydescr `type_num`).
    type_num: c_int,
    // T_PYSSIZET member (like arraydescr `elsize`).
    elsize: Py_ssize_t,
}

fn member(name: &'static [u8], type_: c_int, offset: usize, flags: c_int) -> PyMemberDef {
    assert_eq!(
        *name.last().unwrap(),
        0,
        "member name must be NUL-terminated"
    );
    PyMemberDef {
        name: name.as_ptr() as *const c_char,
        type_,
        offset: offset as Py_ssize_t,
        flags,
        doc: ptr::null(),
    }
}

fn member_sentinel() -> PyMemberDef {
    PyMemberDef {
        name: ptr::null(),
        type_: 0,
        offset: 0,
        flags: 0,
        doc: ptr::null(),
    }
}

// Member type codes (Py_T_*).
const T_INT: c_int = 1;
const T_OBJECT: c_int = 6;
const T_CHAR: c_int = 7;
const T_PYSSIZET: c_int = 19;
const READONLY: c_int = 1;

// ---------------------------------------------------------------------------
// A getter/setter pair for a getset table (like arraydescr `names`).
// ---------------------------------------------------------------------------

// The getter returns a fixed sentinel int so the test can observe it was
// actually invoked through the descriptor protocol.
unsafe extern "C" fn fake_getter(
    _self: *mut PyObject,
    closure: *mut std::ffi::c_void,
) -> *mut PyObject {
    let value = if closure.is_null() {
        1234
    } else {
        unsafe { *(closure.cast::<c_int>()) }
    };
    unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(value as c_long) }
}

fn getset_def(name: &'static [u8], get: Option<getter>, set: Option<setter>) -> PyGetSetDef {
    assert_eq!(
        *name.last().unwrap(),
        0,
        "getset name must be NUL-terminated"
    );
    PyGetSetDef {
        name: name.as_ptr() as *const c_char,
        get,
        set,
        doc: ptr::null(),
        closure: ptr::null_mut(),
    }
}

fn getset_sentinel() -> PyGetSetDef {
    PyGetSetDef {
        name: ptr::null(),
        get: None,
        set: None,
        doc: ptr::null(),
        closure: ptr::null_mut(),
    }
}

// ===========================================================================
// (1) PyType_Ready populates tp_dict with real member/getset descriptors.
// ===========================================================================

#[test]
fn ready_populates_tp_dict_from_members_and_getset() {
    install_runtime_hooks();

    let mut members = [
        member(
            b"type\0",
            T_OBJECT,
            std::mem::offset_of!(FakeDescrObject, type_obj),
            READONLY,
        ),
        member(
            b"kind\0",
            T_CHAR,
            std::mem::offset_of!(FakeDescrObject, kind),
            READONLY,
        ),
        member(
            b"num\0",
            T_INT,
            std::mem::offset_of!(FakeDescrObject, type_num),
            READONLY,
        ),
        member(
            b"itemsize\0",
            T_PYSSIZET,
            std::mem::offset_of!(FakeDescrObject, elsize),
            READONLY,
        ),
        member_sentinel(),
    ];
    let mut getter_value: c_int = 1234;
    let mut getsets = [
        PyGetSetDef {
            name: c"names".as_ptr(),
            get: Some(fake_getter),
            set: None,
            doc: ptr::null(),
            closure: (&mut getter_value as *mut c_int).cast(),
        },
        getset_sentinel(),
    ];

    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"numpy.dtype_shaped".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<FakeDescrObject>() as Py_ssize_t;
    tp.tp_members = members.as_mut_ptr().cast();
    tp.tp_getset = getsets.as_mut_ptr().cast();

    let rc = unsafe { ready(&mut tp) };
    // This test wires the runtime dict bridge because the witness failure was a
    // descriptor that constructed successfully but disappeared at publication.
    assert_eq!(
        rc, 0,
        "PyType_Ready must succeed for a type carrying tp_members + tp_getset"
    );
    assert!(!tp.tp_dict.is_null(), "tp_dict must be created");
    assert_ne!(
        tp.tp_flags & Py_TPFLAGS_READY,
        0,
        "the type must be marked READY after member/getset population"
    );

    let num_key = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"num".as_ptr()) };
    let num_descr = unsafe { molt_cpython_abi::api::typeobj::_PyType_Lookup(&mut tp, num_key) };
    assert!(
        !num_descr.is_null(),
        "tp_members entry must be visible in tp_dict"
    );
    unsafe {
        assert_eq!((*num_descr).ob_type, &raw mut PyMemberDescr_Type);
    }
    assert!(
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .lock()
            .pyobj_to_handle(num_descr)
            .is_some(),
        "member_descriptor must be bridge-resolvable for dict round trips"
    );

    let names_key =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"names".as_ptr()) };
    let names_descr = unsafe { molt_cpython_abi::api::typeobj::_PyType_Lookup(&mut tp, names_key) };
    assert!(
        !names_descr.is_null(),
        "tp_getset entry must be visible in tp_dict"
    );
    unsafe {
        assert_eq!((*names_descr).ob_type, &raw mut PyGetSetDescr_Type);
    }

    let mut inst = FakeDescrObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &mut tp,
        },
        type_obj: ptr::null_mut(),
        kind: b'i' as c_char,
        type_num: 7,
        elsize: 8,
    };
    let inst_ptr = (&mut inst as *mut FakeDescrObject).cast::<PyObject>();
    let num_value =
        unsafe { molt_cpython_abi::api::object::PyObject_GenericGetAttr(inst_ptr, num_key) };
    assert!(
        !num_value.is_null(),
        "member descriptor must bind through GenericGetAttr"
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(num_value) },
        7
    );

    let inst_dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let shadow_value = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(99) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(inst_dict, num_key, shadow_value) },
        0
    );
    let shadowed = unsafe {
        molt_cpython_abi::api::object::_PyObject_GenericGetAttrWithDict(
            inst_ptr, num_key, inst_dict, 0,
        )
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(shadowed) },
        7,
        "member_descriptor is a data descriptor and must beat instance dict values"
    );

    let names_value =
        unsafe { molt_cpython_abi::api::object::PyObject_GenericGetAttr(inst_ptr, names_key) };
    assert!(
        !names_value.is_null(),
        "getset descriptor must bind through GenericGetAttr"
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(names_value) },
        1234
    );
}

// ===========================================================================
// (2) PyDescr_NewMember / PyDescr_NewGetSet mint real, correctly-typed objects.
// ===========================================================================

#[test]
fn new_member_descriptor_has_correct_type_and_name() {
    // Needs a runtime alloc_str: PyDescr_NewMember builds the descriptor's name
    // via PyUnicode_FromString, which fails closed (NULL) under stub hooks. The
    // previous placeholder let a name-less descriptor be fabricated; the real
    // descriptor requires the fake allocator table.
    install_runtime_hooks();
    let mut memb = member(
        b"num\0",
        T_INT,
        std::mem::offset_of!(FakeDescrObject, type_num),
        READONLY,
    );
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"owner".as_ptr();

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewMember(&mut tp, &mut memb) };
    assert!(
        !descr.is_null(),
        "PyDescr_NewMember must return a real object"
    );
    let none = &raw mut Py_None;
    assert!(
        !std::ptr::eq(descr, none),
        "PyDescr_NewMember must not return the Py_None stub"
    );
    unsafe {
        assert_eq!(
            (*descr).ob_type,
            &raw mut PyMemberDescr_Type,
            "member descriptor must have type member_descriptor"
        );
        // The interned name must be readable via PyDescr_NAME.
        let name = molt_cpython_abi::api::typeobj::PyDescr_NAME(descr);
        assert!(!name.is_null(), "descriptor must carry its interned name");
    }
    // A member descriptor is a data descriptor (its type has tp_descr_set).
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyDescr_IsData(descr) },
        1,
        "member_descriptor must report as a data descriptor"
    );
}

#[test]
fn new_getset_descriptor_has_correct_type() {
    // Needs a runtime alloc_str (see new_member_descriptor_has_correct_type_and_name).
    install_runtime_hooks();
    let mut gs = getset_def(b"names\0", Some(fake_getter), None);
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"owner".as_ptr();

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(&mut tp, &mut gs) };
    assert!(!descr.is_null());
    unsafe {
        assert_eq!(
            (*descr).ob_type,
            &raw mut PyGetSetDescr_Type,
            "getset descriptor must have type getset_descriptor"
        );
    }
}

// ===========================================================================
// (3) The descriptor protocol reads real values.
// ===========================================================================

#[test]
fn member_descriptor_get_reads_struct_field() {
    // Needs a runtime alloc_str for the descriptor name (fails closed under stubs).
    install_runtime_hooks();
    // Build an instance with a known type_num, then read it back through the
    // member_descriptor's tp_descr_get.
    let mut inst = FakeDescrObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        },
        type_obj: ptr::null_mut(),
        kind: b'i' as c_char,
        type_num: 7,
        elsize: 8,
    };
    let mut memb = member(
        b"num\0",
        T_INT,
        std::mem::offset_of!(FakeDescrObject, type_num),
        READONLY,
    );
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"owner".as_ptr();

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewMember(&mut tp, &mut memb) };
    assert!(!descr.is_null());

    let descr_type = unsafe { (*descr).ob_type };
    let get =
        unsafe { (*descr_type).tp_descr_get }.expect("member_descriptor must wire tp_descr_get");
    let value = unsafe {
        get(
            descr,
            (&mut inst as *mut FakeDescrObject).cast::<PyObject>(),
            (&mut tp as *mut PyTypeObject).cast::<PyObject>(),
        )
    };
    assert!(
        !value.is_null(),
        "reading a T_INT member must yield an int object"
    );
    let got = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(value) };
    assert_eq!(
        got, 7,
        "member_descriptor.__get__ must read the field value"
    );
}

#[test]
fn getset_descriptor_get_invokes_getter() {
    // Needs a runtime alloc_str for the descriptor name (fails closed under stubs).
    install_runtime_hooks();
    let mut gs = getset_def(b"names\0", Some(fake_getter), None);
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"owner".as_ptr();
    let mut inst = PyObject {
        ob_refcnt: 1,
        ob_type: &mut tp,
    };

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(&mut tp, &mut gs) };
    assert!(!descr.is_null());
    let descr_type = unsafe { (*descr).ob_type };
    let get =
        unsafe { (*descr_type).tp_descr_get }.expect("getset_descriptor must wire tp_descr_get");
    let value = unsafe {
        get(
            descr,
            &mut inst as *mut PyObject,
            (&mut tp as *mut PyTypeObject).cast::<PyObject>(),
        )
    };
    assert!(!value.is_null(), "getset __get__ must invoke the getter");
    let got = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(value) };
    assert_eq!(
        got, 1234,
        "getset_descriptor.__get__ must call the underlying getter"
    );
}

#[test]
fn readonly_getset_set_raises_attributeerror() {
    // Needs a runtime alloc_str for the descriptor name (fails closed under stubs).
    install_runtime_hooks();
    // A getset with no setter is read-only; writing through it must raise
    // AttributeError (never a silent success).
    let mut gs = getset_def(b"names\0", Some(fake_getter), None);
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"owner".as_ptr();
    let mut inst = PyObject {
        ob_refcnt: 1,
        ob_type: &mut tp,
    };

    let descr = unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(&mut tp, &mut gs) };
    let descr_type = unsafe { (*descr).ob_type };
    let set =
        unsafe { (*descr_type).tp_descr_set }.expect("getset_descriptor must wire tp_descr_set");
    // Clear any pending error first.
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let rc = unsafe { set(descr, &mut inst as *mut PyObject, val) };
    assert_eq!(rc, -1, "writing a read-only getset must fail");
    let pending = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(
        !pending.is_null(),
        "a read-only getset write must leave a pending exception (never a contentless failure)"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ===========================================================================
// (4) Failure path is honest: NULL def -> NULL return + recorded silent failure.
// ===========================================================================

#[test]
fn new_descriptor_with_null_def_fails_and_records() {
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"owner".as_ptr();

    let getset_rc =
        unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(&mut tp, ptr::null_mut()) };
    assert!(
        getset_rc.is_null(),
        "PyDescr_NewGetSet(NULL) must return NULL, not a Py_None stub"
    );
    let member_rc =
        unsafe { molt_cpython_abi::api::typeobj::PyDescr_NewMember(&mut tp, ptr::null_mut()) };
    assert!(
        member_rc.is_null(),
        "PyDescr_NewMember(NULL) must return NULL"
    );
    // The last silent failure must name the descriptor primitive, so the
    // exec-slot diagnostic can pinpoint it instead of a contentless -1.
    let recorded = molt_cpython_abi::capi_trace::take_last_silent_failure();
    assert!(
        recorded
            .as_deref()
            .is_some_and(|s| s.contains("PyDescr_NewMember") || s.contains("PyDescr_NewGetSet")),
        "a NULL-def descriptor creation must record a silent failure; got {recorded:?}"
    );
}

#[test]
fn cpython_abi_header_exposes_descriptor_member_authority() {
    let header_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("include/Python.h");
    let header = std::fs::read_to_string(&header_path).expect("read cpython-abi Python.h");
    for expected in [
        "extern PyObject    *PyDescr_NAME        (PyObject *descr);",
        "extern PyObject    *PyMember_GetOne     (const char *addr, PyMemberDef *member);",
        "extern int          PyMember_SetOne     (char *addr, PyMemberDef *member, PyObject *value);",
    ] {
        assert!(
            header.contains(expected),
            "cpython-abi header must expose descriptor/member authority: {expected}"
        );
    }
}
