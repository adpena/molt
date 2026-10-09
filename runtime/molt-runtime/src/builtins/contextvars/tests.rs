//! Independent semantic obligations from CPython context.c / PEP 567. These
//! exercise the real owner and bridge; ABI-only fixtures cannot emulate them.
use super::*;
use crate::with_gil;
use molt_cpython_abi::abi_types::PyObject;
use molt_cpython_abi::api::{contextvars as c, errors, numbers, refcount};
use std::ptr;

fn fixture(body: impl FnOnce(&PyToken<'_>)) {
    let _runtime = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    with_gil(|py| body(&py));
}
fn variable_named(py: &PyToken<'_>, name: &str, default: Option<u64>) -> u64 {
    let p = crate::alloc_string(py, name.as_bytes());
    assert!(!p.is_null());
    let name = MoltObject::from_ptr(p).bits();
    let out = new_variable(py, name, default).unwrap();
    dec_ref_bits(py, name);
    out
}
fn integer(n: i64) -> u64 {
    MoltObject::from_int(n).bits()
}

#[test]
fn copies_share_roots_but_tokens_require_the_original_context_identity() {
    fixture(|py| {
        clear_thread_context(py);
        let var = variable_named(py, "identity", Some(none()));
        let first = set_variable(py, var, integer(10)).unwrap();
        let origin = current(py).unwrap();
        let copy = copy_current(py).unwrap();
        assert_ne!(origin, copy);
        assert_eq!(
            unsafe { context(origin).root },
            unsafe { context(copy).root },
            "copy must retain one persistent root"
        );
        {
            let _entered = EnteredContext::enter(py, copy).unwrap();
            assert!(
                !reset_variable(py, var, first),
                "equal bindings do not confer token identity"
            );
            unsafe {
                errors::PyErr_Clear();
            }
            let second = set_variable(py, var, integer(20)).unwrap();
            dec_ref_bits(py, second);
            assert_eq!(get_variable(py, var, None), Some(Some(integer(20))));
        }
        assert_eq!(get_variable(py, var, None), Some(Some(integer(10))));
        assert!(reset_variable(py, var, first));
        assert_eq!(get_variable(py, var, None), Some(Some(none())));
        assert!(!reset_variable(py, var, first));
        unsafe {
            errors::PyErr_Clear();
        }
        for bits in [first, copy, var] {
            dec_ref_bits(py, bits);
        }
        clear_thread_context(py);
    });
}

#[test]
fn no_context_is_materialized_by_default_only_reads() {
    fixture(|py| {
        clear_thread_context(py);
        let var = variable_named(py, "default", Some(none()));
        assert_eq!(CURRENT.with(Cell::get), 0);
        for default in [None, Some(integer(7)), Some(0)] {
            let value = get_variable(py, var, default).unwrap().unwrap();
            assert_eq!(value, default.unwrap_or_else(none));
            dec_ref_bits(py, value);
        }
        assert_eq!(
            CURRENT.with(Cell::get),
            0,
            "a get/default must not allocate a Context or root"
        );
        dec_ref_bits(py, var);
    });
}

#[test]
fn collision_persistence_removal_and_iteration_keep_every_physical_edge() {
    fixture(|py| {
        clear_thread_context(py);
        let vars: Vec<_> = (0..40)
            .map(|n| variable_named(py, &format!("v{n}"), None))
            .collect();
        // Deliberately identical hashes select the otherwise impractical collision
        // case. Expected values come from the independent indexed input, not trie
        // lookup; variable identity remains distinct despite equal names/hashes.
        for &var in &vars {
            unsafe {
                (*obj_from_bits(var).as_ptr().unwrap().cast::<Variable>()).hash = 17;
            }
        }
        let mut tokens = Vec::new();
        for (i, &var) in vars.iter().enumerate() {
            tokens.push(set_variable(py, var, integer(i as i64)).unwrap());
        }
        let snapshot = copy_current(py).unwrap();
        let root = unsafe { context(snapshot).root };
        let mut cursor = trie::Cursor::new(root);
        let mut got = Vec::new();
        while let Some((k, v)) = cursor.next() {
            got.push((k, v));
        }
        got.sort();
        let mut expected: Vec<_> = vars
            .iter()
            .enumerate()
            .map(|(i, &var)| (var, integer(i as i64)))
            .collect();
        expected.sort();
        assert_eq!(got, expected);
        for (&var, &token) in vars.iter().zip(&tokens).rev() {
            assert!(reset_variable(py, var, token));
        }
        assert_eq!(unsafe { context(current(py).unwrap()).len }, 0);
        assert_eq!(unsafe { context(current(py).unwrap()).root }, 0);
        for (i, &var) in vars.iter().enumerate() {
            assert_eq!(
                trie::lookup(root, var, var_hash(var)),
                Some(integer(i as i64))
            );
        }
        for bits in tokens.into_iter().chain(vars).chain([snapshot]) {
            dec_ref_bits(py, bits);
        }
        clear_thread_context(py);
    });
}

#[test]
fn nested_enter_restores_previous_and_same_context_reentry_fails() {
    fixture(|py| {
        clear_thread_context(py);
        let a = new_context(py).unwrap();
        let b = new_context(py).unwrap();
        assert!(enter(py, a));
        assert!(!enter(py, a));
        unsafe {
            errors::PyErr_Clear();
        }
        assert!(enter(py, b));
        assert!(!exit(py, a));
        unsafe {
            errors::PyErr_Clear();
        }
        assert_eq!(CURRENT.with(Cell::get), b);
        assert_eq!(unsafe { context(b).previous }, a);
        assert!(exit(py, b));
        assert_eq!(CURRENT.with(Cell::get), a);
        assert_eq!(unsafe { context(b).previous }, 0);
        assert!(exit(py, a));
        assert_eq!(CURRENT.with(Cell::get), 0);
        assert!(enter(py, a));
        assert!(exit(py, a));
        dec_ref_bits(py, a);
        dec_ref_bits(py, b);
    });
}

#[test]
fn public_c_api_shares_runtime_identity_defaults_tokens_and_snapshots() {
    fixture(|py| unsafe {
        clear_thread_context(py);
        let default = numbers::PyLong_FromLong(10);
        let caller = numbers::PyLong_FromLong(20);
        let var = c::PyContextVar_New(c"".as_ptr(), default);
        assert!(!var.is_null(), "empty names are legal");
        assert_eq!(c::PyContextVar_CheckExact(var), 1);
        let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(var)
            .unwrap()
            .bits();
        assert_eq!(get_variable(py, bits, None), Some(Some(integer(10))));
        let mut out: *mut PyObject = ptr::null_mut();
        assert_eq!(c::PyContextVar_Get(var, caller, &mut out), 0);
        assert_eq!(out, caller);
        refcount::Py_DECREF(out);
        let token = c::PyContextVar_Set(var, caller);
        assert!(!token.is_null());
        assert_ne!(token, caller);
        assert_eq!(c::PyContextToken_CheckExact(token), 1);
        assert_eq!(get_variable(py, bits, None), Some(Some(integer(20))));
        let copied = c::PyContext_CopyCurrent();
        assert_eq!(c::PyContext_CheckExact(copied), 1);
        assert_eq!(c::PyContext_Enter(copied), 0);
        assert_eq!(c::PyContextVar_Reset(var, token), -1);
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        assert_eq!(c::PyContext_Exit(copied), 0);
        assert_eq!(c::PyContextVar_Reset(var, token), 0);
        assert_eq!(c::PyContextVar_Reset(var, token), -1);
        errors::PyErr_Clear();
        let missing = c::PyContextVar_New(c"missing".as_ptr(), ptr::null_mut());
        out = usize::MAX as *mut PyObject;
        assert_eq!(c::PyContextVar_Get(missing, ptr::null_mut(), &mut out), 0);
        assert!(out.is_null());
        assert!(errors::PyErr_Occurred().is_null());
        assert_eq!(c::PyContextVar_Get(caller, ptr::null_mut(), &mut out), -1);
        assert!(out.is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        for object in [default, caller, var, token, copied, missing] {
            refcount::Py_DECREF(object);
        }
        clear_thread_context(py);
    });
}

#[test]
fn allocation_failure_is_atomic_and_failed_reset_consumes_the_token() {
    fixture(|py| {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct Budget;
        impl Drop for Budget {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let deny = || {
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            Budget
        };
        clear_thread_context(py);
        let var = variable_named(py, "oom", None);
        let initial = set_variable(py, var, integer(1)).unwrap();
        let reset = set_variable(py, var, integer(2)).unwrap();
        let ctx = current(py).unwrap();
        let root = unsafe { context(ctx).root };
        let budget = deny();
        assert!(set_variable(py, var, integer(3)).is_none());
        drop(budget);
        unsafe {
            errors::PyErr_Clear();
        }
        assert_eq!(unsafe { context(ctx).root }, root);
        assert_eq!(get_variable(py, var, None), Some(Some(integer(2))));
        let budget = deny();
        assert!(!reset_variable(py, var, reset));
        drop(budget);
        unsafe {
            errors::PyErr_Clear();
        }
        assert_eq!(unsafe { context(ctx).root }, root);
        assert!(
            !reset_variable(py, var, reset),
            "failed mutation has already consumed the reset token"
        );
        unsafe {
            errors::PyErr_Clear();
        }
        assert!(reset_variable(py, var, initial));
        for bits in [initial, reset, var] {
            dec_ref_bits(py, bits);
        }
        clear_thread_context(py);
    });
}

#[cfg(not(target_arch = "wasm32"))]
extern "C" fn observe_context_poll(raw_task: u64) -> i64 {
    with_gil(|py| unsafe {
        let task = std::ptr::with_exposed_provenance_mut::<u64>(raw_task as usize);
        let var = *task;
        get_variable(&py, var, None).flatten().unwrap_or_else(none) as i64
    })
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn scheduled_poll_leases_owned_context_but_manual_poll_inherits_and_restores() {
    fixture(|py| {
        use crate::async_rt::cancellation::{TaskContextBinding, register_task_execution};
        clear_thread_context(py);
        let var = variable_named(py, "poll", Some(integer(0)));
        let ambient = set_variable(py, var, integer(1)).unwrap();
        let selected = new_context(py).unwrap();
        {
            let _entry = EnteredContext::enter(py, selected).unwrap();
            let token = set_variable(py, var, integer(2)).unwrap();
            dec_ref_bits(py, token);
        }
        let poll = observe_context_poll as *const () as usize as u64;
        let task = crate::molt_task_new(poll, 8, crate::TASK_KIND_FUTURE);
        let task_ptr = obj_from_bits(task).as_ptr().unwrap();
        unsafe {
            crate::object::payload_refs::store_borrowed(py, task_ptr, 0, var);
        }
        register_task_execution(py, task_ptr, 1, TaskContextBinding::Owned(selected));
        for _ in 0..2 {
            assert_eq!(
                unsafe { crate::async_rt::poll::call_scheduled_poll_fn(py, poll, task_ptr) } as u64,
                integer(2)
            );
            assert_eq!(get_variable(py, var, None), Some(Some(integer(1))));
            assert_eq!(
                unsafe { context(selected).entered },
                0,
                "each completed poll releases its entry lease"
            );
            assert_eq!(
                unsafe { crate::async_rt::poll::call_poll_fn(py, poll, task_ptr) } as u64,
                integer(1),
                "manual polling ignores an attached scheduled context"
            );
        }
        let mut edges = Vec::new();
        unsafe {
            crate::object::heap_lifecycle::visit_owned_values(py, task_ptr, &mut |bits| {
                edges.push(bits)
            });
        }
        assert_eq!(
            edges.iter().filter(|&&bits| bits == selected).count(),
            1,
            "the task owns one Context attachment edge"
        );
        register_task_execution(py, task_ptr, 1, TaskContextBinding::OwnedEmpty);
        assert_eq!(
            unsafe { crate::async_rt::poll::call_scheduled_poll_fn(py, poll, task_ptr) } as u64,
            integer(0)
        );
        assert_eq!(
            crate::async_rt::cancellation::task_context_binding(py, task_ptr),
            TaskContextBinding::OwnedEmpty,
            "default reads leave an empty scheduled context unmaterialized"
        );
        assert_eq!(get_variable(py, var, None), Some(Some(integer(1))));
        assert!(reset_variable(py, var, ambient));
        for bits in [task, selected, ambient, var] {
            dec_ref_bits(py, bits);
        }
        clear_thread_context(py);
    });
}

#[test]
fn cold_c_shell_export_precedes_module_identity_and_constructor_dispatch() {
    // Each ordering starts with new runtime class slots and retired old C
    // bindings. The same test covers all three shells before any owner call.
    for ordering in 0..6 {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            with_gil(|py| unsafe {
                use molt_cpython_abi::abi_types::{
                    PyContext_Type, PyContextToken_Type, PyContextVar_Type,
                };
                use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
                let shells = [
                    (&raw mut PyContext_Type).cast::<PyObject>(),
                    (&raw mut PyContextVar_Type).cast::<PyObject>(),
                    (&raw mut PyContextToken_Type).cast::<PyObject>(),
                ];
                assert_eq!(
                    crate::runtime_state(&py)
                        .types
                        .context_class
                        .load(std::sync::atomic::Ordering::Acquire),
                    0
                );
                if ordering == 1 {
                    let object = molt_cpython_abi::api::object::PyObject_CallNoArgs(shells[0]);
                    assert!(!object.is_null());
                    refcount::Py_DECREF(object);
                } else if ordering == 2 {
                    let object = c::PyContext_New();
                    assert!(!object.is_null());
                    refcount::Py_DECREF(object);
                }
                if ordering == 3 {
                    let name = MoltObject::from_ptr(crate::alloc_string(&py, b"cold-var")).bits();
                    let argument = GLOBAL_BRIDGE.owned_handle_to_pyobj(name);
                    let variable =
                        molt_cpython_abi::api::object::PyObject_CallOneArg(shells[1], argument);
                    assert!(!variable.is_null());
                    refcount::Py_DECREF(variable);
                    refcount::Py_DECREF(argument);
                } else if ordering == 4 {
                    let token = molt_cpython_abi::api::object::PyObject_CallNoArgs(shells[2]);
                    assert!(token.is_null());
                    assert!(!errors::PyErr_Occurred().is_null());
                    errors::PyErr_Clear();
                } else if ordering == 5 {
                    let name = molt_cpython_abi::api::object::PyObject_GetAttrString(
                        shells[0],
                        c"__name__".as_ptr(),
                    );
                    assert!(!name.is_null());
                    refcount::Py_DECREF(name);
                }
                let exported =
                    shells.map(|shell| GLOBAL_BRIDGE.molt_value_for_pyobj(shell).unwrap());
                let name = crate::alloc_string(&py, b"_contextvars");
                let name_bits = MoltObject::from_ptr(name).bits();
                let module = crate::molt_module_new(name_bits);
                let namespace = molt_contextvars_types(module);
                let tuple = obj_from_bits(namespace).as_ptr().unwrap();
                for (index, (&bits, &shell)) in exported.iter().zip(&shells).enumerate() {
                    assert_eq!(
                        crate::object::seq_access::with_immutable_tuple_slice(tuple, |values| {
                            values[index]
                        }),
                        Some(bits)
                    );
                    assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits), shell);
                    let name = molt_cpython_abi::api::object::PyObject_GetAttrString(
                        shell,
                        c"__name__".as_ptr(),
                    );
                    assert!(!name.is_null());
                    refcount::Py_DECREF(name);
                    dec_ref_bits(&py, bits);
                }
                let token = molt_cpython_abi::api::object::PyObject_CallNoArgs(shells[2]);
                assert!(token.is_null());
                assert!(!errors::PyErr_Occurred().is_null());
                errors::PyErr_Clear();
                for bits in [namespace, module, name_bits] {
                    dec_ref_bits(&py, bits);
                }
                clear_thread_context(&py);
            });
        });
    }
}

#[test]
fn staged_trie_failures_preserve_shared_snapshot_at_later_allocation_ordinals() {
    fixture(|py| {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct Budget;
        impl Drop for Budget {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        clear_thread_context(py);
        let vars: Vec<_> = (0..4)
            .map(|i| variable_named(py, &format!("staged-{i}"), None))
            .collect();
        // A collision leaf below a long common-prefix bitmap chain plus a
        // distinct root branch exercises both persistent node representations.
        for (&var, hash) in vars.iter().zip([0, 0, 1 << 30, 1]) {
            unsafe {
                (*obj_from_bits(var).as_ptr().unwrap().cast::<Variable>()).hash = hash;
            }
            let token = set_variable(py, var, integer(10)).unwrap();
            dec_ref_bits(py, token);
        }
        let snapshot = copy_current(py).unwrap();
        let old_root = unsafe { context(snapshot).root };
        let mut denied_later = false;
        let mut completed = false;
        for allowed in 0..32 {
            let working = copy_context(py, snapshot).unwrap();
            {
                let _entry = EnteredContext::enter(py, working).unwrap();
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(allowed),
                    ..Default::default()
                })));
                let budget = Budget;
                let outcome = set_variable(py, vars[0], integer(20));
                drop(budget);
                if let Some(token) = outcome {
                    assert_eq!(get_variable(py, vars[0], None), Some(Some(integer(20))));
                    assert!(reset_variable(py, vars[0], token));
                    dec_ref_bits(py, token);
                    completed = true;
                } else {
                    unsafe {
                        errors::PyErr_Clear();
                    }
                    denied_later |= allowed > 1;
                    assert_eq!(unsafe { context(working).root }, old_root);
                    assert_eq!(get_variable(py, vars[0], None), Some(Some(integer(10))));
                }
                assert_eq!(unsafe { context(snapshot).root }, old_root);
                assert_eq!(
                    trie::lookup(old_root, vars[0], var_hash(vars[0])),
                    Some(integer(10))
                );
            }
            dec_ref_bits(py, working);
            if completed {
                break;
            }
        }
        assert!(
            denied_later && completed,
            "exercise staged denial and eventual success"
        );
        let mut reset_denied_later = false;
        let mut reset_completed = false;
        for allowed in 0..32 {
            let working = copy_context(py, snapshot).unwrap();
            {
                let _entry = EnteredContext::enter(py, working).unwrap();
                let token = set_variable(py, vars[0], integer(20)).unwrap();
                let before = unsafe { context(working).root };
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(allowed),
                    ..Default::default()
                })));
                let budget = Budget;
                let reset = reset_variable(py, vars[0], token);
                drop(budget);
                if reset {
                    assert_eq!(get_variable(py, vars[0], None), Some(Some(integer(10))));
                    reset_completed = true;
                } else {
                    unsafe {
                        errors::PyErr_Clear();
                    }
                    reset_denied_later |= allowed > 0;
                    assert_eq!(unsafe { context(working).root }, before);
                    assert_eq!(get_variable(py, vars[0], None), Some(Some(integer(20))));
                    assert!(
                        !reset_variable(py, vars[0], token),
                        "failed reset consumed token before staging"
                    );
                    unsafe {
                        errors::PyErr_Clear();
                    }
                }
                assert_eq!(unsafe { context(snapshot).root }, old_root);
                dec_ref_bits(py, token);
            }
            dec_ref_bits(py, working);
            if reset_completed {
                break;
            }
        }
        assert!(reset_denied_later && reset_completed);

        for bits in vars.into_iter().chain([snapshot]) {
            dec_ref_bits(py, bits);
        }
        clear_thread_context(py);
    });
}

#[cfg(not(target_arch = "wasm32"))]
extern "C" fn mutate_empty_context_poll(raw_task: u64) -> i64 {
    with_gil(|py| unsafe {
        let task = std::ptr::with_exposed_provenance_mut::<u64>(raw_task as usize);
        let var = *task;
        let observed = get_variable(&py, var, None).flatten().unwrap_or(0);
        if let Some(token) = set_variable(&py, var, integer(99)) {
            dec_ref_bits(&py, token);
        }
        observed as i64
    })
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn owned_empty_poll_materializes_once_persists_and_restores_after_allocation_failure() {
    fixture(|py| {
        use crate::async_rt::cancellation::{TaskContextBinding, register_task_execution};
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct Budget;
        impl Drop for Budget {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        clear_thread_context(py);
        let var = variable_named(py, "empty-poll", Some(integer(0)));
        let ambient = set_variable(py, var, integer(1)).unwrap();
        let address = mutate_empty_context_poll as *const () as usize as u64;
        let task = crate::molt_task_new(address, 8, crate::TASK_KIND_FUTURE);
        let pointer = obj_from_bits(task).as_ptr().unwrap();
        unsafe {
            crate::object::payload_refs::store_borrowed(py, pointer, 0, var);
        }
        register_task_execution(py, pointer, 1, TaskContextBinding::OwnedEmpty);
        for expected in [integer(0), integer(99)] {
            assert_eq!(
                unsafe { crate::async_rt::poll::call_scheduled_poll_fn(py, address, pointer) }
                    as u64,
                expected
            );
            assert_eq!(get_variable(py, var, None), Some(Some(integer(1))));
        }
        let binding = crate::async_rt::cancellation::task_context_binding(py, pointer);
        assert!(matches!(binding, TaskContextBinding::Owned(_)));
        // A fresh empty binding fails at first materialization and restores the
        // already populated predecessor without publishing a partial Context.
        register_task_execution(py, pointer, 1, TaskContextBinding::OwnedEmpty);
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(0),
            ..Default::default()
        })));
        let budget = Budget;
        unsafe {
            crate::async_rt::poll::call_scheduled_poll_fn(py, address, pointer);
        }
        drop(budget);
        assert!(crate::exception_pending(py));
        unsafe {
            errors::PyErr_Clear();
        }
        assert_eq!(
            crate::async_rt::cancellation::task_context_binding(py, pointer),
            TaskContextBinding::OwnedEmpty
        );
        assert_eq!(get_variable(py, var, None), Some(Some(integer(1))));
        assert!(reset_variable(py, var, ambient));
        for bits in [task, var, ambient] {
            dec_ref_bits(py, bits);
        }
        clear_thread_context(py);
    });
}
