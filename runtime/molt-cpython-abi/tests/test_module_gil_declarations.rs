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
//! These tests use the canonical stub-hook test transaction: recording happens
//! at module-DEFINITION processing time and must not depend on whether module
//! creation subsequently succeeds (with stub hooks it does not), mirroring
//! CPython, which stamps `md_gil` from the slots before running any exec slot.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{PyModuleDef, PyModuleDef_Base, PyModuleDef_Slot, PyObject};
use molt_cpython_abi::api::modules::{
    PyModule_ExecDef, PyModule_FromDefAndSpec2, PyUnstable_Module_SetGIL,
};
use molt_cpython_abi::gil_declarations::{
    ModuleGilDeclaration, module_gil_declaration, modules_requiring_gil,
    unresolved_gil_declaration_count,
};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

const PY_MOD_EXEC: c_int = 2;
const PY_MOD_GIL: c_int = 4;
const PY_MOD_GIL_USED: *mut c_void = ptr::null_mut(); // ((void *)0)
// ((void *)1) — an integer sentinel, never dereferenced (CPython moduleobject.h).
const PY_MOD_GIL_NOT_USED: *mut c_void = ptr::without_provenance_mut(1);

unsafe extern "C" fn noop_exec(_module: *mut PyObject) -> c_int {
    0
}

/// Build a leaked (test-'static) PyModuleDef with the given name and slots.
fn make_def(name: &'static str, slots: Vec<PyModuleDef_Slot>) -> *mut PyModuleDef {
    support::prepare_abi_test_thread(support::stub_runtime_hooks());
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
    let _ = unsafe { PyModule_FromDefAndSpec2(def, ptr::null_mut(), 0) };
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
    let _ = unsafe { PyModule_FromDefAndSpec2(def, ptr::null_mut(), 0) };
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
    let _ = unsafe { PyModule_FromDefAndSpec2(def, ptr::null_mut(), 0) };
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
    // PyModule_ExecDef. A dummy non-null module suffices — the def's only
    // exec slot ignores it.
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
    let mut dummy = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    let rc = unsafe { PyModule_ExecDef(&raw mut dummy, def) };
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
