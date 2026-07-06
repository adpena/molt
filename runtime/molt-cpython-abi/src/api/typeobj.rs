//! Type object API — PyType_Ready, PyType_GenericAlloc, Py_TYPE checks.

use crate::abi_types::{
    Py_TPFLAGS_READY, Py_ssize_t, PyMethodDef, PyObject, PyType_Spec, PyTypeObject,
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
    // calls, or a re-entrant static-init) must not be re-processed.
    if unsafe { (*tp).tp_flags } & Py_TPFLAGS_READY != 0 {
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

        // (5) Mark ready.
        (*tp).tp_flags |= Py_TPFLAGS_READY;
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
        ty.tp_flags = (*spec).flags as std::os::raw::c_ulong | Py_TPFLAGS_READY;
        ty.tp_base = &raw mut crate::abi_types::PyBaseObject_Type;
        ty.tp_bases = bases;
        ty.tp_alloc = Some(PyType_GenericAlloc);
        ty.tp_new = Some(PyType_GenericNew);
    }
    Box::into_raw(ty).cast::<PyObject>()
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
        (*gs).tp_basicsize =
            std::mem::size_of::<crate::abi_types::PyGetSetDescrObject>() as Py_ssize_t;

        let mem = &raw mut crate::abi_types::PyMemberDescr_Type;
        (*mem).tp_descr_get = Some(member_get);
        (*mem).tp_descr_set = Some(member_set);
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
            PY_T_DOUBLE => crate::api::numbers::PyFloat_FromDouble(*(field as *const f64)),
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
            PY_T_LONGLONG => {
                crate::api::numbers::PyLong_FromLongLong(*(field as *const c_longlong))
            }
            PY_T_ULONGLONG => {
                crate::api::numbers::PyLong_FromUnsignedLongLong(*(field as *const c_ulonglong))
            }
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
    let actual = unsafe { (*op).ob_type };
    std::ptr::eq(actual, tp) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_IsInstance(inst: *mut PyObject, cls: *mut PyObject) -> c_int {
    if inst.is_null() || cls.is_null() {
        return 0;
    }
    // Check whether inst's type pointer matches cls (exact type match).
    // This does not walk the MRO — full isinstance() requires the Molt runtime.
    // Returning -1 (error) would be worse than a conservative match, so we
    // check the one thing we *can* check: pointer identity of ob_type.
    let inst_type = unsafe { (*inst).ob_type };
    if inst_type.is_null() {
        return 0;
    }
    if std::ptr::eq(inst_type as *const PyObject, cls) {
        return 1;
    }
    // Cannot determine — return 0 (not an instance) rather than lying.
    // Extensions that hit this path get a false negative, which is safer than
    // a false positive.  Log via bridge tracing if available.
    0
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
