//! `PyType_FromSpecWithBases` must process `spec->slots` — the review finding #1
//! reproduction (P0, E1-critical). Before the fix the function set only
//! tp_name/basicsize/itemsize/flags and marked the type READY with a generic
//! alloc/new, silently dropping EVERY custom slot (methods, tp_new/tp_init,
//! number/sequence/mapping protocol, repr/hash/richcompare). A numpy/scipy type
//! built via `PyType_FromSpec` therefore carried zero custom behaviour =
//! fail-open type creation = silent wrong results at runtime.
//!
//! These tests build a `PyType_Spec` with representative slots across every
//! dispatch family the fix must cover and assert the resulting type actually
//! carries them:
//!   * `Py_tp_new`      → `tp_new` set to the supplied function
//!   * `Py_tp_repr`     → `tp_repr` set to the supplied function
//!   * `Py_tp_methods`  → the method is resolvable in `tp_dict`
//!   * `Py_nb_add`      → `tp_as_number` allocated with `nb_add` set
//!   * `Py_tp_doc`      → doc string copied into fresh storage
//!   * `Py_TPFLAGS_READY` present (PyType_Ready ran the full pipeline)
//! plus the fail-closed path: an unrecognised slot id returns NULL with a
//! pending exception (no silent drop).

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::RuntimeHooks;
use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::os::raw::c_int;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

// ── Minimal fake runtime backend (mirrors the cfunction bridge test) ─────────

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x6100_0000);
static DICTS: Mutex<Option<HashMap<u64, HashMap<u64, u64>>>> = Mutex::new(None);

fn fresh_handle() -> u64 {
    NEXT_HANDLE.fetch_add(0x10, Ordering::Relaxed)
}

fn dicts() -> std::sync::MutexGuard<'static, Option<HashMap<u64, HashMap<u64, u64>>>> {
    let mut g = DICTS.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}

unsafe extern "C" fn fake_register_c_function(
    _meth: u64,
    _flags: c_int,
    _self_bits: u64,
    _data: *const u8,
    _len: usize,
) -> u64 {
    fresh_handle()
}

unsafe extern "C" fn fake_alloc_dict() -> u64 {
    let h = fresh_handle();
    dicts().as_mut().unwrap().insert(h, HashMap::new());
    h
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
        .and_then(|m| m.get(&key_bits).copied())
        .unwrap_or(0)
}

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    static STR_HANDLES: Mutex<Option<HashMap<Vec<u8>, u64>>> = Mutex::new(None);
    let bytes = if data.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    let mut g = STR_HANDLES.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    *g.as_mut()
        .unwrap()
        .entry(bytes)
        .or_insert_with(fresh_handle)
}

unsafe extern "C" fn fake_classify_heap(_bits: u64) -> u8 {
    0xFF
}

unsafe extern "C" fn fake_noop_ref(_bits: u64) {}

fn install_hooks() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.register_c_function = fake_register_c_function;
    hooks.alloc_dict = fake_alloc_dict;
    hooks.dict_set = fake_dict_set;
    hooks.dict_get = fake_dict_get;
    hooks.alloc_str = fake_alloc_str;
    hooks.classify_heap = fake_classify_heap;
    hooks.inc_ref = fake_noop_ref;
    hooks.dec_ref = fake_noop_ref;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

// ── Slot callbacks whose identity the test verifies survives dispatch ────────

unsafe extern "C" fn spec_new(
    _t: *mut PyTypeObject,
    _a: *mut PyObject,
    _k: *mut PyObject,
) -> *mut PyObject {
    ptr::null_mut()
}

unsafe extern "C" fn spec_repr(_o: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

unsafe extern "C" fn spec_nb_add(_a: *mut PyObject, _b: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

unsafe extern "C" fn spec_method(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

// Slot ids (CPython 3.12 Include/typeslots.h).
const PY_TP_DOC: c_int = 56;
const PY_TP_METHODS: c_int = 64;
const PY_TP_NEW: c_int = 65;
const PY_TP_REPR: c_int = 66;
const PY_NB_ADD: c_int = 7;

// ===========================================================================
// (1) Every representative slot is installed and PyType_Ready runs fully.
// ===========================================================================
#[test]
fn fromspec_installs_all_slot_families() {
    install_hooks();

    let mut methods = [
        PyMethodDef {
            ml_name: c"do_it".as_ptr(),
            ml_meth: Some(spec_method),
            ml_flags: METH_VARARGS,
            ml_doc: ptr::null(),
        },
        PyMethodDef {
            ml_name: ptr::null(),
            ml_meth: None,
            ml_flags: 0,
            ml_doc: ptr::null(),
        },
    ];

    const DOC: &CStr = c"a spec-built type with real slots";

    // Coerce each callback to a typed function pointer, then to a raw data
    // pointer for the slot's pfunc. Comparing these raw pointers later verifies
    // the exact function survived dispatch (a function-pointer -> raw-pointer
    // cast avoids the fn-item-to-integer and fn-pointer `==` lints).
    let new_fp: unsafe extern "C" fn(
        *mut PyTypeObject,
        *mut PyObject,
        *mut PyObject,
    ) -> *mut PyObject = spec_new;
    let repr_fp: unsafe extern "C" fn(*mut PyObject) -> *mut PyObject = spec_repr;
    let nb_add_fp: unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject = spec_nb_add;
    let new_ptr = new_fp as *mut c_void;
    let repr_ptr = repr_fp as *mut c_void;
    let nb_add_ptr = nb_add_fp as *mut c_void;

    let mut slots = [
        PyType_Slot {
            slot: PY_TP_NEW,
            pfunc: new_ptr,
        },
        PyType_Slot {
            slot: PY_TP_REPR,
            pfunc: repr_ptr,
        },
        PyType_Slot {
            slot: PY_NB_ADD,
            pfunc: nb_add_ptr,
        },
        PyType_Slot {
            slot: PY_TP_METHODS,
            pfunc: methods.as_mut_ptr().cast::<c_void>(),
        },
        PyType_Slot {
            slot: PY_TP_DOC,
            pfunc: DOC.as_ptr() as *mut c_void,
        },
        PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        },
    ];

    let mut spec = PyType_Spec {
        name: c"molt.SpecType".as_ptr(),
        basicsize: std::mem::size_of::<PyObject>() as c_int,
        itemsize: 0,
        flags: Py_TPFLAGS_BASETYPE as u32,
        slots: slots.as_mut_ptr(),
    };

    let obj = unsafe {
        molt_cpython_abi::api::typeobj::PyType_FromSpecWithBases(&mut spec, ptr::null_mut())
    };
    assert!(!obj.is_null(), "PyType_FromSpecWithBases returned NULL");
    let tp = obj.cast::<PyTypeObject>();

    unsafe {
        // READY — the full inherit/dict/mro pipeline ran.
        assert_ne!(
            (*tp).tp_flags & Py_TPFLAGS_READY,
            0,
            "type must be marked READY"
        );

        // tp_new carries the supplied function (not the generic default).
        assert_eq!(
            (*tp).tp_new.map(|f| f as *mut c_void),
            Some(new_ptr),
            "Py_tp_new slot must install tp_new"
        );

        // tp_repr carries the supplied function.
        assert_eq!(
            (*tp).tp_repr.map(|f| f as *mut c_void),
            Some(repr_ptr),
            "Py_tp_repr slot must install tp_repr"
        );

        // tp_as_number allocated and nb_add set to the supplied function.
        assert!(
            !(*tp).tp_as_number.is_null(),
            "Py_nb_add slot must allocate tp_as_number"
        );
        let num = (*tp).tp_as_number.cast::<PyNumberMethods>();
        assert_eq!(
            (*num).nb_add,
            nb_add_ptr,
            "nb_add field must hold the supplied function"
        );
        // A sibling number field we never set stays null (no stray writes).
        assert!(
            (*num).nb_subtract.is_null(),
            "unset number fields must remain null"
        );

        // tp_doc copied into fresh storage (distinct pointer, equal bytes).
        assert!(!(*tp).tp_doc.is_null(), "Py_tp_doc slot must set tp_doc");
        assert_ne!(
            (*tp).tp_doc,
            DOC.as_ptr(),
            "tp_doc must be a fresh copy, not the caller's pointer"
        );
        assert_eq!(
            CStr::from_ptr((*tp).tp_doc),
            DOC,
            "tp_doc copy must equal the source string"
        );

        // Method installed into tp_dict by PyType_Ready.
        assert!(!(*tp).tp_dict.is_null(), "tp_dict must be built");
        let key = molt_cpython_abi::api::strings::PyUnicode_FromString(c"do_it".as_ptr());
        let found = molt_cpython_abi::api::mapping::PyDict_GetItem((*tp).tp_dict, key);
        assert!(
            !found.is_null(),
            "Py_tp_methods entry must be resolvable in tp_dict"
        );
    }
}

// ===========================================================================
// (2) An unrecognised slot id fails closed (no silent drop).
// ===========================================================================
#[test]
fn fromspec_unknown_slot_fails_closed() {
    install_hooks();
    unsafe {
        // Clear any stale exception from earlier tests in this binary.
        molt_cpython_abi::api::errors::PyErr_Clear();
    }

    let mut slots = [
        PyType_Slot {
            slot: 9999, // not a valid CPython 3.12 slot id
            pfunc: ptr::null_mut(),
        },
        PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        },
    ];
    let mut spec = PyType_Spec {
        name: c"molt.BadSpec".as_ptr(),
        basicsize: std::mem::size_of::<PyObject>() as c_int,
        itemsize: 0,
        flags: 0,
        slots: slots.as_mut_ptr(),
    };

    let obj = unsafe {
        molt_cpython_abi::api::typeobj::PyType_FromSpecWithBases(&mut spec, ptr::null_mut())
    };
    assert!(
        obj.is_null(),
        "an unknown slot id must fail closed (return NULL)"
    );
    unsafe {
        assert!(
            !molt_cpython_abi::api::errors::PyErr_Occurred().is_null(),
            "an unknown slot id must set a pending exception"
        );
        molt_cpython_abi::api::errors::PyErr_Clear();
    }
}
