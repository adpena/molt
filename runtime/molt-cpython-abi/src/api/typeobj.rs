//! Type object API — PyType_Ready, PyType_GenericAlloc, Py_TYPE checks.

use crate::abi_types::{
    Py_TPFLAGS_HAVE_GC, Py_TPFLAGS_READY, Py_ssize_t, PyMethodDef, PyObject, PyType_Spec,
    PyTypeObject,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_longlong, c_ulonglong};
use std::ptr;

static ABI_LOCAL_TYPES: Lazy<Mutex<HashMap<u32, usize>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_type_canonicalize(
    kind: u32,
    type_obj: *mut PyTypeObject,
) -> *mut PyTypeObject {
    if kind == 0 || type_obj.is_null() {
        return ptr::null_mut();
    }

    let mut guard = ABI_LOCAL_TYPES.lock();
    if let Some(canonical) = guard.get(&kind) {
        return *canonical as *mut PyTypeObject;
    }

    let mut canonical: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    unsafe {
        ptr::copy_nonoverlapping(type_obj, canonical.as_mut(), 1);
        if canonical.ob_base.ob_base.ob_type.is_null() {
            canonical.ob_base.ob_base.ob_type = &raw mut crate::abi_types::PyType_Type;
        }
    }
    let canonical = Box::into_raw(canonical);
    guard.insert(kind, canonical as usize);
    canonical
}

/// Mark a type as ready for use.
/// In Molt's bridge, static type objects are pre-initialized; heap types
/// need basic tp_base resolution.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Ready(tp: *mut PyTypeObject) -> c_int {
    // Unconditional entry trace (before the null check) so *every* call site is
    // visible, including a null/unresolved `tp`. This distinguishes "the caller
    // was never reached" from "the caller passed a bad pointer": if a static
    // extension's exec sequence stops here we see the raw pointer that arrived.
    crate::capi_trace::trace_call("PyType_Ready:entry", Some(&format!("{:p}", tp)));
    if tp.is_null() {
        // A NULL type here is a real linkage/authority failure (a static
        // extension resolved a builtin type symbol such as `PyBool_Type` to a
        // null weak symbol). CPython would crash; we fail closed with an honest
        // record so the exec-failure path can name the exact site instead of
        // vanishing before any trace fires.
        crate::capi_trace::record_silent_failure("PyType_Ready", Some("null type"));
        return -1;
    }
    let name = unsafe { (*tp).tp_name };
    let label = if name.is_null() {
        format!("<unnamed@{:p}>", tp)
    } else {
        unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    crate::capi_trace::trace_call("PyType_Ready", Some(&label));

    // Idempotent: a type readied once (numpy's builtin PyType_Ready(&PyBool_Type)
    // calls, or a re-entrant static-init) must not be re-processed. Still register
    // it in the bridge (idempotent) so an already-ready static type the extension
    // hands back — e.g. `PyBool_Type`, readied at `init_static_types` — resolves
    // via `pyobj_to_handle` instead of failing the bridge lookup.
    if unsafe { (*tp).tp_flags } & Py_TPFLAGS_READY != 0 {
        unsafe {
            crate::bridge::GLOBAL_BRIDGE
                .lock()
                .register_raw_pyobj(tp.cast::<PyObject>());
            install_metatype_getattro(tp);
        }
        return 0;
    }

    unsafe {
        // (1) Default a missing base to `object` — every static type except
        //     object itself inherits from PyBaseObject_Type. numpy's
        //     SINGLE_INHERIT sets tp_base explicitly, but the root
        //     PyGenericArrType_Type and the builtin bases rely on this default.
        let object = &raw mut crate::abi_types::PyBaseObject_Type;
        if (*tp).tp_base.is_null() && !ptr::eq(tp, object) {
            (*tp).tp_base = object;
        }

        // (2) Inherit unset slots from the base type — exactly as CPython's
        //     inherit_slots does. A static C extension (numpy's scalar type
        //     hierarchy is the canonical case) sets `tp_base` and trusts
        //     PyType_Ready to copy the base's function slots into the child
        //     wherever the child left them null. Skipping this leaves derived
        //     types with null number/compare/hash slots and later operations
        //     fail opaquely.
        if !(*tp).tp_base.is_null() {
            inherit_slots_from_base(tp, (*tp).tp_base);
        }

        // (2b) Guarantee the object-lifecycle allocator slots CPython fills
        //      during PyType_Ready. CPython's post-ready invariant (verified
        //      against CPython 3.12: non-GC builtins carry
        //      `tp_free == PyObject_Free`, GC builtins carry
        //      `tp_free == PyObject_GC_Del`, and every readied type carries
        //      `tp_alloc == PyType_GenericAlloc`) is that `tp_free` and
        //      `tp_alloc` are NON-NULL. A static C extension leaves them NULL and
        //      trusts PyType_Ready to fill them from `object`: numpy's
        //      `PyBoundArrayMethod_Type` (`Py_TPFLAGS_DEFAULT`, no `tp_free`) is
        //      the canonical case — its `boundarraymethod_dealloc` ends in
        //      `Py_TYPE(self)->tp_free(self)`. Leaving `tp_free` NULL turns that
        //      into a `call_indirect` on table index 0, which traps with
        //      "null function or function signature mismatch" on the first
        //      dealloc. (In the wasm runtime PyObject_Free / PyObject_GC_Del /
        //      PyMem_Free all route to `libc::free`, so the GC split is
        //      CPython-faithful bookkeeping rather than a behavioral fork.)
        if (*tp).tp_free.is_none() {
            (*tp).tp_free = Some(if (*tp).tp_flags & Py_TPFLAGS_HAVE_GC != 0 {
                crate::api::memory::PyObject_GC_Del
            } else {
                crate::api::memory::PyObject_Free
            });
        }
        if (*tp).tp_alloc.is_none() {
            (*tp).tp_alloc = Some(PyType_GenericAlloc);
        }

        // (3) Build tp_dict and populate it from the type's own tp_methods so
        //     the methods become resolvable attributes. numpy's scalar types
        //     ship large method/getset tables and expect PyType_Ready to expose
        //     them; without a populated tp_dict, _PyType_Lookup finds nothing.
        if (*tp).tp_dict.is_null() {
            let dict = crate::api::mapping::PyDict_New();
            if dict.is_null() {
                crate::capi_trace::record_silent_failure(
                    "PyType_Ready",
                    Some("tp_dict allocation failed"),
                );
                return -1;
            }
            (*tp).tp_dict = dict;
        }
        if add_methods_to_dict(tp) < 0 {
            return -1;
        }

        // (3b) Populate tp_dict from tp_members and tp_getset — exactly the
        //      `type_add_members` / `type_add_getset` steps of CPython's
        //      `type_ready_fill_dict`. numpy's `PyUFunc_Type`, `PyArrayDescr_Type`,
        //      and `PyArray_Type` (all readied in `_multiarray_umath_exec` before
        //      the historical failure point) declare `tp_members`/`tp_getset`
        //      tables and trust PyType_Ready to expose them as
        //      member_descriptor / getset_descriptor entries. Each returns a real
        //      descriptor; a NULL from `PyDescr_New*` (or a dict-insert failure)
        //      propagates as a -1 with a set exception, matching CPython, so a
        //      genuine failure here is never a contentless exec-slot -1.
        if add_members_to_dict(tp) < 0 {
            return -1;
        }
        if add_getset_to_dict(tp) < 0 {
            return -1;
        }

        // (4) Compute tp_mro for single inheritance: [tp, ...base.tp_mro...].
        //     A null MRO makes attribute resolution and isinstance checks fail.
        if compute_single_inheritance_mro(tp) < 0 {
            return -1;
        }

        // (5) Set the metatype. CPython's `PyType_Ready` does
        //     `Py_SET_TYPE(type, &PyType_Type)` when the type's `ob_type` is left
        //     NULL by the static declaration (numpy's `PyVarObject_HEAD_INIT(NULL, 0)`
        //     leaves it NULL). Without this the readied type is a bare sentinel
        //     (`ob_type == NULL`) and every consumer that inspects
        //     `Py_TYPE(type)` — including the split-runtime bridge and the
        //     `describe_unresolved_pyobject` diagnostic — sees an ill-formed object.
        if (*tp).ob_base.ob_base.ob_type.is_null() {
            (*tp).ob_base.ob_base.ob_type = &raw mut crate::abi_types::PyType_Type;
        }

        // (5b) Guarantee the metatype can INSTANTIATE this type. `tp` is a type
        //      object; `Py_TYPE(tp)` is therefore its metatype, and in CPython
        //      every metatype is a subtype of `type` and so carries `type_call`
        //      as its `tp_call` (inherited during the metatype's own
        //      PyType_Ready). numpy's `_DTypeMeta` sets `tp_base = &PyType_Type`
        //      and relies on exactly that inheritance — but in the split wasm
        //      runtime a PIC extension's `&PyType_Type` DATA reference can resolve
        //      to an app-local unresolved GOT placeholder (an all-zero
        //      PyTypeObject: `tp_name`/`tp_call` NULL) rather than the runtime's
        //      canonical `PyType_Type`, so `_DTypeMeta` readies with a NULL
        //      `tp_call`. Then calling a DType class — `StringDType(...)`, whose
        //      metatype is `_DTypeMeta` — dispatches `Py_TYPE(cls)->tp_call`,
        //      finds NULL, and fails "'numpy._DTypeMeta' object is not callable"
        //      during `_multiarray_umath` init. Restore the slot the broken
        //      cross-module inheritance left NULL: this is byte-for-byte the value
        //      a faithful `inherit_slots(_DTypeMeta, &PyType_Type)` would have
        //      copied, so it is the correct metatype call slot, not a mask. Only a
        //      NULL slot is filled, so a metatype that overrides `__call__` keeps
        //      its own `tp_call`. `PyType_Type` itself (the common metatype)
        //      already carries `molt_type_call` from `init_static_types`, so this
        //      is a no-op for ordinary types.
        let meta = (*tp).ob_base.ob_base.ob_type;
        if !meta.is_null() && (*meta).tp_call.is_none() {
            (*meta).tp_call = Some(molt_type_call);
        }

        // (6) Mark ready.
        (*tp).tp_flags |= Py_TPFLAGS_READY;
    }

    // (7) Register the readied type object in the split-runtime object bridge so a
    //     C extension that hands the type back to the runtime — `PyModule_AddObject`
    //     (numpy's `ndarray`/`dtype`/`flatiter`/... module attributes) or a
    //     `PyDict_SetItem` whose key/value IS the type object (numpy's
    //     scalar-type -> DType registry) — resolves it via `pyobj_to_handle`
    //     instead of failing the bridge lookup. This is the same
    //     `register_raw_pyobj` bridging that `PyDescr_NewGetSet`/`PyDescr_NewMember`
    //     already apply to the descriptors they mint (idempotent + stable handle),
    //     not a weakening of the unresolved-object checks.
    unsafe {
        crate::bridge::GLOBAL_BRIDGE
            .lock()
            .register_raw_pyobj(tp.cast::<PyObject>());
        install_metatype_getattro(tp);
    }
    0
}

/// Populate `tp`'s `tp_dict` with a callable for each entry in its own
/// `tp_methods` table, keyed by method name. Mirrors the subset of CPython's
/// `add_methods` that static extension method tables depend on: each method
/// becomes a `builtin_function_or_method` bound to the type so `_PyType_Lookup`
/// (and therefore attribute access) resolves it. Returns 0 on success, -1 with
/// a recorded silent failure otherwise.
unsafe fn add_methods_to_dict(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let mut methods: *mut PyMethodDef = (*tp).tp_methods;
        if methods.is_null() {
            return 0;
        }
        let dict = (*tp).tp_dict;
        // Iterate until the sentinel entry (ml_name == NULL).
        while !(*methods).ml_name.is_null() {
            let name_ptr = (*methods).ml_name;
            // Skip entries without a callable (defensive; numpy tables are dense).
            if (*methods).ml_meth.is_none() {
                methods = methods.add(1);
                continue;
            }
            let func = crate::api::object::PyCFunction_NewEx(
                methods,
                tp.cast::<PyObject>(),
                ptr::null_mut(),
            );
            if func.is_null() {
                crate::capi_trace::record_silent_failure(
                    "PyType_Ready",
                    Some("PyCFunction_NewEx failed for method"),
                );
                return -1;
            }
            // Store the descriptor. A failure here means the runtime dict layer
            // could not hold the entry (e.g. an unresolved bridge handle); we
            // record it for diagnostics but do not abort readiness — the type is
            // structurally ready regardless of whether the store layer is fully
            // wired, and aborting would cascade a degraded dict backend into a
            // spurious extension exec failure.
            let rc = crate::api::mapping::PyDict_SetItemString(dict, name_ptr, func);
            crate::api::refcount::Py_DECREF(func);
            if rc < 0 {
                crate::capi_trace::record_silent_failure(
                    "PyType_Ready",
                    Some("PyDict_SetItemString could not store method descriptor"),
                );
            }
            methods = methods.add(1);
        }
        0
    }
}

/// Insert a descriptor into `tp_dict` keyed by its interned name, using
/// `PyDict_SetDefault` semantics (do not clobber a name already placed by an
/// earlier step — CPython's operators run first). Steals the caller's reference
/// to `descr` (decrefs it after insertion, exactly like CPython's loop bodies).
unsafe fn store_descr(dict: *mut PyObject, descr: *mut PyObject) -> c_int {
    unsafe {
        if dict.is_null() || std::ptr::eq(dict, &raw mut crate::abi_types::Py_None) {
            crate::api::refcount::Py_DECREF(descr);
            crate::capi_trace::record_silent_failure(
                "PyType_Ready",
                Some("tp_dict is not backed by runtime dict hooks"),
            );
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyType_Ready requires a real tp_dict for descriptors".as_ptr(),
            );
            return -1;
        }
        let name = PyDescr_NAME(descr);
        if name.is_null() {
            crate::api::refcount::Py_DECREF(descr);
            crate::capi_trace::record_silent_failure(
                "PyType_Ready",
                Some("descriptor has no name"),
            );
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"descriptor missing name during PyType_Ready".as_ptr(),
            );
            return -1;
        }
        crate::api::refcount::Py_INCREF(descr);
        let stored = crate::api::mapping::PyDict_SetDefault(dict, name, descr);
        let visible = if stored.is_null() {
            ptr::null_mut()
        } else {
            crate::api::mapping::PyDict_GetItem(dict, name)
        };
        let store_visible = !visible.is_null() && std::ptr::eq(visible, stored);
        let dict_retained_descriptor = store_visible && std::ptr::eq(stored, descr);
        crate::api::refcount::Py_DECREF(descr);
        if !dict_retained_descriptor {
            crate::api::refcount::Py_DECREF(descr);
        }
        if !store_visible {
            crate::capi_trace::record_silent_failure(
                "PyType_Ready",
                Some("PyDict_SetDefault did not make getset/member descriptor visible"),
            );
            if crate::api::errors::PyErr_Occurred().is_null() {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"PyType_Ready could not publish getset/member descriptor".as_ptr(),
                );
            }
            return -1;
        }
        0
    }
}

/// Populate `tp`'s `tp_dict` with a `member_descriptor` for each entry in its
/// own `tp_members` table. Mirrors CPython's `type_add_members`.
unsafe fn add_members_to_dict(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let mut memb = (*tp).tp_members.cast::<crate::abi_types::PyMemberDef>();
        if memb.is_null() {
            return 0;
        }
        let dict = (*tp).tp_dict;
        while !(*memb).name.is_null() {
            let descr = PyDescr_NewMember(tp, memb);
            if descr.is_null() {
                // PyDescr_NewMember recorded a silent failure; set an honest
                // exception if the alloc layer left none pending.
                if crate::api::errors::PyErr_Occurred().is_null() {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_SystemError,
                        c"PyDescr_NewMember returned NULL during PyType_Ready".as_ptr(),
                    );
                }
                return -1;
            }
            if store_descr(dict, descr) < 0 {
                return -1;
            }
            memb = memb.add(1);
        }
        0
    }
}

/// Populate `tp`'s `tp_dict` with a `getset_descriptor` for each entry in its
/// own `tp_getset` table. Mirrors CPython's `type_add_getset`.
unsafe fn add_getset_to_dict(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let mut gsp = (*tp).tp_getset.cast::<crate::abi_types::PyGetSetDef>();
        if gsp.is_null() {
            return 0;
        }
        let dict = (*tp).tp_dict;
        while !(*gsp).name.is_null() {
            let descr = PyDescr_NewGetSet(tp, gsp);
            if descr.is_null() {
                if crate::api::errors::PyErr_Occurred().is_null() {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_SystemError,
                        c"PyDescr_NewGetSet returned NULL during PyType_Ready".as_ptr(),
                    );
                }
                return -1;
            }
            if store_descr(dict, descr) < 0 {
                return -1;
            }
            gsp = gsp.add(1);
        }
        0
    }
}

/// Compute `tp_mro` for the single-inheritance chain rooted at `tp`:
/// `[tp, base, base.base, ..., object]`. This is the exact linearization
/// CPython produces for single inheritance, which covers every numpy scalar
/// type (they all use `SINGLE_INHERIT`/`DUAL_INHERIT` where the primary
/// `tp_base` chain drives resolution). Returns 0 on success, -1 otherwise.
unsafe fn compute_single_inheritance_mro(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        // Walk the base chain to collect [tp, base, ...]. Bounded and
        // cycle-guarded: a malformed extension with a self- or cyclic tp_base
        // must fail closed rather than hang the whole module exec.
        let mut chain: Vec<*mut PyTypeObject> = Vec::new();
        let mut cur = tp;
        while !cur.is_null() {
            if chain.contains(&cur) {
                // Cyclic base chain — refuse rather than loop forever.
                crate::capi_trace::record_silent_failure(
                    "PyType_Ready",
                    Some("cyclic tp_base chain"),
                );
                return -1;
            }
            chain.push(cur);
            let base = (*cur).tp_base;
            if base == cur {
                break; // object's tp_base may point to itself; stop.
            }
            cur = base;
        }
        let mro = crate::api::sequences::PyTuple_New(chain.len() as Py_ssize_t);
        if mro.is_null() {
            crate::capi_trace::record_silent_failure(
                "PyType_Ready",
                Some("tp_mro tuple allocation failed"),
            );
            return -1;
        }
        for (i, &entry) in chain.iter().enumerate() {
            let obj = entry.cast::<PyObject>();
            crate::api::refcount::Py_INCREF(obj);
            // PyTuple_SetItem steals the reference we just added.
            crate::api::sequences::PyTuple_SetItem(mro, i as Py_ssize_t, obj);
        }
        (*tp).tp_mro = mro;
        0
    }
}

/// Copy the base type's slots into `tp` wherever `tp` has left them empty,
/// mirroring the subset of CPython's `inherit_slots` that static C-extension
/// type hierarchies depend on. Only null/zero child slots are filled, so a type
/// that defines its own slot keeps it.
unsafe fn inherit_slots_from_base(tp: *mut PyTypeObject, base: *mut PyTypeObject) {
    unsafe {
        // Sizing: a derived type that did not declare its own instance layout
        // uses the base's.
        if (*tp).tp_basicsize == 0 {
            (*tp).tp_basicsize = (*base).tp_basicsize;
        }
        if (*tp).tp_itemsize == 0 {
            (*tp).tp_itemsize = (*base).tp_itemsize;
        }
        if (*tp).tp_dictoffset == 0 {
            (*tp).tp_dictoffset = (*base).tp_dictoffset;
        }
        if (*tp).tp_weaklistoffset == 0 {
            (*tp).tp_weaklistoffset = (*base).tp_weaklistoffset;
        }

        // Function-pointer slots: inherit when the child left them None.
        macro_rules! inherit_fn {
            ($field:ident) => {
                if (*tp).$field.is_none() {
                    (*tp).$field = (*base).$field;
                }
            };
        }
        inherit_fn!(tp_dealloc);
        inherit_fn!(tp_getattr);
        inherit_fn!(tp_setattr);
        inherit_fn!(tp_repr);
        inherit_fn!(tp_hash);
        inherit_fn!(tp_call);
        inherit_fn!(tp_str);
        inherit_fn!(tp_getattro);
        inherit_fn!(tp_setattro);
        inherit_fn!(tp_traverse);
        inherit_fn!(tp_clear);
        inherit_fn!(tp_richcompare);
        inherit_fn!(tp_iter);
        inherit_fn!(tp_iternext);
        inherit_fn!(tp_descr_get);
        inherit_fn!(tp_descr_set);
        inherit_fn!(tp_init);
        inherit_fn!(tp_alloc);
        inherit_fn!(tp_new);
        inherit_fn!(tp_free);
        inherit_fn!(tp_is_gc);
        inherit_fn!(tp_del);
        inherit_fn!(tp_finalize);

        // Raw-pointer sub-protocol tables: inherit when the child left them null.
        macro_rules! inherit_ptr {
            ($field:ident) => {
                if (*tp).$field.is_null() {
                    (*tp).$field = (*base).$field;
                }
            };
        }
        inherit_ptr!(tp_as_async);
        inherit_ptr!(tp_as_number);
        inherit_ptr!(tp_as_sequence);
        inherit_ptr!(tp_as_mapping);
        inherit_ptr!(tp_as_buffer);
        inherit_ptr!(tp_methods);
        inherit_ptr!(tp_members);
        inherit_ptr!(tp_getset);
    }
}

/// CPython 3.12 `type_call` — the `tp_call` slot of `PyType_Type`. Verified
/// verbatim against the primary source (python/cpython v3.12.13
/// `Objects/typeobject.c::type_call`): the `type(x)` one-argument special case
/// (only for `type` itself, #27157), the "type() takes 1 or 3 arguments"
/// error, the NULL-`tp_new` "cannot create '%s' instances" error, the
/// `tp_new` → `PyObject_TypeCheck` → `tp_init` flow with `res < 0` dropping
/// the fresh instance, and `_Py_CheckFunctionResult`'s fail-closed contract
/// (NULL without an exception ⇒ SystemError; a result with an exception
/// pending ⇒ SystemError) instead of CPython's debug-only asserts.
///
/// Installed on `PyType_Type` by `init_static_types`, so every C-extension
/// metatype that sets `tp_base = &PyType_Type` and relies on `PyType_Ready`
/// slot inheritance (numpy's `PyArrayDTypeMeta_Type` is the canonical case —
/// calling a DType class like `BoolDType()` dispatches
/// `Py_TYPE(cls)->tp_call`, i.e. `type.tp_call`) can instantiate its
/// instances. Molt-compiled classes are NOT affected: their bridge proxies
/// carry `ob_type == PyBaseObject_Type` (see `bridge::tag_to_type`), so
/// `PyObject_Call` still routes them through the runtime call authority.
pub unsafe extern "C" fn molt_type_call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> *mut PyObject {
    let tp = callable.cast::<PyTypeObject>();
    if tp.is_null() {
        return ptr::null_mut();
    }
    let type_type = &raw mut crate::abi_types::PyType_Type;
    unsafe {
        // Special case: type(x) should return Py_TYPE(x). Only `type` itself
        // accepts the one-argument form (#27157).
        if ptr::eq(tp, type_type) {
            let nargs = if args.is_null() {
                0
            } else {
                crate::api::sequences::PyTuple_Size(args)
            };
            let kwds_empty = kwds.is_null() || crate::api::mapping::PyDict_Size(kwds) == 0;
            if nargs == 1 && kwds_empty {
                let item = crate::api::sequences::PyTuple_GetItem(args, 0);
                if item.is_null() {
                    return ptr::null_mut();
                }
                let item_type = (*item).ob_type.cast::<PyObject>();
                crate::api::refcount::Py_INCREF(item_type);
                return item_type;
            }
            if nargs != 3 {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_TypeError,
                    c"type() takes 1 or 3 arguments".as_ptr(),
                );
                return ptr::null_mut();
            }
        }

        let Some(tp_new) = (*tp).tp_new else {
            let name = if (*tp).tp_name.is_null() {
                "<anonymous>".to_string()
            } else {
                std::ffi::CStr::from_ptr((*tp).tp_name)
                    .to_string_lossy()
                    .into_owned()
            };
            crate::capi_trace::record_silent_failure("type_call", Some(&name));
            if let Ok(msg) = std::ffi::CString::new(format!("cannot create '{name}' instances")) {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_TypeError,
                    msg.as_ptr(),
                );
            }
            return ptr::null_mut();
        };

        // Env-gated diagnostic (MOLT_TRACE_CAPI): name the type being
        // instantiated, its `tp_new` slot pointer, and whether the call arrived
        // with a NULL args pointer. This is the probe that pins split-runtime
        // DType-instantiation failures (numpy `use_new_as_default`) to a
        // concrete DType + slot. Zero cost when the env var is unset.
        if crate::capi_trace::trace_enabled() {
            let name = if (*tp).tp_name.is_null() {
                "<anonymous>".to_string()
            } else {
                std::ffi::CStr::from_ptr((*tp).tp_name)
                    .to_string_lossy()
                    .into_owned()
            };
            crate::capi_trace::trace_call(
                "molt_type_call:new",
                Some(&format!(
                    "{name} tp_new={:p} args_null={} kwds_null={}",
                    tp_new as *const (),
                    args.is_null(),
                    kwds.is_null()
                )),
            );
        }

        let obj = tp_new(tp, args, kwds);

        if crate::capi_trace::trace_enabled() {
            let result_type = if obj.is_null() {
                "NULL".to_string()
            } else if (*obj).ob_type.is_null() || (*(*obj).ob_type).tp_name.is_null() {
                "<anonymous-result>".to_string()
            } else {
                std::ffi::CStr::from_ptr((*(*obj).ob_type).tp_name)
                    .to_string_lossy()
                    .into_owned()
            };
            crate::capi_trace::trace_call(
                "molt_type_call:result",
                Some(&format!("-> {result_type}")),
            );
        }
        // _Py_CheckFunctionResult: fail closed on a contract violation rather
        // than silently propagating a bare NULL / stale exception.
        if obj.is_null() {
            if crate::api::errors::PyErr_Occurred().is_null() {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"tp_new returned NULL without setting an exception".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        if !crate::api::errors::PyErr_Occurred().is_null() {
            crate::api::refcount::Py_DECREF(obj);
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"tp_new returned a result with an exception set".as_ptr(),
            );
            return ptr::null_mut();
        }

        // If the returned object is not an instance of the called type, it
        // won't be initialized.
        if PyObject_TypeCheck(obj, tp) == 0 {
            return obj;
        }

        let instance_type = (*obj).ob_type;
        if !instance_type.is_null()
            && let Some(tp_init) = (*instance_type).tp_init
        {
            let res = tp_init(obj, args, kwds);
            if res < 0 {
                if crate::api::errors::PyErr_Occurred().is_null() {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_SystemError,
                        c"tp_init failed without setting an exception".as_ptr(),
                    );
                }
                crate::api::refcount::Py_DECREF(obj);
                return ptr::null_mut();
            }
        }
        obj
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GenericAlloc(
    tp: *mut PyTypeObject,
    nitems: Py_ssize_t,
) -> *mut PyObject {
    if tp.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::memory::molt_object_alloc(tp, nitems) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GenericNew(
    tp: *mut PyTypeObject,
    _args: *mut PyObject,
    _kwds: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyType_GenericAlloc(tp, 0) }
}

/// CPython 3.12 `Include/typeslots.h` slot ids. Verified against the primary
/// source (python/cpython @ 3.12). Each id maps a `PyType_Slot.slot` value to a
/// destination field so `PyType_FromSpecWithBases` can install every slot a
/// static/heap C extension (numpy/scipy) declares. Qualified-path patterns
/// (`ts::Py_tp_new`) are matched as constants, never bindings.
#[allow(non_upper_case_globals)]
mod ts {
    use std::os::raw::c_int;
    // Buffer protocol.
    pub const Py_bf_getbuffer: c_int = 1;
    pub const Py_bf_releasebuffer: c_int = 2;
    // Mapping protocol.
    pub const Py_mp_ass_subscript: c_int = 3;
    pub const Py_mp_length: c_int = 4;
    pub const Py_mp_subscript: c_int = 5;
    // Number protocol.
    pub const Py_nb_absolute: c_int = 6;
    pub const Py_nb_add: c_int = 7;
    pub const Py_nb_and: c_int = 8;
    pub const Py_nb_bool: c_int = 9;
    pub const Py_nb_divmod: c_int = 10;
    pub const Py_nb_float: c_int = 11;
    pub const Py_nb_floor_divide: c_int = 12;
    pub const Py_nb_index: c_int = 13;
    pub const Py_nb_inplace_add: c_int = 14;
    pub const Py_nb_inplace_and: c_int = 15;
    pub const Py_nb_inplace_floor_divide: c_int = 16;
    pub const Py_nb_inplace_lshift: c_int = 17;
    pub const Py_nb_inplace_multiply: c_int = 18;
    pub const Py_nb_inplace_or: c_int = 19;
    pub const Py_nb_inplace_power: c_int = 20;
    pub const Py_nb_inplace_remainder: c_int = 21;
    pub const Py_nb_inplace_rshift: c_int = 22;
    pub const Py_nb_inplace_subtract: c_int = 23;
    pub const Py_nb_inplace_true_divide: c_int = 24;
    pub const Py_nb_inplace_xor: c_int = 25;
    pub const Py_nb_int: c_int = 26;
    pub const Py_nb_invert: c_int = 27;
    pub const Py_nb_lshift: c_int = 28;
    pub const Py_nb_multiply: c_int = 29;
    pub const Py_nb_negative: c_int = 30;
    pub const Py_nb_or: c_int = 31;
    pub const Py_nb_positive: c_int = 32;
    pub const Py_nb_power: c_int = 33;
    pub const Py_nb_remainder: c_int = 34;
    pub const Py_nb_rshift: c_int = 35;
    pub const Py_nb_subtract: c_int = 36;
    pub const Py_nb_true_divide: c_int = 37;
    pub const Py_nb_xor: c_int = 38;
    // Sequence protocol.
    pub const Py_sq_ass_item: c_int = 39;
    pub const Py_sq_concat: c_int = 40;
    pub const Py_sq_contains: c_int = 41;
    pub const Py_sq_inplace_concat: c_int = 42;
    pub const Py_sq_inplace_repeat: c_int = 43;
    pub const Py_sq_item: c_int = 44;
    pub const Py_sq_length: c_int = 45;
    pub const Py_sq_repeat: c_int = 46;
    // Type slots.
    pub const Py_tp_alloc: c_int = 47;
    pub const Py_tp_base: c_int = 48;
    pub const Py_tp_bases: c_int = 49;
    pub const Py_tp_call: c_int = 50;
    pub const Py_tp_clear: c_int = 51;
    pub const Py_tp_dealloc: c_int = 52;
    pub const Py_tp_del: c_int = 53;
    pub const Py_tp_descr_get: c_int = 54;
    pub const Py_tp_descr_set: c_int = 55;
    pub const Py_tp_doc: c_int = 56;
    pub const Py_tp_getattr: c_int = 57;
    pub const Py_tp_getattro: c_int = 58;
    pub const Py_tp_hash: c_int = 59;
    pub const Py_tp_init: c_int = 60;
    pub const Py_tp_is_gc: c_int = 61;
    pub const Py_tp_iter: c_int = 62;
    pub const Py_tp_iternext: c_int = 63;
    pub const Py_tp_methods: c_int = 64;
    pub const Py_tp_new: c_int = 65;
    pub const Py_tp_repr: c_int = 66;
    pub const Py_tp_richcompare: c_int = 67;
    pub const Py_tp_setattr: c_int = 68;
    pub const Py_tp_setattro: c_int = 69;
    pub const Py_tp_str: c_int = 70;
    pub const Py_tp_traverse: c_int = 71;
    pub const Py_tp_members: c_int = 72;
    pub const Py_tp_getset: c_int = 73;
    pub const Py_tp_free: c_int = 74;
    // Number protocol (matrix multiply, added later in the id space).
    pub const Py_nb_matrix_multiply: c_int = 75;
    pub const Py_nb_inplace_matrix_multiply: c_int = 76;
    // Async protocol.
    pub const Py_am_await: c_int = 77;
    pub const Py_am_aiter: c_int = 78;
    pub const Py_am_anext: c_int = 79;
    pub const Py_tp_finalize: c_int = 80;
    pub const Py_am_send: c_int = 81;
}

/// Get-or-allocate the `tp_as_number` sub-table, zero-initialised.
unsafe fn ensure_number(ty: *mut PyTypeObject) -> *mut crate::abi_types::PyNumberMethods {
    unsafe {
        if (*ty).tp_as_number.is_null() {
            let b: Box<crate::abi_types::PyNumberMethods> = Box::new(std::mem::zeroed());
            (*ty).tp_as_number = Box::into_raw(b).cast::<c_void>();
        }
        (*ty)
            .tp_as_number
            .cast::<crate::abi_types::PyNumberMethods>()
    }
}

/// Get-or-allocate the `tp_as_sequence` sub-table, zero-initialised.
unsafe fn ensure_sequence(ty: *mut PyTypeObject) -> *mut crate::abi_types::PySequenceMethods {
    unsafe {
        if (*ty).tp_as_sequence.is_null() {
            let b: Box<crate::abi_types::PySequenceMethods> = Box::new(std::mem::zeroed());
            (*ty).tp_as_sequence = Box::into_raw(b).cast::<c_void>();
        }
        (*ty)
            .tp_as_sequence
            .cast::<crate::abi_types::PySequenceMethods>()
    }
}

/// Get-or-allocate the `tp_as_mapping` sub-table, zero-initialised.
unsafe fn ensure_mapping(ty: *mut PyTypeObject) -> *mut crate::abi_types::PyMappingMethods {
    unsafe {
        if (*ty).tp_as_mapping.is_null() {
            let b: Box<crate::abi_types::PyMappingMethods> = Box::new(std::mem::zeroed());
            (*ty).tp_as_mapping = Box::into_raw(b).cast::<c_void>();
        }
        (*ty)
            .tp_as_mapping
            .cast::<crate::abi_types::PyMappingMethods>()
    }
}

/// Get-or-allocate the `tp_as_async` sub-table, zero-initialised.
unsafe fn ensure_async(ty: *mut PyTypeObject) -> *mut crate::abi_types::PyAsyncMethods {
    unsafe {
        if (*ty).tp_as_async.is_null() {
            let b: Box<crate::abi_types::PyAsyncMethods> = Box::new(std::mem::zeroed());
            (*ty).tp_as_async = Box::into_raw(b).cast::<c_void>();
        }
        (*ty).tp_as_async.cast::<crate::abi_types::PyAsyncMethods>()
    }
}

/// Get-or-allocate the `tp_as_buffer` sub-table, zero-initialised.
unsafe fn ensure_buffer(ty: *mut PyTypeObject) -> *mut crate::abi_types::PyBufferProcs {
    unsafe {
        if (*ty).tp_as_buffer.is_null() {
            let b: Box<crate::abi_types::PyBufferProcs> = Box::new(std::mem::zeroed());
            (*ty).tp_as_buffer = Box::into_raw(b).cast::<c_void>();
        }
        (*ty).tp_as_buffer.cast::<crate::abi_types::PyBufferProcs>()
    }
}

/// Apply every entry of a `PyType_Spec.slots` array (terminated by `slot == 0`)
/// to the corresponding field of the type under construction. Mirrors the slot
/// dispatch of CPython 3.12's `PyType_FromMetaclass` (`Objects/typeobject.c`):
/// each `Py_tp_*` id targets a `tp_*` field, each `Py_nb_*/sq_*/mp_*/am_*/bf_*`
/// id targets a lazily-allocated protocol sub-table, and `Py_tp_doc` copies the
/// documentation string into freshly allocated memory. An unrecognised slot id
/// fails closed with a set exception (CPython raises `RuntimeError: invalid slot
/// offset`) rather than silently dropping behaviour. Returns 0 on success, -1
/// with a recorded silent failure + pending exception otherwise.
unsafe fn apply_spec_slots(
    ty: *mut PyTypeObject,
    slots: *mut crate::abi_types::PyType_Slot,
) -> c_int {
    if slots.is_null() {
        return 0;
    }
    unsafe {
        let mut slot = slots;
        while (*slot).slot != 0 {
            let id = (*slot).slot;
            let pfunc = (*slot).pfunc;
            // tp_* function-pointer fields are `Option<extern "C" fn(...)>`, which
            // is pointer-sized and null-niche-optimised, so a transmute from the
            // raw `pfunc` yields `None` for a null pointer and `Some(fn)` otherwise.
            macro_rules! set_fn {
                ($field:ident) => {{
                    (*ty).$field = ::std::mem::transmute(pfunc);
                }};
            }
            match id {
                // ── tp_* slots ────────────────────────────────────────────────
                ts::Py_tp_alloc => set_fn!(tp_alloc),
                ts::Py_tp_base => (*ty).tp_base = pfunc.cast::<PyTypeObject>(),
                ts::Py_tp_bases => (*ty).tp_bases = pfunc.cast::<PyObject>(),
                ts::Py_tp_call => set_fn!(tp_call),
                ts::Py_tp_clear => set_fn!(tp_clear),
                ts::Py_tp_dealloc => set_fn!(tp_dealloc),
                ts::Py_tp_del => set_fn!(tp_del),
                ts::Py_tp_descr_get => set_fn!(tp_descr_get),
                ts::Py_tp_descr_set => set_fn!(tp_descr_set),
                ts::Py_tp_doc => {
                    // CPython copies the doc string into fresh storage owned by the
                    // type (the caller's static string need not outlive the spec).
                    if !pfunc.is_null() {
                        let src = pfunc.cast::<c_char>();
                        let bytes = std::ffi::CStr::from_ptr(src).to_bytes_with_nul();
                        let buf = crate::api::memory::PyMem_Malloc(bytes.len()).cast::<c_char>();
                        if buf.is_null() {
                            crate::capi_trace::record_silent_failure(
                                "PyType_FromSpec",
                                Some("tp_doc allocation failed"),
                            );
                            crate::api::errors::PyErr_NoMemory();
                            return -1;
                        }
                        ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), bytes.len());
                        (*ty).tp_doc = buf;
                    }
                }
                ts::Py_tp_getattr => set_fn!(tp_getattr),
                ts::Py_tp_getattro => set_fn!(tp_getattro),
                ts::Py_tp_hash => set_fn!(tp_hash),
                ts::Py_tp_init => set_fn!(tp_init),
                ts::Py_tp_is_gc => set_fn!(tp_is_gc),
                ts::Py_tp_iter => set_fn!(tp_iter),
                ts::Py_tp_iternext => set_fn!(tp_iternext),
                ts::Py_tp_methods => (*ty).tp_methods = pfunc.cast::<PyMethodDef>(),
                ts::Py_tp_new => set_fn!(tp_new),
                ts::Py_tp_repr => set_fn!(tp_repr),
                ts::Py_tp_richcompare => set_fn!(tp_richcompare),
                ts::Py_tp_setattr => set_fn!(tp_setattr),
                ts::Py_tp_setattro => set_fn!(tp_setattro),
                ts::Py_tp_str => set_fn!(tp_str),
                ts::Py_tp_traverse => set_fn!(tp_traverse),
                ts::Py_tp_members => (*ty).tp_members = pfunc,
                ts::Py_tp_getset => (*ty).tp_getset = pfunc,
                ts::Py_tp_free => set_fn!(tp_free),
                ts::Py_tp_finalize => set_fn!(tp_finalize),
                // ── nb_* (number) slots ───────────────────────────────────────
                ts::Py_nb_absolute => (*ensure_number(ty)).nb_absolute = pfunc,
                ts::Py_nb_add => (*ensure_number(ty)).nb_add = pfunc,
                ts::Py_nb_and => (*ensure_number(ty)).nb_and = pfunc,
                ts::Py_nb_bool => (*ensure_number(ty)).nb_bool = pfunc,
                ts::Py_nb_divmod => (*ensure_number(ty)).nb_divmod = pfunc,
                ts::Py_nb_float => (*ensure_number(ty)).nb_float = pfunc,
                ts::Py_nb_floor_divide => (*ensure_number(ty)).nb_floor_divide = pfunc,
                ts::Py_nb_index => (*ensure_number(ty)).nb_index = pfunc,
                ts::Py_nb_inplace_add => (*ensure_number(ty)).nb_inplace_add = pfunc,
                ts::Py_nb_inplace_and => (*ensure_number(ty)).nb_inplace_and = pfunc,
                ts::Py_nb_inplace_floor_divide => {
                    (*ensure_number(ty)).nb_inplace_floor_divide = pfunc
                }
                ts::Py_nb_inplace_lshift => (*ensure_number(ty)).nb_inplace_lshift = pfunc,
                ts::Py_nb_inplace_multiply => (*ensure_number(ty)).nb_inplace_multiply = pfunc,
                ts::Py_nb_inplace_or => (*ensure_number(ty)).nb_inplace_or = pfunc,
                ts::Py_nb_inplace_power => (*ensure_number(ty)).nb_inplace_power = pfunc,
                ts::Py_nb_inplace_remainder => (*ensure_number(ty)).nb_inplace_remainder = pfunc,
                ts::Py_nb_inplace_rshift => (*ensure_number(ty)).nb_inplace_rshift = pfunc,
                ts::Py_nb_inplace_subtract => (*ensure_number(ty)).nb_inplace_subtract = pfunc,
                ts::Py_nb_inplace_true_divide => {
                    (*ensure_number(ty)).nb_inplace_true_divide = pfunc
                }
                ts::Py_nb_inplace_xor => (*ensure_number(ty)).nb_inplace_xor = pfunc,
                ts::Py_nb_int => (*ensure_number(ty)).nb_int = pfunc,
                ts::Py_nb_invert => (*ensure_number(ty)).nb_invert = pfunc,
                ts::Py_nb_lshift => (*ensure_number(ty)).nb_lshift = pfunc,
                ts::Py_nb_multiply => (*ensure_number(ty)).nb_multiply = pfunc,
                ts::Py_nb_negative => (*ensure_number(ty)).nb_negative = pfunc,
                ts::Py_nb_or => (*ensure_number(ty)).nb_or = pfunc,
                ts::Py_nb_positive => (*ensure_number(ty)).nb_positive = pfunc,
                ts::Py_nb_power => (*ensure_number(ty)).nb_power = pfunc,
                ts::Py_nb_remainder => (*ensure_number(ty)).nb_remainder = pfunc,
                ts::Py_nb_rshift => (*ensure_number(ty)).nb_rshift = pfunc,
                ts::Py_nb_subtract => (*ensure_number(ty)).nb_subtract = pfunc,
                ts::Py_nb_true_divide => (*ensure_number(ty)).nb_true_divide = pfunc,
                ts::Py_nb_xor => (*ensure_number(ty)).nb_xor = pfunc,
                ts::Py_nb_matrix_multiply => (*ensure_number(ty)).nb_matrix_multiply = pfunc,
                ts::Py_nb_inplace_matrix_multiply => {
                    (*ensure_number(ty)).nb_inplace_matrix_multiply = pfunc
                }
                // ── sq_* (sequence) slots ─────────────────────────────────────
                ts::Py_sq_length => (*ensure_sequence(ty)).sq_length = pfunc,
                ts::Py_sq_concat => (*ensure_sequence(ty)).sq_concat = pfunc,
                ts::Py_sq_repeat => (*ensure_sequence(ty)).sq_repeat = pfunc,
                ts::Py_sq_item => (*ensure_sequence(ty)).sq_item = pfunc,
                ts::Py_sq_ass_item => (*ensure_sequence(ty)).sq_ass_item = pfunc,
                ts::Py_sq_contains => (*ensure_sequence(ty)).sq_contains = pfunc,
                ts::Py_sq_inplace_concat => (*ensure_sequence(ty)).sq_inplace_concat = pfunc,
                ts::Py_sq_inplace_repeat => (*ensure_sequence(ty)).sq_inplace_repeat = pfunc,
                // ── mp_* (mapping) slots ──────────────────────────────────────
                ts::Py_mp_length => (*ensure_mapping(ty)).mp_length = pfunc,
                ts::Py_mp_subscript => (*ensure_mapping(ty)).mp_subscript = pfunc,
                ts::Py_mp_ass_subscript => (*ensure_mapping(ty)).mp_ass_subscript = pfunc,
                // ── am_* (async) slots ────────────────────────────────────────
                ts::Py_am_await => (*ensure_async(ty)).am_await = pfunc,
                ts::Py_am_aiter => (*ensure_async(ty)).am_aiter = pfunc,
                ts::Py_am_anext => (*ensure_async(ty)).am_anext = pfunc,
                ts::Py_am_send => (*ensure_async(ty)).am_send = pfunc,
                // ── bf_* (buffer) slots ───────────────────────────────────────
                ts::Py_bf_getbuffer => (*ensure_buffer(ty)).bf_getbuffer = pfunc,
                ts::Py_bf_releasebuffer => (*ensure_buffer(ty)).bf_releasebuffer = pfunc,
                // Unrecognised slot id — fail closed (poison contract): a silently
                // dropped slot is a new miscompile. CPython raises RuntimeError.
                other => {
                    crate::capi_trace::record_silent_failure(
                        "PyType_FromSpec",
                        Some(&format!("unknown PyType_Slot id {other}")),
                    );
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_RuntimeError,
                        c"PyType_FromSpec: invalid slot offset".as_ptr(),
                    );
                    return -1;
                }
            }
            slot = slot.add(1);
        }
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromSpecWithBases(
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    if spec.is_null() {
        return ptr::null_mut();
    }
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    unsafe {
        ty.ob_base.ob_base.ob_refcnt = 1;
        ty.ob_base.ob_base.ob_type = &raw mut crate::abi_types::PyType_Type;
        ty.ob_base.ob_size = 0;
        ty.tp_name = (*spec).name;
        ty.tp_basicsize = (*spec).basicsize as Py_ssize_t;
        ty.tp_itemsize = (*spec).itemsize as Py_ssize_t;
        // Carry the caller's flags but do NOT pre-mark READY — PyType_Ready must
        // run its full inherit/dict/mro pipeline below, and it early-returns if
        // READY is already set.
        ty.tp_flags = (*spec).flags as std::os::raw::c_ulong & !Py_TPFLAGS_READY;
        let tp = Box::into_raw(ty);

        // (1) Apply every spec slot to its destination field/sub-table. A bad
        //     slot id fails closed with a pending exception.
        if apply_spec_slots(tp, (*spec).slots) < 0 {
            // Leave the pending exception; the type is unusable. (The partial
            // allocation is intentionally not reclaimed — a slot error aborts
            // module init, matching CPython's fail-closed behaviour.)
            return ptr::null_mut();
        }

        // (2) Resolve the base. A Py_tp_base slot wins; otherwise derive from the
        //     explicit `bases` tuple (first entry — the single-inheritance case
        //     numpy/scipy use); otherwise PyType_Ready defaults it to `object`.
        if !bases.is_null() {
            (*tp).tp_bases = bases;
            if (*tp).tp_base.is_null() && crate::api::sequences::PyTuple_GET_SIZE(bases) >= 1 {
                let first = crate::api::sequences::PyTuple_GetItem(bases, 0);
                if !first.is_null() {
                    (*tp).tp_base = first.cast::<PyTypeObject>();
                }
            }
        }

        // (3) Instantiation defaults: object (PyBaseObject_Type) ships no tp_new/
        //     tp_alloc, so nothing is inherited for them. Provide the generic
        //     allocator/constructor only where the spec left them unset, so a
        //     spec that supplies its own Py_tp_new/Py_tp_alloc keeps it.
        if (*tp).tp_alloc.is_none() {
            (*tp).tp_alloc = Some(PyType_GenericAlloc);
        }
        if (*tp).tp_new.is_none() {
            (*tp).tp_new = Some(PyType_GenericNew);
        }

        // (4) Run the comprehensive readiness pipeline: default tp_base, inherit
        //     unset slots from the base, build tp_dict and install
        //     tp_methods/tp_members/tp_getset, compute the MRO, mark READY.
        if PyType_Ready(tp) < 0 {
            return ptr::null_mut();
        }
        tp.cast::<PyObject>()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromModuleAndSpec(
    _module: *mut PyObject,
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyType_FromSpecWithBases(spec, bases) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromMetaclass(
    _metaclass: *mut PyTypeObject,
    module: *mut PyObject,
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyType_FromModuleAndSpec(module, spec, bases) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let type_type = &raw mut crate::abi_types::PyType_Type;
    if std::ptr::eq(op, type_type.cast::<PyObject>()) {
        return 1;
    }
    std::ptr::eq(unsafe { (*op).ob_type }, type_type) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Modified(_tp: *mut PyTypeObject) {}

/// Resolve `name` on `tp` by walking its MRO and returning the first matching
/// `tp_dict` entry, mirroring CPython's `_PyType_Lookup`. Returns a *borrowed*
/// reference (no incref), matching the CPython contract. Static extensions rely
/// on this for method resolution — a stub that returns NULL silently breaks
/// every inherited attribute lookup on numpy's scalar types.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyType_Lookup(
    tp: *mut PyTypeObject,
    name: *mut PyObject,
) -> *mut PyObject {
    if tp.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let mro = (*tp).tp_mro;
        if !mro.is_null() {
            let n = crate::api::sequences::PyTuple_Size(mro);
            let mut i: Py_ssize_t = 0;
            while i < n {
                let base = crate::api::sequences::PyTuple_GetItem(mro, i).cast::<PyTypeObject>();
                if !base.is_null() {
                    let dict = (*base).tp_dict;
                    if !dict.is_null() {
                        let found = crate::api::mapping::PyDict_GetItem(dict, name);
                        if !found.is_null() {
                            return found;
                        }
                    }
                }
                i += 1;
            }
            return ptr::null_mut();
        }
        // No MRO computed (type not readied): fall back to a direct base-chain
        // walk so lookups still resolve.
        let mut cur = tp;
        while !cur.is_null() {
            let dict = (*cur).tp_dict;
            if !dict.is_null() {
                let found = crate::api::mapping::PyDict_GetItem(dict, name);
                if !found.is_null() {
                    return found;
                }
            }
            let base = (*cur).tp_base;
            if base == cur {
                break;
            }
            cur = base;
        }
        ptr::null_mut()
    }
}

/// `PyDescr_IsData` — a descriptor is a *data* descriptor iff its type defines
/// `tp_descr_set`. CPython keys this purely on `Py_TYPE(descr)->tp_descr_set !=
/// NULL` (not on whether the individual `PyGetSetDef` has a setter), and both
/// `PyGetSetDescr_Type` and `PyMemberDescr_Type` install a `tp_descr_set` (which
/// itself raises `AttributeError` for a read-only entry). Attribute-resolution
/// order — a data descriptor on the type wins over the instance dict — depends on
/// an honest answer here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_IsData(descr: *mut PyObject) -> c_int {
    if descr.is_null() {
        return 0;
    }
    let tp = unsafe { (*descr).ob_type };
    if tp.is_null() {
        return 0;
    }
    unsafe { (*tp).tp_descr_set }.is_some() as c_int
}

/// `PyDescr_NAME(descr)` — the interned attribute name of any descriptor. All
/// descriptor objects share the `PyDescrObject` header, so this reads
/// `d_common.d_name` (a borrowed reference, matching CPython's macro).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NAME(descr: *mut PyObject) -> *mut PyObject {
    if descr.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*descr.cast::<crate::abi_types::PyDescrObject>()).d_name }
}

/// Allocate the shared descriptor header for a `type`/`name` pair. Mirrors
/// CPython's `descr_new`: interns the name, takes an owned reference to `type`.
/// Returns a boxed, ABI-owned descriptor with refcount 1, or NULL (with an
/// exception set) on allocation failure.
unsafe fn descr_alloc(
    descr_type: *mut PyTypeObject,
    for_type: *mut PyTypeObject,
    name: *const c_char,
) -> *mut crate::abi_types::PyDescrObject {
    unsafe {
        let name_obj = if name.is_null() {
            crate::api::strings::PyUnicode_FromString(c"".as_ptr())
        } else {
            crate::api::strings::PyUnicode_InternFromString(name)
        };
        if name_obj.is_null() {
            // PyUnicode_* sets MemoryError on failure; record for the exec-slot
            // diagnostic in case a stubbed string layer returned NULL silently.
            crate::capi_trace::record_silent_failure(
                "PyDescr_New",
                Some("descriptor name allocation failed"),
            );
            return ptr::null_mut();
        }
        crate::api::refcount::Py_INCREF(for_type.cast::<PyObject>());
        let descr = Box::new(crate::abi_types::PyDescrObject {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: descr_type,
            },
            d_type: for_type,
            d_name: name_obj,
            d_qualname: ptr::null_mut(),
        });
        Box::into_raw(descr)
    }
}

/// Create a `getset_descriptor` for `getset` bound to `type`. Faithful to
/// CPython `PyDescr_NewGetSet` (`Objects/descrobject.c`): the descriptor stores
/// a *borrowed* pointer to the caller's `PyGetSetDef` (which must outlive the
/// type), so a static numpy `tp_getset` table becomes real, resolvable
/// `getset_descriptor` attributes in `tp_dict`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NewGetSet(
    type_: *mut PyTypeObject,
    getset: *mut crate::abi_types::PyGetSetDef,
) -> *mut PyObject {
    if type_.is_null() || getset.is_null() {
        crate::capi_trace::record_silent_failure("PyDescr_NewGetSet", Some("null type or getset"));
        return ptr::null_mut();
    }
    let name = unsafe { (*getset).name };
    let common = unsafe { descr_alloc(&raw mut crate::abi_types::PyGetSetDescr_Type, type_, name) };
    if common.is_null() {
        return ptr::null_mut();
    }
    // Widen the shared header allocation to the getset descriptor. `descr_alloc`
    // boxed a bare `PyDescrObject`; reallocate as the wider struct so the
    // `d_getset` tail is owned. Simpler and leak-free: box the full struct here
    // and copy the header out, then free the header box.
    unsafe {
        let header = *Box::from_raw(common);
        let descr = Box::new(crate::abi_types::PyGetSetDescrObject {
            d_common: header,
            d_getset: getset,
        });
        let ptr = Box::into_raw(descr).cast::<PyObject>();
        crate::bridge::GLOBAL_BRIDGE.lock().register_raw_pyobj(ptr);
        ptr
    }
}

/// Create a `member_descriptor` for `member` bound to `type`. Faithful to
/// CPython `PyDescr_NewMember`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NewMember(
    type_: *mut PyTypeObject,
    member: *mut crate::abi_types::PyMemberDef,
) -> *mut PyObject {
    if type_.is_null() || member.is_null() {
        crate::capi_trace::record_silent_failure("PyDescr_NewMember", Some("null type or member"));
        return ptr::null_mut();
    }
    let name = unsafe { (*member).name };
    let common = unsafe { descr_alloc(&raw mut crate::abi_types::PyMemberDescr_Type, type_, name) };
    if common.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let header = *Box::from_raw(common);
        let descr = Box::new(crate::abi_types::PyMemberDescrObject {
            d_common: header,
            d_member: member,
        });
        let ptr = Box::into_raw(descr).cast::<PyObject>();
        crate::bridge::GLOBAL_BRIDGE.lock().register_raw_pyobj(ptr);
        ptr
    }
}

// ─── Descriptor protocol (tp_descr_get / tp_descr_set) ─────────────────────
//
// `getset_descriptor` and `member_descriptor` are the objects `PyType_Ready`
// stores in `tp_dict` for a type's `tp_getset` / `tp_members` tables. When an
// attribute lookup finds one of these in the type's dict, the runtime invokes
// its `tp_descr_get` (read) or `tp_descr_set` (write). Faithful to
// CPython `Objects/descrobject.c` (`getset_get`/`getset_set`/`member_get`/
// `member_set`).

/// `tp_descr_get` for `getset_descriptor`. `obj == NULL` (attribute accessed on
/// the type itself) returns the descriptor; otherwise it invokes the underlying
/// getter with the `closure`, or raises `AttributeError` for a write-only entry.
unsafe extern "C" fn getset_get(
    descr: *mut PyObject,
    obj: *mut PyObject,
    _type: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if obj.is_null() {
            crate::api::refcount::Py_INCREF(descr);
            return descr;
        }
        let d = descr.cast::<crate::abi_types::PyGetSetDescrObject>();
        let getset = (*d).d_getset;
        if getset.is_null() {
            return ptr::null_mut();
        }
        match (*getset).get {
            Some(get) => get(obj, (*getset).closure),
            None => {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_AttributeError,
                    c"unreadable attribute".as_ptr(),
                );
                ptr::null_mut()
            }
        }
    }
}

/// `tp_descr_set` for `getset_descriptor`. Invokes the underlying setter with
/// the `closure`, or raises `AttributeError` for a read-only entry.
unsafe extern "C" fn getset_set(
    descr: *mut PyObject,
    obj: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        let d = descr.cast::<crate::abi_types::PyGetSetDescrObject>();
        let getset = (*d).d_getset;
        if getset.is_null() {
            return -1;
        }
        match (*getset).set {
            Some(set) => set(obj, value, (*getset).closure),
            None => {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_AttributeError,
                    c"readonly attribute".as_ptr(),
                );
                -1
            }
        }
    }
}

/// `tp_descr_get` for `member_descriptor`. Reads the struct member at
/// `d_member->offset` off `obj` using `PyMember_GetOne`.
unsafe extern "C" fn member_get(
    descr: *mut PyObject,
    obj: *mut PyObject,
    _type: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if obj.is_null() {
            crate::api::refcount::Py_INCREF(descr);
            return descr;
        }
        let d = descr.cast::<crate::abi_types::PyMemberDescrObject>();
        let member = (*d).d_member;
        if member.is_null() {
            return ptr::null_mut();
        }
        PyMember_GetOne(obj.cast::<c_char>(), member)
    }
}

/// `tp_descr_set` for `member_descriptor`.
unsafe extern "C" fn member_set(
    descr: *mut PyObject,
    obj: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    unsafe {
        let d = descr.cast::<crate::abi_types::PyMemberDescrObject>();
        let member = (*d).d_member;
        if member.is_null() {
            return -1;
        }
        PyMember_SetOne(obj.cast::<c_char>(), member, value)
    }
}

unsafe fn descr_common_decref(common: &mut crate::abi_types::PyDescrObject) {
    unsafe {
        crate::api::refcount::Py_XDECREF(common.d_type.cast::<PyObject>());
        crate::api::refcount::Py_XDECREF(common.d_name);
        crate::api::refcount::Py_XDECREF(common.d_qualname);
    }
}

unsafe extern "C" fn getset_descr_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe {
        let descr = op.cast::<crate::abi_types::PyGetSetDescrObject>();
        descr_common_decref(&mut (*descr).d_common);
        drop(Box::from_raw(descr));
    }
}

unsafe extern "C" fn member_descr_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe {
        let descr = op.cast::<crate::abi_types::PyMemberDescrObject>();
        descr_common_decref(&mut (*descr).d_common);
        drop(Box::from_raw(descr));
    }
}

/// Install the descriptor protocol slots on the two descriptor type objects.
/// Called once at ABI init (after `init_static_types`) so that a
/// `getset_descriptor` / `member_descriptor` found in a type's `tp_dict`
/// resolves through `tp_descr_get` / `tp_descr_set` exactly as CPython wires
/// `PyGetSetDescr_Type` / `PyMemberDescr_Type`.
///
/// # Safety
/// Single-threaded init only; must run before any C extension attribute access.
pub unsafe fn init_descriptor_slots() {
    unsafe {
        let gs = &raw mut crate::abi_types::PyGetSetDescr_Type;
        (*gs).tp_descr_get = Some(getset_get);
        (*gs).tp_descr_set = Some(getset_set);
        (*gs).tp_dealloc = Some(getset_descr_dealloc);
        (*gs).tp_basicsize =
            std::mem::size_of::<crate::abi_types::PyGetSetDescrObject>() as Py_ssize_t;

        let mem = &raw mut crate::abi_types::PyMemberDescr_Type;
        (*mem).tp_descr_get = Some(member_get);
        (*mem).tp_descr_set = Some(member_set);
        (*mem).tp_dealloc = Some(member_descr_dealloc);
        (*mem).tp_basicsize =
            std::mem::size_of::<crate::abi_types::PyMemberDescrObject>() as Py_ssize_t;
    }
}

// Member type codes (CPython `Include/descrobject.h`, `Py_T_*`).
const PY_T_SHORT: c_int = 0;
const PY_T_INT: c_int = 1;
const PY_T_LONG: c_int = 2;
const PY_T_FLOAT: c_int = 3;
const PY_T_DOUBLE: c_int = 4;
const PY_T_STRING: c_int = 5;
const PY_T_OBJECT: c_int = 6;
const PY_T_CHAR: c_int = 7;
const PY_T_BYTE: c_int = 8;
const PY_T_UBYTE: c_int = 9;
const PY_T_USHORT: c_int = 10;
const PY_T_UINT: c_int = 11;
const PY_T_ULONG: c_int = 12;
const PY_T_BOOL: c_int = 14;
const PY_T_OBJECT_EX: c_int = 16;
const PY_T_LONGLONG: c_int = 17;
const PY_T_ULONGLONG: c_int = 18;
const PY_T_PYSSIZET: c_int = 19;
const PY_T_NONE: c_int = 20;
const PY_READONLY: c_int = 1;

/// `PyMember_GetOne` — read one struct member into a Python object. Faithful to
/// CPython `Python/structmember.c`. `addr` is the base address of the containing
/// object; the member lives at `addr + member->offset` with the C type given by
/// `member->type`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMember_GetOne(
    addr: *const c_char,
    member: *mut crate::abi_types::PyMemberDef,
) -> *mut PyObject {
    if addr.is_null() || member.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let field = addr.offset((*member).offset) as *const c_void;
        // Width-correct constructors: `c_long` is 32-bit on Windows MSVC, so
        // 64-bit members must route through the LongLong/Ssize_t constructors to
        // avoid silent truncation. C `long`/`unsigned long` map to Ssize_t/Size_t
        // which are pointer-width and cover both LP64 and LLP64 hosts.
        match (*member).type_ {
            PY_T_SHORT => crate::api::numbers::PyLong_FromSsize_t(*(field as *const i16) as isize),
            PY_T_INT => crate::api::numbers::PyLong_FromSsize_t(*(field as *const i32) as isize),
            PY_T_LONG => crate::api::numbers::PyLong_FromLongLong(
                *(field as *const std::os::raw::c_long) as c_longlong,
            ),
            PY_T_FLOAT => crate::api::numbers::PyFloat_FromDouble(*(field as *const f32) as f64),
            // `field = addr + offset` is only guaranteed aligned to the C
            // object's struct alignment, which on wasm32 is 4 for a statically
            // declared object. An 8-byte `f64` read there would be a misaligned
            // dereference (UB; caught by the debug alignment check), so read it
            // unaligned. (4-byte members above are always ≥4-aligned = safe.)
            PY_T_DOUBLE => crate::api::numbers::PyFloat_FromDouble(std::ptr::read_unaligned(
                field as *const f64,
            )),
            PY_T_BOOL => {
                let b = *(field as *const i8) != 0;
                let obj = if b {
                    &raw mut crate::abi_types::Py_True
                } else {
                    &raw mut crate::abi_types::Py_False
                };
                crate::api::refcount::Py_INCREF(obj);
                obj
            }
            PY_T_BYTE => crate::api::numbers::PyLong_FromSsize_t(*(field as *const i8) as isize),
            PY_T_UBYTE => crate::api::numbers::PyLong_FromSize_t(*(field as *const u8) as usize),
            PY_T_USHORT => crate::api::numbers::PyLong_FromSize_t(*(field as *const u16) as usize),
            PY_T_UINT => crate::api::numbers::PyLong_FromSize_t(*(field as *const u32) as usize),
            PY_T_ULONG => crate::api::numbers::PyLong_FromUnsignedLongLong(
                *(field as *const std::os::raw::c_ulong) as c_ulonglong,
            ),
            // 8-byte members: `field` may be only 4-aligned (see PY_T_DOUBLE) —
            // read unaligned to avoid a misaligned dereference on wasm32.
            PY_T_LONGLONG => crate::api::numbers::PyLong_FromLongLong(std::ptr::read_unaligned(
                field as *const c_longlong,
            )),
            PY_T_ULONGLONG => crate::api::numbers::PyLong_FromUnsignedLongLong(
                std::ptr::read_unaligned(field as *const c_ulonglong),
            ),
            PY_T_PYSSIZET => crate::api::numbers::PyLong_FromSsize_t(*(field as *const isize)),
            PY_T_CHAR => {
                let c = *(field as *const c_char);
                let buf = [c as u8, 0u8];
                crate::api::strings::PyUnicode_FromStringAndSize(buf.as_ptr().cast(), 1)
            }
            PY_T_STRING => {
                let s = *(field as *const *const c_char);
                if s.is_null() {
                    let none = &raw mut crate::abi_types::Py_None;
                    crate::api::refcount::Py_INCREF(none);
                    none
                } else {
                    crate::api::strings::PyUnicode_FromString(s)
                }
            }
            PY_T_OBJECT | PY_T_OBJECT_EX => {
                let v = *(field as *const *mut PyObject);
                if v.is_null() {
                    if (*member).type_ == PY_T_OBJECT_EX {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_AttributeError,
                            (*member).name,
                        );
                        return ptr::null_mut();
                    }
                    let none = &raw mut crate::abi_types::Py_None;
                    crate::api::refcount::Py_INCREF(none);
                    none
                } else {
                    crate::api::refcount::Py_INCREF(v);
                    v
                }
            }
            PY_T_NONE => {
                let none = &raw mut crate::abi_types::Py_None;
                crate::api::refcount::Py_INCREF(none);
                none
            }
            _ => {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"bad member type in PyMember_GetOne".as_ptr(),
                );
                ptr::null_mut()
            }
        }
    }
}

/// `PyMember_SetOne` — write one struct member from a Python object. Faithful to
/// CPython `Python/structmember.c`, covering the mutable subset numpy uses (it
/// declares nearly all members `READONLY`). Read-only / audit-only members and
/// unsupported writes fail closed with an honest exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMember_SetOne(
    addr: *mut c_char,
    member: *mut crate::abi_types::PyMemberDef,
    value: *mut PyObject,
) -> c_int {
    if addr.is_null() || member.is_null() {
        return -1;
    }
    unsafe {
        if (*member).flags & PY_READONLY != 0 {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_AttributeError,
                c"readonly attribute".as_ptr(),
            );
            return -1;
        }
        // T_OBJECT/T_OBJECT_EX are the writable members ordinary extensions use;
        // deleting (value == NULL) is only valid for T_OBJECT_EX.
        match (*member).type_ {
            PY_T_OBJECT | PY_T_OBJECT_EX => {
                let slot = addr.offset((*member).offset) as *mut *mut PyObject;
                let old = *slot;
                if !value.is_null() {
                    crate::api::refcount::Py_INCREF(value);
                }
                *slot = value;
                if !old.is_null() {
                    crate::api::refcount::Py_DECREF(old);
                }
                0
            }
            _ => {
                // Numeric/char member writes are rare for extension init; fail
                // closed with a precise diagnostic rather than a silent no-op.
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"cannot set non-object member in PyMember_SetOne".as_ptr(),
                );
                -1
            }
        }
    }
}

/// Py_TYPE(op) — return ob_type pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_TYPE(op: *mut PyObject) -> *mut PyTypeObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*op).ob_type }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Type(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyObject_Type called with NULL".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"object has NULL type".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let type_obj = tp.cast::<PyObject>();
    unsafe { crate::api::refcount::Py_INCREF(type_obj) };
    type_obj
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_TypeCheck(op: *mut PyObject, tp: *mut PyTypeObject) -> c_int {
    if op.is_null() || tp.is_null() {
        return 0;
    }
    // CPython's `PyObject_TypeCheck` (Include/object.h) is
    //   `Py_IS_TYPE(ob, tp) || PyType_IsSubtype(Py_TYPE(ob), tp)`
    // — an EXACT-type match OR a subtype relationship. Molt previously answered
    // only the exact match, so any C extension that type-checks an instance
    // against a BASE type failed closed. numpy's `PyArray_DescrCheck(res)` is
    // `PyObject_TypeCheck(res, &PyArrayDescr_Type)`: a DType descriptor's
    // `Py_TYPE` is its concrete DType class (e.g. `StringDType`, whose
    // `tp_base == &PyArrayDescr_Type`), never `PyArrayDescr_Type` itself, so the
    // exact-only check rejected every genuine descriptor and stranded
    // `use_new_as_default` (dtypemeta.c) with "did not return a dtype instance".
    // Walk the subtype chain exactly as CPython does.
    let actual = unsafe { (*op).ob_type };
    if std::ptr::eq(actual, tp) {
        return 1;
    }
    unsafe { PyType_IsSubtype(actual, tp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_IsInstance(inst: *mut PyObject, cls: *mut PyObject) -> c_int {
    if inst.is_null() || cls.is_null() {
        return 0;
    }
    // When `cls` is a type object, CPython's `PyObject_IsInstance` reduces to
    // `PyObject_TypeCheck(inst, (PyTypeObject *)cls)` (Objects/abstract.c ->
    // `recursive_isinstance`). That is exactly the C-extension case (e.g.
    // numpy `descriptor.c` does `PyObject_IsInstance(conv, &PyArray_StringDType)`),
    // so answer it with the same exact-OR-subtype walk `PyObject_TypeCheck`
    // now performs. `PyObject_TypeCheck` only POINTER-compares `cls`
    // (it dereferences `inst`'s type, never `cls`), so a non-type `cls` — the
    // `__instancecheck__` / tuple-of-classes cases Molt cannot resolve here —
    // safely yields the same conservative `0` (not-an-instance) as before,
    // never a false positive.
    unsafe { PyObject_TypeCheck(inst, cls.cast::<PyTypeObject>()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCallable_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    // Check if the object's type has tp_call set — the CPython definition of
    // "callable".  Without tp_call we cannot determine callability from the
    // bridge alone, but checking it is strictly better than always returning 0,
    // which caused extensions to wrongly reject callable objects.
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        return 0;
    }
    if unsafe { (*tp).tp_call }.is_some() {
        return 1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Hash(op: *mut PyObject) -> isize {
    if op.is_null() {
        return -1;
    }
    // Try tp_hash first.
    let tp = unsafe { (*op).ob_type };
    if !tp.is_null()
        && let Some(hash_fn) = unsafe { (*tp).tp_hash }
    {
        return unsafe { hash_fn(op) };
    }
    op as isize // pointer-based hash as last resort
}

// ─── PyType subtype / flags / name ────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_IsSubtype(a: *mut PyTypeObject, b: *mut PyTypeObject) -> c_int {
    if a.is_null() || b.is_null() {
        return 0;
    }
    if std::ptr::eq(a, b) {
        return 1;
    }
    // Walk tp_base chain.
    let mut cursor = a;
    while !cursor.is_null() {
        if std::ptr::eq(cursor, b) {
            return 1;
        }
        cursor = unsafe { (*cursor).tp_base };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetFlags(tp: *mut PyTypeObject) -> std::os::raw::c_ulong {
    if tp.is_null() {
        return 0;
    }
    unsafe { (*tp).tp_flags }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetName(tp: *mut PyTypeObject) -> *mut PyObject {
    if tp.is_null() {
        return ptr::null_mut();
    }
    let name_ptr = unsafe { (*tp).tp_name };
    if name_ptr.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::strings::PyUnicode_FromString(name_ptr) }
}

/// Read a `str` attribute-name `PyObject` into an owned `String`, or `None`.
fn attr_name_utf8(name: *mut PyObject) -> Option<String> {
    let mut size: Py_ssize_t = 0;
    let ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8AndSize(name, &raw mut size) };
    if ptr.is_null() || size < 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, size as usize) };
    std::str::from_utf8(bytes).ok().map(|s| s.to_string())
}

/// `tp_getattro` for `PyType_Type` — inherited by every metaclass that leaves
/// its own slot null (numpy's `_DTypeMeta`, whose `tp_base` is `type`). CPython
/// exposes `type.__name__` / `type.__qualname__` as getset descriptors in
/// `type`'s dict, backed by the `PyTypeObject` fields; our static `PyType_Type`
/// carries no populated dict, so those well-known attributes are answered here
/// straight from `tp_name` — exactly what CPython's getters read. This is not a
/// per-attribute fake: it is the genuine `type` attribute semantics, and it lets
/// `DType.__name__` inside numpy's `numpy.dtypes._add_dtype_helper` resolve once
/// the DType crosses into Molt as a foreign wrapper. Every other attribute
/// delegates to generic resolution (the type's own dict + MRO, populated by
/// `PyType_Ready` from `tp_methods`/`tp_getset`, e.g. numpy's `_abstract`).
unsafe extern "C" fn type_getattro(o: *mut PyObject, name: *mut PyObject) -> *mut PyObject {
    if o.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    if let Some(attr) = attr_name_utf8(name) {
        // `__name__` / `__qualname__` are data descriptors on the metatype in
        // CPython, so they take priority over the type's own dict. Resolve them
        // from `tp_name`, stripping any module/qualifier prefix (the part up to
        // and including the last '.') — exactly what CPython's `type.__name__`
        // getter does, so numpy's dotted `numpy.dtypes.BoolDType` reports
        // `BoolDType` (the key `_add_dtype_helper` stores in `numpy.dtypes`).
        if attr == "__name__" || attr == "__qualname__" {
            let tp = o.cast::<PyTypeObject>();
            let name_ptr = unsafe { (*tp).tp_name };
            if !name_ptr.is_null() {
                let bytes = unsafe { std::ffi::CStr::from_ptr(name_ptr) }.to_bytes();
                let short = match bytes.iter().rposition(|&b| b == b'.') {
                    Some(dot) => &bytes[dot + 1..],
                    None => bytes,
                };
                return unsafe {
                    crate::api::strings::PyUnicode_FromStringAndSize(
                        short.as_ptr().cast::<std::os::raw::c_char>(),
                        short.len() as Py_ssize_t,
                    )
                };
            }
        }
    }
    unsafe { crate::api::object::PyObject_GenericGetAttr(o, name) }
}

/// Install `type_getattro` on `PyType_Type` so metaclasses inherit it. Called
/// from `init_static_types` after the static type table is zero-initialized.
///
/// # Safety
/// Must be called during single-threaded ABI initialization.
pub unsafe fn init_type_getattro() {
    unsafe {
        crate::abi_types::PyType_Type.tp_getattro = Some(type_getattro);
    }
}


/// Ensure the *metatype* of a just-readied type `tp` carries a `tp_getattro`.
///
/// In CPython every metaclass inherits `type.__getattribute__` (our
/// `type_getattro`) from `type`; a static extension's metaclass (numpy's
/// `_DTypeMeta`) should get it when `PyType_Ready` runs. But in the
/// split-runtime the extension's `&PyType_Type` can retarget to a copy of
/// `type` that never received our `type_getattro`, so `inherit_slots` copies a
/// null slot and the metaclass is left with no getattro — making
/// `DType.__name__` (numpy's `numpy.dtypes._add_dtype_helper`) fail to resolve.
/// `tp`'s metatype is the object that answers attribute access for `tp` and
/// every sibling instance of that metaclass, so installing our `type_getattro`
/// here (only when the slot is null — never overriding a metatype's own) makes
/// `Type.__name__` / `__qualname__` resolve for every type of that metaclass,
/// from Molt (via foreign-object custody) and from C alike.
///
/// # Safety
/// `tp` must be a readied `PyTypeObject` (its `ob_type` is set).
pub(crate) unsafe fn install_metatype_getattro(tp: *mut PyTypeObject) {
    if tp.is_null() {
        return;
    }
    let metatype = unsafe { (*tp).ob_base.ob_base.ob_type };
    if metatype.is_null() || std::ptr::eq(metatype, tp) {
        return;
    }
    if unsafe { (*metatype).tp_getattro }.is_none() {
        unsafe { (*metatype).tp_getattro = Some(type_getattro) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetQualName(tp: *mut PyTypeObject) -> *mut PyObject {
    // For our purposes, qualname == name.
    unsafe { PyType_GetName(tp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_HasFeature(
    tp: *mut PyTypeObject,
    feature: std::os::raw::c_ulong,
) -> c_int {
    if tp.is_null() {
        return 0;
    }
    (unsafe { (*tp).tp_flags } & feature != 0) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Repr(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::strings::PyUnicode_FromString(c"<molt object>".as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Str(op: *mut PyObject) -> *mut PyObject {
    unsafe { PyObject_Repr(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_RichCompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    // Try tp_richcompare on v's type first, then w's type (reflected).
    if !v.is_null() {
        let tp = unsafe { (*v).ob_type };
        if !tp.is_null()
            && let Some(richcmp) = unsafe { (*tp).tp_richcompare }
        {
            let result = unsafe { richcmp(v, w, op) };
            if !result.is_null()
                && !std::ptr::eq(result, &raw mut crate::abi_types::Py_NotImplementedSentinel)
            {
                return result;
            }
        }
    }
    // Return NotImplemented sentinel — callers must check for this.
    &raw mut crate::abi_types::Py_NotImplementedSentinel
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_RichCompareBool(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> c_int {
    let result = unsafe { PyObject_RichCompare(v, w, op) };
    if result.is_null() {
        return -1;
    }
    if std::ptr::eq(result, &raw mut crate::abi_types::Py_NotImplementedSentinel) {
        // Comparison not supported — for Py_EQ/Py_NE fall back to pointer
        // identity (CPython semantics for unsupported comparisons).
        const PY_EQ: c_int = 2;
        const PY_NE: c_int = 3;
        return match op {
            PY_EQ => std::ptr::eq(v, w) as c_int,
            PY_NE => !std::ptr::eq(v, w) as c_int,
            _ => -1, // cannot compare: error
        };
    }
    // Truthy check: Py_True → 1, Py_False → 0, Py_None → 0
    if std::ptr::eq(result, &raw mut crate::abi_types::Py_True) {
        1
    } else if std::ptr::eq(result, &raw mut crate::abi_types::Py_False)
        || std::ptr::eq(result, &raw mut crate::abi_types::Py_None)
    {
        0
    } else {
        // Non-null, non-sentinel result — treat as truthy.
        1
    }
}
