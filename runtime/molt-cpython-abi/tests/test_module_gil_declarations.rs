//! Free-threading readiness contract (PEP 703, `Py_mod_gil`): the module GIL
//! declaration an extension carries MUST be recorded, never silently
//! discarded.
//!
//! Mask-proof: before the recording landed, the module-creation slot loops
//! matched `PY_MOD_GIL => {}` (discard) and `PyUnstable_Module_SetGIL` was a
//! `return 0` stub, so every assertion on `module_gil_declaration(...)` below
//! fails on the pre-change code and passes after.
//!
//! CPython semantics under test (primary sources
//! <https://docs.python.org/3.14/c-api/module.html>,
//! <https://docs.python.org/3.14/howto/free-threading-extensions.html>):
//!   * `{Py_mod_gil, Py_MOD_GIL_NOT_USED}` — declares free-threading support
//!     (exactly what numpy ≥ 2.1 ships on 3.13+).
//!   * `{Py_mod_gil, Py_MOD_GIL_USED}` — explicit opt-out.
//!   * slot absent — DEFAULT is GIL-used; a free-threaded interpreter
//!     re-enables the GIL at import.
//!
//! Valid native import specs supply spec.name before declaration recording.
//! A minimal module transport then exercises failed creation and real exec
//! admission separately. Invalid spec/module inputs are not fake successes.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    MoltTypeTag, PyModuleDef, PyModuleDef_Base, PyModuleDef_Slot, PyObject, PyTypeObject,
};
use molt_cpython_abi::api::modules::{
    PyModule_ExecDef, PyModule_FromDefAndSpec2, PyUnstable_Module_SetGIL,
};
use molt_cpython_abi::gil_declarations::{
    ModuleGilDeclaration, module_gil_declaration, modules_requiring_gil,
    unresolved_gil_declaration_count,
};
use std::collections::HashSet;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::{LazyLock, Mutex};
static MODULES: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
thread_local! {
    static EXEC_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static EXEC_MODULE_CHECK: std::cell::Cell<c_int> = const { std::cell::Cell::new(0) };
}
unsafe extern "C" fn classify(bits: u64) -> u8 {
    if support::fake_strings::contains(bits) {
        MoltTypeTag::Str as u8
    } else if MODULES.lock().unwrap().contains(&bits) {
        MoltTypeTag::Module as u8
    } else {
        MoltTypeTag::Other as u8
    }
}
unsafe extern "C" fn alloc_module(_data: *const u8, _len: usize) -> u64 {
    let bits = molt_lang_obj_model::MoltObject::from_ptr(Box::into_raw(Box::new(0u8))).bits();
    MODULES.lock().unwrap().insert(bits);
    bits
}
unsafe extern "C" fn exec_begin(bits: u64, _definition: usize) -> c_int {
    if MODULES.lock().unwrap().contains(&bits) {
        0
    } else {
        -1
    }
}
fn install_hooks() {
    let mut hooks = support::stub_runtime_hooks();
    support::fake_strings::wire(&mut hooks);
    hooks.classify_heap = classify;
    hooks.alloc_module = alloc_module;
    hooks.module_exec_begin = exec_begin;
    support::prepare_runtime_class_abi_test_thread(hooks);
}
#[repr(C)]
struct NativeSpec {
    object: PyObject,
    name: *const c_char,
}
unsafe extern "C" fn spec_getattro(spec: *mut PyObject, name: *mut PyObject) -> *mut PyObject {
    let key = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(name) };
    if key.is_null() || unsafe { std::ffi::CStr::from_ptr(key) }.to_bytes() != b"name" {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetNone(
                (&raw mut molt_cpython_abi::abi_types::PyExc_AttributeError).cast(),
            )
        };
        return ptr::null_mut();
    }
    unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromString((*spec.cast::<NativeSpec>()).name)
    }
}
unsafe fn from_valid_spec(def: *mut PyModuleDef) -> *mut PyObject {
    let mut type_: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    type_.tp_name = c"test_import_spec".as_ptr();
    type_.tp_getattro = Some(spec_getattro);
    let mut spec = NativeSpec {
        object: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *type_,
        },
        name: unsafe { (*def).m_name },
    };
    unsafe { PyModule_FromDefAndSpec2(def, &raw mut spec.object, 0) }
}

const PY_MOD_EXEC: c_int = 2;
const PY_MOD_GIL: c_int = 4;
const PY_MOD_GIL_USED: *mut c_void = ptr::null_mut(); // ((void *)0)
// ((void *)1) — an integer sentinel, never dereferenced (CPython moduleobject.h).
const PY_MOD_GIL_NOT_USED: *mut c_void = ptr::without_provenance_mut(1);

unsafe extern "C" fn noop_exec(module: *mut PyObject) -> c_int {
    let classification = unsafe { molt_cpython_abi::api::modules::PyModule_Check(module) };
    EXEC_MODULE_CHECK.with(|observed| observed.set(classification));
    if classification != 1 {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast(),
                c"exec callback requires a module".as_ptr(),
            )
        };
        return -1;
    }
    EXEC_CALLS.with(|count| count.set(count.get() + 1));
    0
}

/// Build a leaked (test-'static) PyModuleDef with the given name and slots.
fn make_def(name: &'static str, slots: Vec<PyModuleDef_Slot>) -> *mut PyModuleDef {
    install_hooks();
    assert!(name.ends_with('\0'), "name must be NUL-terminated");
    let mut slots = slots;
    slots.push(PyModuleDef_Slot {
        slot: 0,
        value: ptr::null_mut(),
    });
    let slots: &'static mut [PyModuleDef_Slot] = Box::leak(slots.into_boxed_slice());
    let def = PyModuleDef {
        m_base: unsafe { std::mem::zeroed::<PyModuleDef_Base>() },
        m_name: name.as_ptr() as *const c_char,
        m_doc: ptr::null(),
        m_size: 0,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    Box::leak(Box::new(def))
}

#[test]
fn py_mod_gil_not_used_slot_is_recorded_from_fromdefandspec() {
    // The numpy shape: {Py_mod_exec, ...}, {Py_mod_gil, Py_MOD_GIL_NOT_USED}.
    let def = make_def(
        "gil_itest_notused\0",
        vec![
            PyModuleDef_Slot {
                slot: PY_MOD_EXEC,
                value: noop_exec as *mut c_void,
            },
            PyModuleDef_Slot {
                slot: PY_MOD_GIL,
                value: PY_MOD_GIL_NOT_USED,
            },
        ],
    );
    // Stub hooks make the actual module creation fail (NULL return) — the
    // declaration must be recorded regardless.
    let module = unsafe { from_valid_spec(def) };
    // The metadata registration hook remains unsupported: creation fails only
    // after a valid spec name and declaration have been admitted.
    assert!(module.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        module_gil_declaration("gil_itest_notused"),
        Some(ModuleGilDeclaration::GilNotUsed),
        "Py_mod_gil = Py_MOD_GIL_NOT_USED must be recorded, not discarded"
    );
    assert!(
        !modules_requiring_gil()
            .iter()
            .any(|n| n == "gil_itest_notused"),
        "a declared-free module must not be listed as GIL-requiring"
    );
}

#[test]
fn py_mod_gil_used_explicit_slot_is_recorded() {
    let def = make_def(
        "gil_itest_used_explicit\0",
        vec![PyModuleDef_Slot {
            slot: PY_MOD_GIL,
            value: PY_MOD_GIL_USED,
        }],
    );
    let module = unsafe { from_valid_spec(def) };
    // The metadata registration hook remains unsupported: creation fails only
    // after a valid spec name and declaration have been admitted.
    assert!(module.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        module_gil_declaration("gil_itest_used_explicit"),
        Some(ModuleGilDeclaration::GilUsedExplicit),
        "an explicit Py_MOD_GIL_USED must be recorded as explicit"
    );
    assert!(
        modules_requiring_gil()
            .iter()
            .any(|n| n == "gil_itest_used_explicit"),
        "an explicit GIL-user must be listed as GIL-requiring"
    );
}

#[test]
fn absent_slot_records_cpython_default_gil_used() {
    // Slot array present but WITHOUT Py_mod_gil: CPython default (GIL used).
    let def = make_def(
        "gil_itest_default\0",
        vec![PyModuleDef_Slot {
            slot: PY_MOD_EXEC,
            value: noop_exec as *mut c_void,
        }],
    );
    let module = unsafe { from_valid_spec(def) };
    // The metadata registration hook remains unsupported: creation fails only
    // after a valid spec name and declaration have been admitted.
    assert!(module.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        module_gil_declaration("gil_itest_default"),
        Some(ModuleGilDeclaration::GilUsedDefault),
        "an undeclared module must record the CPython default (GIL used)"
    );
    assert!(
        modules_requiring_gil()
            .iter()
            .any(|n| n == "gil_itest_default"),
        "an undeclared module re-enables the GIL on a free-threaded interpreter"
    );
}

#[test]
fn execdef_records_the_declaration_too() {
    // The two-step loader path: creation elsewhere, exec through
    // PyModule_ExecDef. The exec slot must receive a semantically valid module.
    let def = make_def(
        "gil_itest_execdef\0",
        vec![
            PyModuleDef_Slot {
                slot: PY_MOD_EXEC,
                value: noop_exec as *mut c_void,
            },
            PyModuleDef_Slot {
                slot: PY_MOD_GIL,
                value: PY_MOD_GIL_NOT_USED,
            },
        ],
    );
    let module =
        unsafe { molt_cpython_abi::api::modules::PyModule_New(c"gil_itest_execdef".as_ptr()) };
    assert!(!module.is_null());
    let rc = unsafe { PyModule_ExecDef(module, def) };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module) };
    assert_eq!(
        EXEC_MODULE_CHECK.with(std::cell::Cell::get),
        1,
        "the exec slot must receive an object PyModule_Check accepts"
    );
    assert_eq!(EXEC_CALLS.with(std::cell::Cell::get), 1);
    assert_eq!(rc, 0, "exec slot returning 0 must succeed");
    assert_eq!(
        module_gil_declaration("gil_itest_execdef"),
        Some(ModuleGilDeclaration::GilNotUsed),
        "PyModule_ExecDef must record the Py_mod_gil slot"
    );
}

#[test]
fn setgil_on_unresolvable_module_counts_unresolved_and_still_returns_0() {
    // Without runtime hooks the module name cannot be resolved; the recorder
    // must count the declaration (not drop it) and preserve the historical
    // always-0 return with no pending-exception side effect.
    let before = unresolved_gil_declaration_count();
    let rc = unsafe { PyUnstable_Module_SetGIL(ptr::null_mut(), PY_MOD_GIL_NOT_USED) };
    assert_eq!(rc, 0, "SetGIL must keep the behavior-free 0 return");
    assert_eq!(
        unresolved_gil_declaration_count(),
        before + 1,
        "an unattributable declaration must be counted, not silently dropped"
    );
}

#[test]
fn invalid_definition_inputs_fail_before_metadata_publication() {
    let def = make_def("gil_itest_invalid_spec\0", vec![]);
    unsafe {
        assert!(PyModule_FromDefAndSpec2(def, ptr::null_mut(), 0).is_null());
        assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        assert_eq!(module_gil_declaration("gil_itest_invalid_spec"), None);
        molt_cpython_abi::api::errors::PyErr_Clear();
        assert_eq!(PyModule_ExecDef(ptr::null_mut(), def), -1);
        assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        assert_eq!(module_gil_declaration("gil_itest_invalid_spec"), None);
        molt_cpython_abi::api::errors::PyErr_Clear();
        assert!(molt_cpython_abi::api::modules::PyModuleDef_Init(ptr::null_mut()).is_null());
        assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        molt_cpython_abi::api::errors::PyErr_Clear();
        assert!(molt_cpython_abi::api::modules::PyModule_Create2(ptr::null_mut(), 0).is_null());
        assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        molt_cpython_abi::api::errors::PyErr_Clear();
    }
}
