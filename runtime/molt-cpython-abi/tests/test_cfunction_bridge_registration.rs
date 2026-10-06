//! PyCFunction_NewEx must return a *bridge-registered* PyObject when a runtime
//! is wired, so the callable resolves back to a Molt handle via
//! `pyobj_to_handle`. This is the exact contract that PyType_Ready's tp_dict
//! method population depends on: native method descriptors survive dictionary
//! ownership and bind to concrete, bridge-resolvable CFunctions. Missing foreign
//! descriptor custody or callable registration must not silently drop a method
//! or erase its PyMethodDef/receiver layout.
//!
//! This is a dedicated test binary so it owns a fresh `OnceLock` for the runtime
//! hook vtable: it installs a minimal hook set with a working
//! `register_c_function` and dict backend, then proves the full store/retrieve
//! chain. (Pure STUB hooks make register_c_function return 0, exercising only
//! the raw-object fallback; the real bridge path needs a live runtime, mirrored
//! here by the minimal fakes.)

#![allow(non_snake_case)]

mod support;
use support::fake_runtime::{fresh_handle, inc_ref as fake_inc_ref};

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::RuntimeHooks;
use std::os::raw::c_char;
use std::ptr;

// ── Minimal fake runtime backend ───────────────────────────────────────────
// A tiny handle-keyed store: dicts are handles mapping key-bits -> value-bits.

fn install_hooks() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    support::prepare_runtime_class_abi_test_thread(hooks);
}

unsafe extern "C" fn dummy_method(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

fn method_def(name: &'static [u8]) -> PyMethodDef {
    PyMethodDef {
        ml_name: name.as_ptr() as *const c_char,
        ml_meth: Some(dummy_method),
        ml_flags: METH_VARARGS,
        ml_doc: ptr::null(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn cfunction_newex_returns_bridge_resolvable_object() {
    install_hooks();
    let mut ml = method_def(b"reduce\0");
    let func = unsafe {
        molt_cpython_abi::api::object::PyCFunction_NewEx(&mut ml, ptr::null_mut(), ptr::null_mut())
    };
    assert!(!func.is_null(), "PyCFunction_NewEx must return a callable");
    assert_eq!(
        unsafe { (*func).ob_type },
        &raw mut PyCFunction_Type,
        "runtime-backed C functions must retain exact PyCFunction_Type identity",
    );
    assert_eq!(
        unsafe { (*(func.cast::<PyCFunctionObject>())).m_ml },
        &raw mut ml,
        "runtime-backed C functions must retain their PyMethodDef layout",
    );
    // The returned object must resolve back to a Molt handle — the whole point
    // of the fix. A raw, unregistered object would return None here.
    let handle = molt_cpython_abi::bridge::GLOBAL_BRIDGE.pyobj_to_handle(func);
    assert!(
        handle.is_some(),
        "PyCFunction_NewEx result must be bridge-registered so PyDict_SetItem can store it"
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(func) };
}

#[test]
fn cfunction_descriptor_stores_and_retrieves_in_type_dict() {
    install_hooks();
    // Full chain: PyType_Ready publishes native method descriptors, dictionary
    // ownership retains them, and descriptor binding constructs a CFunction.
    let mut methods = [
        method_def(b"reduce\0"),
        PyMethodDef {
            ml_name: ptr::null(),
            ml_meth: None,
            ml_flags: 0,
            ml_doc: ptr::null(),
        },
    ];
    let mut tp = support::StaticType::new();
    tp.ob_base.ob_base.ob_refcnt = 1;
    tp.tp_name = c"scalar".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    tp.tp_methods = methods.as_mut_ptr();

    let rc = unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(tp.as_ptr()) };
    assert_eq!(rc, 0);
    assert!(!tp.tp_dict.is_null());

    let key = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"reduce".as_ptr()) };
    let found = unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(tp.tp_dict, key) };
    assert!(
        !found.is_null(),
        "method descriptor stored by PyType_Ready must be retrievable from tp_dict"
    );
    assert_eq!(unsafe { (*found).ob_type }, &raw mut PyMethodDescr_Type);
    let descriptor = found.cast::<PyMethodDescrObject>();
    assert_eq!(unsafe { (*descriptor).d_method }, methods.as_mut_ptr());
    assert_eq!(unsafe { (*descriptor).d_common.d_type }, tp.as_ptr());
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: tp.as_ptr(),
    };
    unsafe {
        let get = (*(*found).ob_type)
            .tp_descr_get
            .expect("method descriptor get slot");
        let bound = get(found, &raw mut receiver, tp.as_ptr().cast());
        assert!(
            !bound.is_null(),
            "retrieved descriptor must bind to its receiver"
        );
        assert_eq!((*bound).ob_type, &raw mut PyCFunction_Type);
        assert_eq!(
            (*bound.cast::<PyCFunctionObject>()).m_ml,
            methods.as_mut_ptr()
        );
        assert_eq!(
            (*bound.cast::<PyCFunctionObject>()).m_self,
            &raw mut receiver
        );
        assert!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .pyobj_to_handle(bound)
                .is_some()
        );
        molt_cpython_abi::api::refcount::Py_DECREF(bound);
        assert_eq!(
            receiver.ob_refcnt, 1,
            "bound callable releases its receiver"
        );
        molt_cpython_abi::api::refcount::Py_DECREF(key);
    }
}

#[test]
fn cfunction_newex_null_methoddef_returns_null() {
    install_hooks();
    let out = unsafe {
        molt_cpython_abi::api::object::PyCFunction_NewEx(
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert!(out.is_null());
}

#[test]
fn runtime_backed_callable_views_own_member_edges_until_runtime_retirement() {
    install_hooks();
    let bridge = &*molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    unsafe {
        let receiver = bridge.owned_handle_to_pyobj(fresh_handle());
        let module = bridge.owned_handle_to_pyobj(fresh_handle());
        let class = bridge.owned_handle_to_pyobj(fresh_handle());
        let edges = [receiver, module, class];
        for (flags, defining_class, expected_type) in [
            (METH_VARARGS, ptr::null_mut(), &raw mut PyCFunction_Type),
            (
                METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
                class.cast::<PyTypeObject>(),
                &raw mut PyCMethod_Type,
            ),
        ] {
            let before = edges.map(|edge| (*edge).ob_refcnt);
            let mut ml = PyMethodDef {
                ml_flags: flags,
                ..method_def(b"owned\0")
            };
            let function = molt_cpython_abi::api::object::PyCMethod_New(
                &mut ml,
                receiver,
                module,
                defining_class,
            );
            assert!(!function.is_null());
            assert_eq!((*function).ob_type, expected_type);
            let adopted = [1, 1, isize::from(!defining_class.is_null())];
            for ((edge, before), adopted) in edges.iter().zip(before).zip(adopted) {
                assert_eq!((**edge).ob_refcnt, before + adopted);
            }
            let bits = bridge
                .molt_handle_for_pyobj(function)
                .expect("runtime-backed callable is bridge-registered")
                .bits();
            // A runtime owner such as a module dict outlives the constructor
            // reference; the physical layout must survive that release.
            fake_inc_ref(bits);
            molt_cpython_abi::api::refcount::Py_DECREF(function);
            assert_eq!(bridge.handle_to_borrowed_pyobj(bits), function);
            assert_eq!((*function).ob_type, expected_type);
            assert_eq!((*function.cast::<PyCFunctionObject>()).m_ml, &raw mut ml);
            let mut gc_edges = Vec::new();
            bridge.visit_physical_owned_edges_for_gc(bits, &mut |edge| gc_edges.push(edge));
            assert_eq!(gc_edges.len(), 1);
            assert_eq!(
                gc_edges[0].kind,
                molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8
            );
            assert_eq!(
                gc_edges[0].value,
                bridge.molt_handle_for_pyobj(module).unwrap().bits()
            );
            for ((edge, before), adopted) in edges.iter().zip(before).zip(adopted) {
                assert_eq!((**edge).ob_refcnt, before + adopted);
            }
            // Runtime terminal retirement is the only release of member edges.
            drop(
                bridge
                    .retire_runtime_object_deferred(bits)
                    .expect("callable is a canonical managed view"),
            );
            gc_edges.clear();
            bridge.visit_physical_owned_edges_for_gc(bits, &mut |edge| gc_edges.push(edge));
            assert!(gc_edges.is_empty());
            for (edge, before) in edges.iter().zip(before) {
                assert_eq!(
                    (**edge).ob_refcnt,
                    before,
                    "member edge released exactly once"
                );
            }
        }
        for edge in edges {
            molt_cpython_abi::api::refcount::Py_DECREF(edge);
        }
    }
}
