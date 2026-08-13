//! Type object API — PyType_Ready, PyType_GenericAlloc, Py_TYPE checks.

use crate::abi_types::{
    Py_TPFLAGS_HAVE_GC, Py_TPFLAGS_HEAPTYPE, Py_TPFLAGS_READY, Py_ssize_t, PyHeapTypeObject,
    PyMethodDef, PyObject, PyType_Spec, PyTypeObject,
};
use crate::bridge::GLOBAL_BRIDGE;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_longlong, c_ulong, c_ulonglong};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static ABI_LOCAL_TYPES: Lazy<Mutex<HashMap<u32, usize>>> = Lazy::new(|| Mutex::new(HashMap::new()));
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TypeIdentity {
    address: usize,
    generation: u64,
}

#[derive(Default)]
struct SubclassIdentities {
    order: Vec<TypeIdentity>,
    members: HashSet<TypeIdentity>,
}

#[derive(Default)]
struct TypeSubclassRegistry {
    live: HashMap<usize, u64>,
    subclasses: HashMap<TypeIdentity, SubclassIdentities>,
    bases_by_subclass: HashMap<TypeIdentity, HashSet<TypeIdentity>>,
}

static TYPE_SUBCLASSES: Lazy<Mutex<TypeSubclassRegistry>> =
    Lazy::new(|| Mutex::new(TypeSubclassRegistry::default()));
static NEXT_TYPE_IDENTITY_GENERATION: AtomicU64 = AtomicU64::new(1);
type PyTypeWatchCallback = unsafe extern "C" fn(*mut PyObject) -> c_int;
const TYPE_MAX_WATCHERS: usize = 8;
const CANONICAL_INTERPRETER_ID: i64 = 0;
struct TypeWatcherState {
    interpreter_id: i64,
    callbacks: [Option<PyTypeWatchCallback>; TYPE_MAX_WATCHERS],
}

// Molt exposes one canonical interpreter (ID 0), with no subinterpreter
// creation surface. The mutex is the free-threaded watcher-state boundary.
static TYPE_WATCHER_STATE: Lazy<Mutex<TypeWatcherState>> = Lazy::new(|| {
    Mutex::new(TypeWatcherState {
        interpreter_id: CANONICAL_INTERPRETER_ID,
        callbacks: [None; TYPE_MAX_WATCHERS],
    })
});
static NEXT_TYPE_VERSION_TAG: AtomicU32 = AtomicU32::new(1);

fn type_identity(
    registry: &mut TypeSubclassRegistry,
    tp: *mut PyTypeObject,
) -> Option<TypeIdentity> {
    if tp.is_null() {
        return None;
    }
    let address = tp.addr();
    let generation = if let Some(generation) = registry.live.get(&address) {
        *generation
    } else {
        let generation = NEXT_TYPE_IDENTITY_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()?;
        registry.live.insert(address, generation);
        generation
    };
    Some(TypeIdentity {
        address,
        generation,
    })
}

unsafe fn register_subclass(base: *mut PyTypeObject, subclass: *mut PyTypeObject) {
    if base.is_null() || subclass.is_null() || ptr::eq(base, subclass) {
        return;
    }
    let mut registry = TYPE_SUBCLASSES.lock();
    let Some(base_identity) = type_identity(&mut registry, base) else {
        return;
    };
    let Some(subclass_identity) = type_identity(&mut registry, subclass) else {
        return;
    };
    let subclasses = registry.subclasses.entry(base_identity).or_default();
    if subclasses.members.insert(subclass_identity) {
        subclasses.order.push(subclass_identity);
        registry
            .bases_by_subclass
            .entry(subclass_identity)
            .or_default()
            .insert(base_identity);
    }
}

/// Remove a non-owning type identity before its allocation is returned.
///
/// Every object-domain free routes here. Non-type addresses are absent and
/// cost one map probe; heap types lose both outgoing and incoming subclass
/// edges before address reuse can create a new generation.
pub(crate) fn unregister_type_address(address: usize) {
    if address == 0 {
        return;
    }
    let mut registry = TYPE_SUBCLASSES.lock();
    let Some(generation) = registry.live.remove(&address) else {
        return;
    };
    let identity = TypeIdentity {
        address,
        generation,
    };
    if let Some(children) = registry.subclasses.remove(&identity) {
        for child in children.order {
            if let Some(bases) = registry.bases_by_subclass.get_mut(&child) {
                bases.remove(&identity);
                if bases.is_empty() {
                    registry.bases_by_subclass.remove(&child);
                }
            }
        }
    }
    if let Some(bases) = registry.bases_by_subclass.remove(&identity) {
        for base in bases {
            if let Some(children) = registry.subclasses.get_mut(&base) {
                children.members.remove(&identity);
                // Keep the ordered slot as a tombstone. Repeated sibling
                // teardown is O(1) per base edge; the next deterministic
                // traversal compacts all dead slots in one linear pass.
            }
        }
    }
}

unsafe fn register_type_subclasses(tp: *mut PyTypeObject) {
    let bases = unsafe { (*tp).tp_bases };
    if !bases.is_null() {
        let count = unsafe { crate::api::sequences::PyTuple_Size(bases) };
        if count >= 0 {
            for index in 0..count {
                let base = unsafe { crate::api::sequences::PyTuple_GetItem(bases, index) }
                    .cast::<PyTypeObject>();
                unsafe { register_subclass(base, tp) };
            }
            return;
        }
    }
    unsafe { register_subclass((*tp).tp_base, tp) };
}

unsafe fn reject_type_layout(message: &'static std::ffi::CStr) -> c_int {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
            message.as_ptr(),
        )
    };
    -1
}

unsafe fn validate_base_layout(tp: *mut PyTypeObject, base: *mut PyTypeObject) -> c_int {
    if tp.is_null() || base.is_null() {
        return 0;
    }
    unsafe {
        if (*base).tp_flags & crate::abi_types::Py_TPFLAGS_BASETYPE == 0 {
            return reject_type_layout(c"type is not an acceptable base type");
        }
        if (*tp).tp_basicsize != 0 && (*tp).tp_basicsize < (*base).tp_basicsize {
            return reject_type_layout(c"type basicsize is smaller than its base layout");
        }
        if (*tp).tp_itemsize != 0
            && (*base).tp_itemsize != 0
            && (*tp).tp_itemsize != (*base).tp_itemsize
        {
            return reject_type_layout(c"type itemsize is incompatible with its base layout");
        }
    }
    0
}

/// Select one truthful physical base.  Incomparable C layouts are rejected;
/// managed runtime multiple inheritance is answered by the runtime
/// `type_is_subtype` hook instead of fabricating a single `tp_base` chain.
unsafe fn acceptable_best_base(bases: *mut PyObject) -> *mut PyTypeObject {
    if bases.is_null() {
        return ptr::null_mut();
    }
    let count = unsafe { crate::api::sequences::PyTuple_Size(bases) };
    if count < 0 {
        return ptr::null_mut();
    }
    let mut best: *mut PyTypeObject = ptr::null_mut();
    for index in 0..count {
        let candidate = unsafe { crate::api::sequences::PyTuple_GetItem(bases, index) };
        if candidate.is_null() || unsafe { PyType_Check(candidate) } == 0 {
            unsafe { reject_type_layout(c"bases must contain only type objects") };
            return ptr::null_mut();
        }
        let candidate = candidate.cast::<PyTypeObject>();
        if unsafe { (*candidate).tp_flags } & crate::abi_types::Py_TPFLAGS_BASETYPE == 0 {
            unsafe { reject_type_layout(c"type is not an acceptable base type") };
            return ptr::null_mut();
        }
        if best.is_null() || unsafe { PyType_IsSubtype(candidate, best) } != 0 {
            best = candidate;
        } else if unsafe { PyType_IsSubtype(best, candidate) } == 0 {
            unsafe { reject_type_layout(c"multiple bases have incompatible physical layouts") };
            return ptr::null_mut();
        }
    }
    best
}

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
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
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
            register_type_subclasses(tp);
            crate::bridge::GLOBAL_BRIDGE.register_foreign_pyobj(tp.cast::<PyObject>());
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
        if add_operators_to_dict(tp) < 0 {
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

        // (6) Register every direct base before publishing READY. This is the
        // recursive invalidation graph consumed by PyType_Modified; keeping it
        // as non-owning raw identities mirrors CPython's weakref subclass dict
        // without adding a strong type cycle to the GC graph.
        register_type_subclasses(tp);

        // (7) Mark ready.
        (*tp).tp_flags |= Py_TPFLAGS_READY;
    }

    // (7) Register the readied type object in the split-runtime object bridge so a
    //     C extension that hands the type back to the runtime — `PyModule_AddObject`
    //     (numpy's `ndarray`/`dtype`/`flatiter`/... module attributes) or a
    //     `PyDict_SetItem` whose key/value IS the type object (numpy's
    //     scalar-type -> DType registry) — resolves it via `pyobj_to_handle`
    //     instead of failing the bridge lookup. This is the same
    //     canonical bridge registration that `PyDescr_NewGetSet`/`PyDescr_NewMember`
    //     already apply to the descriptors they mint (idempotent + stable handle),
    //     not a weakening of the unresolved-object checks.
    unsafe {
        crate::bridge::GLOBAL_BRIDGE.register_foreign_pyobj(tp.cast::<PyObject>());
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
            // could not hold the entry (e.g. an unresolved bridge handle). CPython's
            // add_methods (Objects/typeobject.c) propagates a PyDict store failure
            // as -1, so PyType_Ready FAILS — it never marks a type ready with a
            // silently-dropped method (which would surface much later as an
            // AttributeError / wrong dispatch on the missing method). Match that:
            // fail CLOSED with a set exception, mirroring the add_members_/
            // add_getset_ (store_descr) siblings.
            let rc = crate::api::mapping::PyDict_SetItemString(dict, name_ptr, func);
            crate::api::refcount::Py_DECREF(func);
            if rc < 0 {
                crate::capi_trace::record_silent_failure(
                    "PyType_Ready",
                    Some("PyDict_SetItemString could not store method descriptor"),
                );
                // Guarantee a pending exception even if the dict layer returned
                // -1 without setting one (record-without-exception class), so the
                // caller's `< 0` check never sees a contentless failure.
                if crate::api::errors::PyErr_Occurred().is_null() {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"PyType_Ready could not store method descriptor in tp_dict".as_ptr(),
                    );
                }
                return -1;
            }
            methods = methods.add(1);
        }
        0
    }
}

#[derive(Clone, Copy)]
enum SlotWrapper {
    Direct(DirectSlot),
    Number(NumberSlot),
    Sequence(SequenceSlot),
    Mapping(MappingSlot),
    Async(AsyncSlot),
    Buffer(BufferSlot),
}

#[derive(Clone, Copy)]
enum DirectSlot {
    Alloc,
    Base,
    Bases,
    Repr,
    Hash,
    Call,
    Clear,
    Dealloc,
    Del,
    Str,
    Doc,
    LegacyGetAttr,
    GetAttr,
    LegacySetAttr,
    SetAttr,
    RichCompare,
    IsGc,
    Iter,
    IterNext,
    Methods,
    New,
    DescrGet,
    DescrSet,
    Init,
    Traverse,
    Members,
    GetSet,
    Free,
    Finalize,
}

#[derive(Clone, Copy)]
enum NumberSlot {
    Divmod,
    Add,
    Subtract,
    Multiply,
    Remainder,
    Power,
    Negative,
    Positive,
    Absolute,
    Bool,
    Invert,
    LShift,
    RShift,
    And,
    Xor,
    Or,
    Int,
    Float,
    InPlaceAdd,
    InPlaceSubtract,
    InPlaceMultiply,
    InPlaceRemainder,
    InPlacePower,
    InPlaceLShift,
    InPlaceRShift,
    InPlaceAnd,
    InPlaceXor,
    InPlaceOr,
    FloorDivide,
    TrueDivide,
    InPlaceFloorDivide,
    InPlaceTrueDivide,
    Index,
    MatrixMultiply,
    InPlaceMatrixMultiply,
}

#[derive(Clone, Copy)]
enum SequenceSlot {
    Length,
    Concat,
    Repeat,
    Item,
    AssItem,
    Contains,
    InPlaceConcat,
    InPlaceRepeat,
}

#[derive(Clone, Copy)]
enum MappingSlot {
    Length,
    Subscript,
    AssSubscript,
}

#[derive(Clone, Copy)]
enum AsyncSlot {
    Await,
    Iter,
    Next,
    Send,
}

#[derive(Clone, Copy)]
enum BufferSlot {
    Get,
    Release,
}

struct SlotWrapperDef {
    name: &'static [u8],
    slot: SlotWrapper,
}

macro_rules! direct {
    ($name:literal, $slot:ident) => {
        SlotWrapperDef {
            name: concat!($name, "\0").as_bytes(),
            slot: SlotWrapper::Direct(DirectSlot::$slot),
        }
    };
}
macro_rules! number {
    ($name:literal, $slot:ident) => {
        SlotWrapperDef {
            name: concat!($name, "\0").as_bytes(),
            slot: SlotWrapper::Number(NumberSlot::$slot),
        }
    };
}
macro_rules! sequence {
    ($name:literal, $slot:ident) => {
        SlotWrapperDef {
            name: concat!($name, "\0").as_bytes(),
            slot: SlotWrapper::Sequence(SequenceSlot::$slot),
        }
    };
}
macro_rules! mapping {
    ($name:literal, $slot:ident) => {
        SlotWrapperDef {
            name: concat!($name, "\0").as_bytes(),
            slot: SlotWrapper::Mapping(MappingSlot::$slot),
        }
    };
}

static SLOT_WRAPPER_DEFS: &[SlotWrapperDef] = &[
    direct!("__repr__", Repr),
    direct!("__hash__", Hash),
    direct!("__call__", Call),
    direct!("__str__", Str),
    direct!("__getattribute__", GetAttr),
    direct!("__setattr__", SetAttr),
    direct!("__delattr__", SetAttr),
    direct!("__lt__", RichCompare),
    direct!("__le__", RichCompare),
    direct!("__eq__", RichCompare),
    direct!("__ne__", RichCompare),
    direct!("__gt__", RichCompare),
    direct!("__ge__", RichCompare),
    direct!("__iter__", Iter),
    direct!("__next__", IterNext),
    direct!("__get__", DescrGet),
    direct!("__set__", DescrSet),
    direct!("__delete__", DescrSet),
    direct!("__init__", Init),
    direct!("__del__", Finalize),
    SlotWrapperDef {
        name: b"__buffer__\0",
        slot: SlotWrapper::Buffer(BufferSlot::Get),
    },
    SlotWrapperDef {
        name: b"__release_buffer__\0",
        slot: SlotWrapper::Buffer(BufferSlot::Release),
    },
    SlotWrapperDef {
        name: b"__await__\0",
        slot: SlotWrapper::Async(AsyncSlot::Await),
    },
    SlotWrapperDef {
        name: b"__aiter__\0",
        slot: SlotWrapper::Async(AsyncSlot::Iter),
    },
    SlotWrapperDef {
        name: b"__anext__\0",
        slot: SlotWrapper::Async(AsyncSlot::Next),
    },
    number!("__add__", Add),
    number!("__radd__", Add),
    number!("__sub__", Subtract),
    number!("__rsub__", Subtract),
    number!("__mul__", Multiply),
    number!("__rmul__", Multiply),
    number!("__mod__", Remainder),
    number!("__rmod__", Remainder),
    number!("__pow__", Power),
    number!("__rpow__", Power),
    number!("__neg__", Negative),
    number!("__pos__", Positive),
    number!("__abs__", Absolute),
    number!("__bool__", Bool),
    number!("__invert__", Invert),
    number!("__lshift__", LShift),
    number!("__rlshift__", LShift),
    number!("__rshift__", RShift),
    number!("__rrshift__", RShift),
    number!("__and__", And),
    number!("__rand__", And),
    number!("__xor__", Xor),
    number!("__rxor__", Xor),
    number!("__or__", Or),
    number!("__ror__", Or),
    number!("__int__", Int),
    number!("__float__", Float),
    number!("__iadd__", InPlaceAdd),
    number!("__isub__", InPlaceSubtract),
    number!("__imul__", InPlaceMultiply),
    number!("__imod__", InPlaceRemainder),
    number!("__ipow__", InPlacePower),
    number!("__ilshift__", InPlaceLShift),
    number!("__irshift__", InPlaceRShift),
    number!("__iand__", InPlaceAnd),
    number!("__ixor__", InPlaceXor),
    number!("__ior__", InPlaceOr),
    number!("__floordiv__", FloorDivide),
    number!("__rfloordiv__", FloorDivide),
    number!("__truediv__", TrueDivide),
    number!("__rtruediv__", TrueDivide),
    number!("__ifloordiv__", InPlaceFloorDivide),
    number!("__itruediv__", InPlaceTrueDivide),
    number!("__index__", Index),
    number!("__matmul__", MatrixMultiply),
    number!("__rmatmul__", MatrixMultiply),
    number!("__imatmul__", InPlaceMatrixMultiply),
    mapping!("__len__", Length),
    mapping!("__getitem__", Subscript),
    mapping!("__setitem__", AssSubscript),
    mapping!("__delitem__", AssSubscript),
    sequence!("__len__", Length),
    sequence!("__add__", Concat),
    sequence!("__mul__", Repeat),
    sequence!("__rmul__", Repeat),
    sequence!("__getitem__", Item),
    sequence!("__setitem__", AssItem),
    sequence!("__delitem__", AssItem),
    sequence!("__contains__", Contains),
    sequence!("__iadd__", InPlaceConcat),
    sequence!("__imul__", InPlaceRepeat),
];

/// Return the raw pointer-sized storage that owns one direct type slot. Rust's
/// FFI `Option<extern "C" fn>` fields use the nullable-pointer representation;
/// the compile-time size/alignment assertions below make that assumption
/// explicit. Both FromSpec writes and GetSlot reads go through this address, so
/// the stable slot map cannot drift into independent setter/getter authorities.
unsafe fn direct_slot_storage(tp: *mut PyTypeObject, slot: DirectSlot) -> *mut *mut c_void {
    const _: () = assert!(
        std::mem::size_of::<Option<unsafe extern "C" fn(*mut PyObject)>>()
            == std::mem::size_of::<*mut c_void>()
    );
    const _: () = assert!(
        std::mem::align_of::<Option<unsafe extern "C" fn(*mut PyObject)>>()
            == std::mem::align_of::<*mut c_void>()
    );
    macro_rules! storage {
        ($field:ident) => {
            std::ptr::addr_of_mut!((*tp).$field).cast::<*mut c_void>()
        };
    }
    unsafe {
        match slot {
            DirectSlot::Alloc => storage!(tp_alloc),
            DirectSlot::Base => storage!(tp_base),
            DirectSlot::Bases => storage!(tp_bases),
            DirectSlot::Repr => storage!(tp_repr),
            DirectSlot::Hash => storage!(tp_hash),
            DirectSlot::Call => storage!(tp_call),
            DirectSlot::Clear => storage!(tp_clear),
            DirectSlot::Dealloc => storage!(tp_dealloc),
            DirectSlot::Del => storage!(tp_del),
            DirectSlot::Str => storage!(tp_str),
            DirectSlot::Doc => storage!(tp_doc),
            DirectSlot::LegacyGetAttr => storage!(tp_getattr),
            DirectSlot::GetAttr => storage!(tp_getattro),
            DirectSlot::LegacySetAttr => storage!(tp_setattr),
            DirectSlot::SetAttr => storage!(tp_setattro),
            DirectSlot::RichCompare => storage!(tp_richcompare),
            DirectSlot::IsGc => storage!(tp_is_gc),
            DirectSlot::Iter => storage!(tp_iter),
            DirectSlot::IterNext => storage!(tp_iternext),
            DirectSlot::Methods => storage!(tp_methods),
            DirectSlot::New => storage!(tp_new),
            DirectSlot::DescrGet => storage!(tp_descr_get),
            DirectSlot::DescrSet => storage!(tp_descr_set),
            DirectSlot::Init => storage!(tp_init),
            DirectSlot::Traverse => storage!(tp_traverse),
            DirectSlot::Members => storage!(tp_members),
            DirectSlot::GetSet => storage!(tp_getset),
            DirectSlot::Free => storage!(tp_free),
            DirectSlot::Finalize => storage!(tp_finalize),
        }
    }
}

/// Return the one pointer-sized storage cell for a public Stable-ABI slot.
/// Protocol tables are created only for FromSpec writes; GetSlot reads a missing
/// parent as a valid NULL slot without allocating or setting an exception.
unsafe fn slot_wrapper_storage(
    tp: *mut PyTypeObject,
    slot: SlotWrapper,
    create: bool,
) -> *mut *mut c_void {
    macro_rules! field_storage {
        ($table:expr, $field:ident) => {
            std::ptr::addr_of_mut!((*$table).$field)
        };
    }
    unsafe {
        match slot {
            SlotWrapper::Direct(slot) => direct_slot_storage(tp, slot),
            SlotWrapper::Number(slot) => {
                let table = if create {
                    ensure_number(tp)
                } else {
                    (*tp)
                        .tp_as_number
                        .cast::<crate::abi_types::PyNumberMethods>()
                };
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    NumberSlot::Divmod => field_storage!(table, nb_divmod),
                    NumberSlot::Add => field_storage!(table, nb_add),
                    NumberSlot::Subtract => field_storage!(table, nb_subtract),
                    NumberSlot::Multiply => field_storage!(table, nb_multiply),
                    NumberSlot::Remainder => field_storage!(table, nb_remainder),
                    NumberSlot::Power => field_storage!(table, nb_power),
                    NumberSlot::Negative => field_storage!(table, nb_negative),
                    NumberSlot::Positive => field_storage!(table, nb_positive),
                    NumberSlot::Absolute => field_storage!(table, nb_absolute),
                    NumberSlot::Bool => field_storage!(table, nb_bool),
                    NumberSlot::Invert => field_storage!(table, nb_invert),
                    NumberSlot::LShift => field_storage!(table, nb_lshift),
                    NumberSlot::RShift => field_storage!(table, nb_rshift),
                    NumberSlot::And => field_storage!(table, nb_and),
                    NumberSlot::Xor => field_storage!(table, nb_xor),
                    NumberSlot::Or => field_storage!(table, nb_or),
                    NumberSlot::Int => field_storage!(table, nb_int),
                    NumberSlot::Float => field_storage!(table, nb_float),
                    NumberSlot::InPlaceAdd => field_storage!(table, nb_inplace_add),
                    NumberSlot::InPlaceSubtract => {
                        field_storage!(table, nb_inplace_subtract)
                    }
                    NumberSlot::InPlaceMultiply => {
                        field_storage!(table, nb_inplace_multiply)
                    }
                    NumberSlot::InPlaceRemainder => {
                        field_storage!(table, nb_inplace_remainder)
                    }
                    NumberSlot::InPlacePower => field_storage!(table, nb_inplace_power),
                    NumberSlot::InPlaceLShift => field_storage!(table, nb_inplace_lshift),
                    NumberSlot::InPlaceRShift => field_storage!(table, nb_inplace_rshift),
                    NumberSlot::InPlaceAnd => field_storage!(table, nb_inplace_and),
                    NumberSlot::InPlaceXor => field_storage!(table, nb_inplace_xor),
                    NumberSlot::InPlaceOr => field_storage!(table, nb_inplace_or),
                    NumberSlot::FloorDivide => field_storage!(table, nb_floor_divide),
                    NumberSlot::TrueDivide => field_storage!(table, nb_true_divide),
                    NumberSlot::InPlaceFloorDivide => {
                        field_storage!(table, nb_inplace_floor_divide)
                    }
                    NumberSlot::InPlaceTrueDivide => {
                        field_storage!(table, nb_inplace_true_divide)
                    }
                    NumberSlot::Index => field_storage!(table, nb_index),
                    NumberSlot::MatrixMultiply => field_storage!(table, nb_matrix_multiply),
                    NumberSlot::InPlaceMatrixMultiply => {
                        field_storage!(table, nb_inplace_matrix_multiply)
                    }
                }
            }
            SlotWrapper::Sequence(slot) => {
                let table = if create {
                    ensure_sequence(tp)
                } else {
                    (*tp)
                        .tp_as_sequence
                        .cast::<crate::abi_types::PySequenceMethods>()
                };
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    SequenceSlot::Length => field_storage!(table, sq_length),
                    SequenceSlot::Concat => field_storage!(table, sq_concat),
                    SequenceSlot::Repeat => field_storage!(table, sq_repeat),
                    SequenceSlot::Item => field_storage!(table, sq_item),
                    SequenceSlot::AssItem => field_storage!(table, sq_ass_item),
                    SequenceSlot::Contains => field_storage!(table, sq_contains),
                    SequenceSlot::InPlaceConcat => field_storage!(table, sq_inplace_concat),
                    SequenceSlot::InPlaceRepeat => field_storage!(table, sq_inplace_repeat),
                }
            }
            SlotWrapper::Mapping(slot) => {
                let table = if create {
                    ensure_mapping(tp)
                } else {
                    (*tp)
                        .tp_as_mapping
                        .cast::<crate::abi_types::PyMappingMethods>()
                };
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    MappingSlot::Length => field_storage!(table, mp_length),
                    MappingSlot::Subscript => field_storage!(table, mp_subscript),
                    MappingSlot::AssSubscript => field_storage!(table, mp_ass_subscript),
                }
            }
            SlotWrapper::Async(slot) => {
                let table = if create {
                    ensure_async(tp)
                } else {
                    (*tp).tp_as_async.cast::<crate::abi_types::PyAsyncMethods>()
                };
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    AsyncSlot::Await => field_storage!(table, am_await),
                    AsyncSlot::Iter => field_storage!(table, am_aiter),
                    AsyncSlot::Next => field_storage!(table, am_anext),
                    AsyncSlot::Send => field_storage!(table, am_send),
                }
            }
            SlotWrapper::Buffer(slot) => {
                let table = if create {
                    ensure_buffer(tp)
                } else {
                    (*tp).tp_as_buffer.cast::<crate::abi_types::PyBufferProcs>()
                };
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    BufferSlot::Get => field_storage!(table, bf_getbuffer),
                    BufferSlot::Release => field_storage!(table, bf_releasebuffer),
                }
            }
        }
    }
}

unsafe fn slot_wrapper_ptr(tp: *mut PyTypeObject, slot: SlotWrapper) -> *mut c_void {
    let storage = unsafe { slot_wrapper_storage(tp, slot, false) };
    if storage.is_null() {
        ptr::null_mut()
    } else {
        unsafe { storage.read() }
    }
}

fn stable_slot_wrapper(slot: c_int) -> Option<SlotWrapper> {
    use AsyncSlot as A;
    use BufferSlot as B;
    use DirectSlot as D;
    use MappingSlot as M;
    use NumberSlot as N;
    use SequenceSlot as S;
    Some(match slot {
        ts::Py_bf_getbuffer => SlotWrapper::Buffer(B::Get),
        ts::Py_bf_releasebuffer => SlotWrapper::Buffer(B::Release),
        ts::Py_mp_ass_subscript => SlotWrapper::Mapping(M::AssSubscript),
        ts::Py_mp_length => SlotWrapper::Mapping(M::Length),
        ts::Py_mp_subscript => SlotWrapper::Mapping(M::Subscript),
        ts::Py_nb_absolute => SlotWrapper::Number(N::Absolute),
        ts::Py_nb_add => SlotWrapper::Number(N::Add),
        ts::Py_nb_and => SlotWrapper::Number(N::And),
        ts::Py_nb_bool => SlotWrapper::Number(N::Bool),
        ts::Py_nb_divmod => SlotWrapper::Number(N::Divmod),
        ts::Py_nb_float => SlotWrapper::Number(N::Float),
        ts::Py_nb_floor_divide => SlotWrapper::Number(N::FloorDivide),
        ts::Py_nb_index => SlotWrapper::Number(N::Index),
        ts::Py_nb_inplace_add => SlotWrapper::Number(N::InPlaceAdd),
        ts::Py_nb_inplace_and => SlotWrapper::Number(N::InPlaceAnd),
        ts::Py_nb_inplace_floor_divide => SlotWrapper::Number(N::InPlaceFloorDivide),
        ts::Py_nb_inplace_lshift => SlotWrapper::Number(N::InPlaceLShift),
        ts::Py_nb_inplace_multiply => SlotWrapper::Number(N::InPlaceMultiply),
        ts::Py_nb_inplace_or => SlotWrapper::Number(N::InPlaceOr),
        ts::Py_nb_inplace_power => SlotWrapper::Number(N::InPlacePower),
        ts::Py_nb_inplace_remainder => SlotWrapper::Number(N::InPlaceRemainder),
        ts::Py_nb_inplace_rshift => SlotWrapper::Number(N::InPlaceRShift),
        ts::Py_nb_inplace_subtract => SlotWrapper::Number(N::InPlaceSubtract),
        ts::Py_nb_inplace_true_divide => SlotWrapper::Number(N::InPlaceTrueDivide),
        ts::Py_nb_inplace_xor => SlotWrapper::Number(N::InPlaceXor),
        ts::Py_nb_int => SlotWrapper::Number(N::Int),
        ts::Py_nb_invert => SlotWrapper::Number(N::Invert),
        ts::Py_nb_lshift => SlotWrapper::Number(N::LShift),
        ts::Py_nb_multiply => SlotWrapper::Number(N::Multiply),
        ts::Py_nb_negative => SlotWrapper::Number(N::Negative),
        ts::Py_nb_or => SlotWrapper::Number(N::Or),
        ts::Py_nb_positive => SlotWrapper::Number(N::Positive),
        ts::Py_nb_power => SlotWrapper::Number(N::Power),
        ts::Py_nb_remainder => SlotWrapper::Number(N::Remainder),
        ts::Py_nb_rshift => SlotWrapper::Number(N::RShift),
        ts::Py_nb_subtract => SlotWrapper::Number(N::Subtract),
        ts::Py_nb_true_divide => SlotWrapper::Number(N::TrueDivide),
        ts::Py_nb_xor => SlotWrapper::Number(N::Xor),
        ts::Py_sq_ass_item => SlotWrapper::Sequence(S::AssItem),
        ts::Py_sq_concat => SlotWrapper::Sequence(S::Concat),
        ts::Py_sq_contains => SlotWrapper::Sequence(S::Contains),
        ts::Py_sq_inplace_concat => SlotWrapper::Sequence(S::InPlaceConcat),
        ts::Py_sq_inplace_repeat => SlotWrapper::Sequence(S::InPlaceRepeat),
        ts::Py_sq_item => SlotWrapper::Sequence(S::Item),
        ts::Py_sq_length => SlotWrapper::Sequence(S::Length),
        ts::Py_sq_repeat => SlotWrapper::Sequence(S::Repeat),
        ts::Py_tp_alloc => SlotWrapper::Direct(D::Alloc),
        ts::Py_tp_base => SlotWrapper::Direct(D::Base),
        ts::Py_tp_bases => SlotWrapper::Direct(D::Bases),
        ts::Py_tp_call => SlotWrapper::Direct(D::Call),
        ts::Py_tp_clear => SlotWrapper::Direct(D::Clear),
        ts::Py_tp_dealloc => SlotWrapper::Direct(D::Dealloc),
        ts::Py_tp_del => SlotWrapper::Direct(D::Del),
        ts::Py_tp_descr_get => SlotWrapper::Direct(D::DescrGet),
        ts::Py_tp_descr_set => SlotWrapper::Direct(D::DescrSet),
        ts::Py_tp_doc => SlotWrapper::Direct(D::Doc),
        ts::Py_tp_getattr => SlotWrapper::Direct(D::LegacyGetAttr),
        ts::Py_tp_getattro => SlotWrapper::Direct(D::GetAttr),
        ts::Py_tp_hash => SlotWrapper::Direct(D::Hash),
        ts::Py_tp_init => SlotWrapper::Direct(D::Init),
        ts::Py_tp_is_gc => SlotWrapper::Direct(D::IsGc),
        ts::Py_tp_iter => SlotWrapper::Direct(D::Iter),
        ts::Py_tp_iternext => SlotWrapper::Direct(D::IterNext),
        ts::Py_tp_methods => SlotWrapper::Direct(D::Methods),
        ts::Py_tp_new => SlotWrapper::Direct(D::New),
        ts::Py_tp_repr => SlotWrapper::Direct(D::Repr),
        ts::Py_tp_richcompare => SlotWrapper::Direct(D::RichCompare),
        ts::Py_tp_setattr => SlotWrapper::Direct(D::LegacySetAttr),
        ts::Py_tp_setattro => SlotWrapper::Direct(D::SetAttr),
        ts::Py_tp_str => SlotWrapper::Direct(D::Str),
        ts::Py_tp_traverse => SlotWrapper::Direct(D::Traverse),
        ts::Py_tp_members => SlotWrapper::Direct(D::Members),
        ts::Py_tp_getset => SlotWrapper::Direct(D::GetSet),
        ts::Py_tp_free => SlotWrapper::Direct(D::Free),
        ts::Py_nb_matrix_multiply => SlotWrapper::Number(N::MatrixMultiply),
        ts::Py_nb_inplace_matrix_multiply => SlotWrapper::Number(N::InPlaceMatrixMultiply),
        ts::Py_am_await => SlotWrapper::Async(A::Await),
        ts::Py_am_aiter => SlotWrapper::Async(A::Iter),
        ts::Py_am_anext => SlotWrapper::Async(A::Next),
        ts::Py_tp_finalize => SlotWrapper::Direct(D::Finalize),
        ts::Py_am_send => SlotWrapper::Async(A::Send),
        _ => return None,
    })
}

/// Return one CPython Stable-ABI type-slot value for every public id 1..=81.
/// The numeric ids come from the single generated authority; the lookup reads
/// the same concrete fields that `PyType_FromSpec*` populates.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetSlot(tp: *mut PyTypeObject, slot: c_int) -> *mut c_void {
    if tp.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(wrapper) = stable_slot_wrapper(slot) else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    };
    unsafe { slot_wrapper_ptr(tp, wrapper) }
}

unsafe fn new_wrapper_descr(
    tp: *mut PyTypeObject,
    name: *const c_char,
    wrapped: *mut c_void,
) -> *mut PyObject {
    let common = unsafe { descr_alloc(&raw mut crate::abi_types::PyWrapperDescr_Type, tp, name) };
    if common.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let header = *Box::from_raw(common);
        let descr = Box::new(crate::abi_types::PyWrapperDescrObject {
            d_common: header,
            d_wrapped: wrapped,
        });
        let ptr = Box::into_raw(descr).cast::<PyObject>();
        let _ = crate::bridge::GLOBAL_BRIDGE.register_foreign_pyobj(ptr);
        ptr
    }
}

unsafe fn add_operators_to_dict(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let dict = (*tp).tp_dict;
        // A NULL tp_hash means "inherit from the tp_base chain" (CPython), NOT
        // "unhashable". Inherit it BEFORE the slot-wrapper loop builds __hash__
        // and before the __hash__=None baking below — else a metatype whose base
        // (PyType_Type) supplies an identity hash, i.e. numpy's `_DTypeMeta`, has
        // its DType CLASSES marked "unhashable type: 'type'" during numpy.dtypes
        // registration. Genuinely-unhashable types set tp_hash =
        // PyObject_HashNotImplemented (still baked to __hash__=None below).
        // PyBaseObject_Type.tp_hash is now the identity _Py_HashPointer (CPython
        // object.__hash__), so a plain object() and inheriting-only-object
        // subtypes are hashable; the builtin containers (list/dict/set/bytearray)
        // set PyObject_HashNotImplemented on their own type shells in abi_types, so
        // they stay unhashable despite the now-non-NULL object root.
        if (*tp).tp_hash.is_none() {
            let mut base = (*tp).tp_base;
            while !base.is_null() {
                if let Some(h) = (*base).tp_hash {
                    (*tp).tp_hash = Some(h);
                    break;
                }
                base = (*base).tp_base;
            }
        }
        for def in SLOT_WRAPPER_DEFS {
            let wrapped = slot_wrapper_ptr(tp, def.slot);
            if wrapped.is_null() {
                continue;
            }
            let name = def.name.as_ptr().cast::<c_char>();
            if !crate::api::mapping::PyDict_GetItemString(dict, name).is_null() {
                continue;
            }
            let hash_not_implemented = matches!(def.slot, SlotWrapper::Direct(DirectSlot::Hash))
                && wrapped == PyObject_HashNotImplemented as *const () as *mut c_void;
            if hash_not_implemented {
                if crate::api::mapping::PyDict_SetItemString(
                    dict,
                    name,
                    &raw mut crate::abi_types::Py_None,
                ) < 0
                {
                    return -1;
                }
            } else {
                let descr = new_wrapper_descr(tp, name, wrapped);
                if descr.is_null() {
                    return -1;
                }
                let stored = crate::api::mapping::PyDict_SetItemString(dict, name, descr);
                crate::api::refcount::Py_DECREF(descr);
                if stored < 0 {
                    return -1;
                }
            }
        }
        if (*tp).tp_hash.is_none()
            && crate::api::mapping::PyDict_GetItemString(dict, c"__hash__".as_ptr()).is_null()
            && crate::api::mapping::PyDict_SetItemString(
                dict,
                c"__hash__".as_ptr(),
                &raw mut crate::abi_types::Py_None,
            ) < 0
        {
            return -1;
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
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
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
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
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
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

/// CPython 3.12 `type_is_gc` — the `tp_is_gc` slot of `PyType_Type`.
///
/// `type` itself advertises `Py_TPFLAGS_HAVE_GC`, but only heap-allocated type
/// objects may participate in cycles and be traversed by the collector.  This
/// predicate is therefore deliberately keyed on the candidate type object's
/// `Py_TPFLAGS_HEAPTYPE` bit, not on `Py_TPFLAGS_HAVE_GC`.  C extensions call
/// this slot directly (numpy's `_DTypeMeta` does so while deciding whether a
/// dtype metaclass instance is GC-tracked), so the canonical `PyType_Type`
/// must publish a real callable rather than relying on the bridge's optional-
/// slot fallback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_type_is_gc(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ty = op.cast::<PyTypeObject>();
    (unsafe { (*ty).tp_flags } & Py_TPFLAGS_HEAPTYPE) as c_int
}

type TypeVisitProc = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int;

/// CPython 3.12 ``type_traverse`` for heap type objects.
///
/// ``type_is_gc`` prevents the collector from invoking this slot for static
/// types. Heap types own references through the common type header plus
/// ``PyHeapTypeObject.ht_module``; visiting this exact family is what makes
/// class/dict/MRO/module cycles observable to the collector.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_type_traverse(
    op: *mut PyObject,
    visit_raw: *mut c_void,
    arg: *mut c_void,
) -> c_int {
    if op.is_null() || visit_raw.is_null() {
        return 0;
    }
    let type_ = op.cast::<PyTypeObject>();
    if unsafe { (*type_).tp_flags } & Py_TPFLAGS_HEAPTYPE == 0 {
        return 0;
    }
    let visit: TypeVisitProc = unsafe { std::mem::transmute(visit_raw) };
    let heap = type_.cast::<PyHeapTypeObject>();
    let references = unsafe {
        [
            (*type_).tp_dict,
            (*type_).tp_cache,
            (*type_).tp_mro,
            (*type_).tp_bases,
            (*type_).tp_base.cast::<PyObject>(),
            (*heap).ht_module,
        ]
    };
    for reference in references {
        if reference.is_null() {
            continue;
        }
        let rc = unsafe { visit(reference, arg) };
        if rc != 0 {
            return rc;
        }
    }
    0
}

/// CPython 3.12 ``type_clear`` for heap type objects.
///
/// Invalidate method-cache authority before clearing the type dict, then break
/// the two hard ownership cycles CPython clears here: ``ht_module`` and
/// ``tp_mro``. Bases/cache/subclasses/slot-name tuples are deliberately retained
/// for the same ownership reasons as CPython's implementation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_type_clear(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let type_ = op.cast::<PyTypeObject>();
    if unsafe { (*type_).tp_flags } & Py_TPFLAGS_HEAPTYPE == 0 {
        return 0;
    }
    unsafe {
        PyType_Modified(type_);
        if !(*type_).tp_dict.is_null() {
            crate::api::mapping::PyDict_Clear((*type_).tp_dict);
        }
        let heap = type_.cast::<PyHeapTypeObject>();
        crate::api::refcount::Py_CLEAR(&raw mut (*heap).ht_module);
        crate::api::refcount::Py_CLEAR(&raw mut (*type_).tp_mro);
    }
    0
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
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
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
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
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
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"tp_new returned NULL without setting an exception".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        if !crate::api::errors::PyErr_Occurred().is_null() {
            crate::api::refcount::Py_DECREF(obj);
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
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
    // CPython Objects/typeobject.c: `return type->tp_alloc(type, 0);` — dispatch
    // the type's OWN tp_alloc slot (a C extension may install a custom allocator).
    // Fall back to PyType_GenericAlloc only when tp_alloc is absent.
    if !tp.is_null()
        && let Some(alloc) = unsafe { (*tp).tp_alloc }
    {
        return unsafe { alloc(tp, 0) };
    }
    unsafe { PyType_GenericAlloc(tp, 0) }
}

use crate::type_slots as ts;

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
            let Some(wrapper) = stable_slot_wrapper(id) else {
                crate::capi_trace::record_silent_failure(
                    "PyType_FromSpec",
                    Some(&format!("unknown PyType_Slot id {id}")),
                );
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_RuntimeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"PyType_FromSpec: invalid slot offset".as_ptr(),
                );
                return -1;
            };
            // CPython owns a private copy of tp_doc. Every other stable slot is
            // a pointer-sized value written through the shared slot-storage map.
            let stored = if id == ts::Py_tp_doc && !pfunc.is_null() {
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
                buf.cast::<c_void>()
            } else {
                pfunc
            };
            let storage = slot_wrapper_storage(ty, wrapper, true);
            if storage.is_null() {
                crate::capi_trace::record_silent_failure(
                    "PyType_FromSpec",
                    Some("stable type-slot storage unavailable"),
                );
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"PyType_FromSpec: stable slot storage unavailable".as_ptr(),
                );
                return -1;
            }
            storage.write(stored);
            slot = slot.add(1);
        }
        0
    }
}

/// Shared body for `PyType_FromSpec*` / `PyType_FromMetaclass`. Allocates a real
/// `PyHeapTypeObject` (NOT a bare `Box<PyTypeObject>`), sets `Py_TPFLAGS_HEAPTYPE`
/// and populates `ht_name`/`ht_qualname`/`ht_module`, so an extension's inlined
/// `((PyHeapTypeObject*)type)->ht_name`/`ht_module` reads land IN BOUNDS and the
/// per-module state a spec type carries is retained (matrix PyTypeObject #3, L3).
/// Mirrors CPython v3.12.0 `_PyType_FromMetaclass_impl` (Objects/typeobject.c):
/// `type->tp_flags = spec->flags | Py_TPFLAGS_HEAPTYPE`, `ht_name` = the segment
/// after the last '.' in `spec->name`, `ht_qualname = ht_name`, `ht_module =
/// Py_XNewRef(module)`.
///
/// The heap type is intentionally leaked (like every static/extension type in the
/// process): heap types created during extension import live for the process, and
/// molt has no type teardown path that would reclaim it — so the larger allocation
/// never causes a size-mismatched free.
unsafe fn type_from_spec_impl(
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
    module: *mut PyObject,
) -> *mut PyObject {
    if spec.is_null() {
        return ptr::null_mut();
    }
    let heap: Box<crate::abi_types::PyHeapTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    let heap_ptr = Box::into_raw(heap);
    unsafe {
        let tp: *mut PyTypeObject = &raw mut (*heap_ptr).ht_type;
        (*tp).ob_base.ob_base.ob_refcnt = 1;
        (*tp).ob_base.ob_base.ob_type = &raw mut crate::abi_types::PyType_Type;
        (*tp).ob_base.ob_size = 0;
        (*tp).tp_name = (*spec).name;
        (*tp).tp_basicsize = (*spec).basicsize as Py_ssize_t;
        (*tp).tp_itemsize = (*spec).itemsize as Py_ssize_t;
        // HEAPTYPE is mandatory for a spec-built type (CPython always ORs it), but
        // do NOT pre-mark READY — PyType_Ready must run its full pipeline below.
        (*tp).tp_flags = ((*spec).flags as std::os::raw::c_ulong & !Py_TPFLAGS_READY)
            | crate::abi_types::Py_TPFLAGS_HEAPTYPE;

        // ht_name / ht_qualname: the `spec->name` segment after the last '.', as a
        // str object. The C string is null-terminated, so the after-dot pointer is
        // itself a valid C string — no copy needed. Best-effort: a NULL (str
        // allocation unavailable) is still IN BOUNDS, never OOB.
        let name_ptr = (*spec).name;
        if !name_ptr.is_null() {
            let short = match std::ffi::CStr::from_ptr(name_ptr)
                .to_bytes()
                .iter()
                .rposition(|&b| b == b'.')
            {
                Some(dot) => name_ptr.add(dot + 1),
                None => name_ptr,
            };
            let ht_name = crate::api::strings::PyUnicode_FromString(short);
            if !ht_name.is_null() {
                (*heap_ptr).ht_name = ht_name;
                crate::api::refcount::Py_INCREF(ht_name);
                (*heap_ptr).ht_qualname = ht_name;
            }
        }

        // ht_module: retain the defining module (Py_XNewRef) so PyType_GetModule /
        // PyType_GetModuleState resolve instead of dropping per-module state.
        if !module.is_null() {
            crate::api::refcount::Py_INCREF(module);
            (*heap_ptr).ht_module = module;
        }

        // (1) Apply every spec slot to its destination field/sub-table. A bad slot
        //     id fails closed with a pending exception.
        if apply_spec_slots(tp, (*spec).slots) < 0 {
            return ptr::null_mut();
        }

        // (2) Resolve the base. A Py_tp_base slot wins; otherwise derive from the
        //     explicit `bases` tuple (first entry — the single-inheritance case
        //     numpy/scipy use); otherwise PyType_Ready defaults it to `object`.
        if !bases.is_null() {
            let best = acceptable_best_base(bases);
            if best.is_null() && crate::api::sequences::PyTuple_Size(bases) != 0 {
                return ptr::null_mut();
            }
            crate::api::refcount::Py_INCREF(bases);
            (*tp).tp_bases = bases;
            if !best.is_null() {
                (*tp).tp_base = best;
            }
        }
        if !(*tp).tp_base.is_null() && validate_base_layout(tp, (*tp).tp_base) < 0 {
            return ptr::null_mut();
        }

        // (3) Instantiation defaults where the spec left them unset.
        if (*tp).tp_alloc.is_none() {
            (*tp).tp_alloc = Some(PyType_GenericAlloc);
        }
        if (*tp).tp_new.is_none() {
            (*tp).tp_new = Some(PyType_GenericNew);
        }

        // (4) Comprehensive readiness pipeline (base default, slot inherit, dict,
        //     mro, mark READY).
        if PyType_Ready(tp) < 0 {
            return ptr::null_mut();
        }
        tp.cast::<PyObject>()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromSpecWithBases(
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    unsafe { type_from_spec_impl(spec, bases, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromModuleAndSpec(
    module: *mut PyObject,
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    unsafe { type_from_spec_impl(spec, bases, module) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromMetaclass(
    _metaclass: *mut PyTypeObject,
    module: *mut PyObject,
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    unsafe { type_from_spec_impl(spec, bases, module) }
}

/// CPython `PyType_GetModule` (Objects/typeobject.c): the module a heap type was
/// defined in. Requires `Py_TPFLAGS_HEAPTYPE` (TypeError otherwise) and reads
/// `((PyHeapTypeObject*)type)->ht_module` — in bounds now that spec types are full
/// `PyHeapTypeObject`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetModule(ty: *mut PyTypeObject) -> *mut PyObject {
    if ty.is_null() {
        return ptr::null_mut();
    }
    if unsafe { (*ty).tp_flags } & crate::abi_types::Py_TPFLAGS_HEAPTYPE == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"PyType_GetModule: Type is not a heap type".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let et = ty.cast::<crate::abi_types::PyHeapTypeObject>();
    let m = unsafe { (*et).ht_module };
    if m.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"PyType_GetModule: This type has no module associated with it".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    m
}

/// CPython `PyType_GetModuleState`: the per-module state of the heap type's
/// defining module, or NULL (with the `PyType_GetModule` exception on a non-heap
/// type, or cleanly when the module carries no state).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetModuleState(ty: *mut PyTypeObject) -> *mut c_void {
    let m = unsafe { PyType_GetModule(ty) };
    if m.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::modules::PyModule_GetState(m) }
}

/// CPython `PyType_GetModuleByDef` (Objects/typeobject.c): walk `type`'s MRO and
/// return the first *heap* super whose `ht_module` belongs to `def`. molt has no
/// `PyModule_GetDef`, so it matches via `PyState_FindModule(def)` (the runtime's
/// def→module registry) and returns that module iff it is the `ht_module` of some
/// heap type on the MRO/base chain. Sufficient for the single-module extension
/// shape numpy/Cython use; the strict def-per-super match is specced, not fully
/// implemented (PEP 573 long tail).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetModuleByDef(
    ty: *mut PyTypeObject,
    def: *mut crate::abi_types::PyModuleDef,
) -> *mut PyObject {
    if ty.is_null() || def.is_null() {
        return ptr::null_mut();
    }
    let target = unsafe { crate::api::modules::PyState_FindModule(def) };
    // Walk the base chain (falling back from tp_mro), returning the target module
    // if it is the ht_module of a heap super — otherwise NULL (no matching super).
    let mut cursor = ty;
    while !cursor.is_null() {
        if unsafe { (*cursor).tp_flags } & crate::abi_types::Py_TPFLAGS_HEAPTYPE != 0 {
            let et = cursor.cast::<crate::abi_types::PyHeapTypeObject>();
            let m = unsafe { (*et).ht_module };
            if !m.is_null() && (target.is_null() || std::ptr::eq(m, target)) {
                return m;
            }
        }
        cursor = unsafe { (*cursor).tp_base };
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    // CPython: PyType_Check(op) == PyType_FastSubclass(Py_TYPE(op),
    // Py_TPFLAGS_TYPE_SUBCLASS) — true whenever op's METATYPE is `type` OR a
    // subtype of it (numpy DType classes carry a `type`-subclass metatype such
    // as `PyArrayDTypeMeta_Type`). The prior exact ob_type == &PyType_Type
    // compare answered only PyType_CheckExact and rejected every C metaclass
    // instance. Walk the metatype's subtype chain like PyObject_TypeCheck.
    let type_type = &raw mut crate::abi_types::PyType_Type;
    let meta = unsafe { (*op).ob_type };
    if meta.is_null() {
        return 0;
    }
    if std::ptr::eq(meta, type_type) {
        return 1;
    }
    unsafe { PyType_IsSubtype(meta, type_type) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Modified(tp: *mut PyTypeObject) {
    fn live_subclasses(tp: *mut PyTypeObject) -> Vec<TypeIdentity> {
        {
            let mut registry = TYPE_SUBCLASSES.lock();
            let Some(tp_identity) = type_identity(&mut registry, tp) else {
                return Vec::new();
            };
            let order = registry
                .subclasses
                .get(&tp_identity)
                .map(|entry| entry.order.clone())
                .unwrap_or_default();
            let members = registry
                .subclasses
                .get(&tp_identity)
                .map(|entry| entry.members.clone())
                .unwrap_or_default();
            let live_order: Vec<_> = order
                .into_iter()
                .filter(|identity| {
                    members.contains(identity)
                        && registry.live.get(&identity.address) == Some(&identity.generation)
                })
                .collect();
            if let Some(entry) = registry.subclasses.get_mut(&tp_identity) {
                entry.order.clone_from(&live_order);
                entry.members = live_order.iter().copied().collect();
            }
            live_order
        }
    }

    unsafe fn invalidate_one(tp: *mut PyTypeObject) {
        let watched = unsafe { (*tp).tp_watched };
        if watched != 0 {
            let watcher_state = TYPE_WATCHER_STATE.lock();
            debug_assert_eq!(watcher_state.interpreter_id, CANONICAL_INTERPRETER_ID);
            let watchers = watcher_state.callbacks;
            drop(watcher_state);
            for (watcher_id, callback) in watchers.into_iter().enumerate() {
                if watched & (1 << watcher_id) == 0 {
                    continue;
                }
                if let Some(callback) = callback
                    && unsafe { callback(tp.cast::<PyObject>()) } < 0
                {
                    unsafe { crate::api::errors::PyErr_WriteUnraisable(tp.cast()) };
                }
            }
        }
        unsafe {
            (*tp).tp_flags &= !crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG;
            (*tp).tp_version_tag = 0;
            if (*tp).tp_flags & Py_TPFLAGS_HEAPTYPE != 0 {
                (*tp.cast::<PyHeapTypeObject>())._spec_cache.getitem = ptr::null_mut();
            }
        }
    }
    if tp.is_null() {
        return;
    }
    // Explicit post-order traversal preserves CPython's subclass-before-base
    // callback order without consuming one Rust stack frame per hierarchy
    // level. Children are pushed in reverse so their registration order is
    // observed deterministically.
    let mut seen = HashSet::new();
    let mut work = vec![(tp, false)];
    while let Some((current, expanded)) = work.pop() {
        if current.is_null() {
            continue;
        }
        if expanded {
            unsafe { invalidate_one(current) };
            continue;
        }
        if !seen.insert(current.addr())
            || unsafe { (*current).tp_flags } & crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG == 0
        {
            continue;
        }
        work.push((current, true));
        for child in live_subclasses(current).into_iter().rev() {
            work.push((
                ptr::with_exposed_provenance_mut::<PyTypeObject>(child.address),
                false,
            ));
        }
    }
}

unsafe fn validate_type_watcher_id(watcher_id: c_int) -> bool {
    if watcher_id < 0 || watcher_id as usize >= TYPE_MAX_WATCHERS {
        unsafe { reject_type_layout(c"invalid type watcher ID") };
        return false;
    }
    if TYPE_WATCHER_STATE.lock().callbacks[watcher_id as usize].is_none() {
        unsafe { reject_type_layout(c"no type watcher is registered for this ID") };
        return false;
    }
    true
}

unsafe fn assign_type_version_tag(tp: *mut PyTypeObject, seen: &mut HashSet<usize>) -> bool {
    if tp.is_null() {
        return false;
    }
    if unsafe { (*tp).tp_flags } & crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG != 0 {
        return true;
    }
    if !seen.insert(tp as usize) {
        return false;
    }
    if unsafe { (*tp).tp_flags } & Py_TPFLAGS_READY == 0 {
        return false;
    }
    let bases = unsafe { (*tp).tp_bases };
    if !bases.is_null() {
        let count = unsafe { crate::api::sequences::PyTuple_Size(bases) };
        if count < 0 {
            return false;
        }
        for index in 0..count {
            let base = unsafe { crate::api::sequences::PyTuple_GetItem(bases, index) }
                .cast::<PyTypeObject>();
            if !unsafe { assign_type_version_tag(base, seen) } {
                return false;
            }
        }
    }
    let Some(tag) = allocate_type_version_tag(&NEXT_TYPE_VERSION_TAG) else {
        return false;
    };
    unsafe {
        (*tp).tp_version_tag = tag as c_ulong;
        (*tp).tp_flags |= crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG;
    }
    true
}

fn allocate_type_version_tag(counter: &AtomicU32) -> Option<u32> {
    match counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        current.checked_add(1)
    }) {
        Ok(tag) if tag != 0 => Some(tag),
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_AddWatcher(callback: Option<PyTypeWatchCallback>) -> c_int {
    let Some(callback) = callback else {
        unsafe { reject_type_layout(c"type watcher callback must not be NULL") };
        return -1;
    };
    let mut watcher_state = TYPE_WATCHER_STATE.lock();
    debug_assert_eq!(watcher_state.interpreter_id, CANONICAL_INTERPRETER_ID);
    if let Some((index, slot)) = watcher_state
        .callbacks
        .iter_mut()
        .enumerate()
        .find(|(_, slot)| slot.is_none())
    {
        *slot = Some(callback);
        return index as c_int;
    }
    unsafe { reject_type_layout(c"no more type watcher IDs available") };
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_ClearWatcher(watcher_id: c_int) -> c_int {
    if !unsafe { validate_type_watcher_id(watcher_id) } {
        return -1;
    }
    TYPE_WATCHER_STATE.lock().callbacks[watcher_id as usize] = None;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Watch(watcher_id: c_int, obj: *mut PyObject) -> c_int {
    if obj.is_null() || unsafe { PyType_Check(obj) } == 0 {
        unsafe { reject_type_layout(c"cannot watch a non-type object") };
        return -1;
    }
    if !unsafe { validate_type_watcher_id(watcher_id) } {
        return -1;
    }
    let tp = obj.cast::<PyTypeObject>();
    if !unsafe { assign_type_version_tag(tp, &mut HashSet::new()) } {
        unsafe { reject_type_layout(c"cannot assign a version tag to this type") };
        return -1;
    }
    unsafe { (*tp).tp_watched |= 1 << watcher_id };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Unwatch(watcher_id: c_int, obj: *mut PyObject) -> c_int {
    if obj.is_null() || unsafe { PyType_Check(obj) } == 0 {
        unsafe { reject_type_layout(c"cannot unwatch a non-type object") };
        return -1;
    }
    if !unsafe { validate_type_watcher_id(watcher_id) } {
        return -1;
    }
    unsafe { (*obj.cast::<PyTypeObject>()).tp_watched &= !(1 << watcher_id) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Type_AssignVersionTag(tp: *mut PyTypeObject) -> c_int {
    unsafe { assign_type_version_tag(tp, &mut HashSet::new()) as c_int }
}

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
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
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
        crate::bridge::GLOBAL_BRIDGE.register_foreign_pyobj(ptr);
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
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
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
        crate::bridge::GLOBAL_BRIDGE.register_foreign_pyobj(ptr);
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
                    (&raw mut crate::abi_types::PyExc_AttributeError)
                        .cast::<crate::abi_types::PyObject>(),
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
                    (&raw mut crate::abi_types::PyExc_AttributeError)
                        .cast::<crate::abi_types::PyObject>(),
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
                    (&raw mut crate::abi_types::Py_True).cast::<PyObject>()
                } else {
                    (&raw mut crate::abi_types::Py_False).cast::<PyObject>()
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
                            (&raw mut crate::abi_types::PyExc_AttributeError)
                                .cast::<crate::abi_types::PyObject>(),
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
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
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
                (&raw mut crate::abi_types::PyExc_AttributeError)
                    .cast::<crate::abi_types::PyObject>(),
                c"readonly attribute".as_ptr(),
            );
            return -1;
        }
        let ty = (*member).type_;
        let field = addr.offset((*member).offset);
        // CPython Python/structmember.c delete (v == NULL) rules: only T_OBJECT
        // (unconditionally) and T_OBJECT_EX (when already set) may be deleted;
        // deleting a numeric/char member is a TypeError.
        if value.is_null() {
            if ty == PY_T_OBJECT_EX {
                if (*(field as *const *mut PyObject)).is_null() {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_AttributeError)
                            .cast::<crate::abi_types::PyObject>(),
                        (*member).name,
                    );
                    return -1;
                }
            } else if ty != PY_T_OBJECT {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"can't delete numeric/char attribute".as_ptr(),
                );
                return -1;
            }
        }
        // Helper: has an exception been raised by a converter?
        let err_set = || !crate::api::errors::PyErr_Occurred().is_null();
        // NOTE: CPython emits a non-fatal RuntimeWarning on out-of-range
        // truncation (the WARN macro); the stored (truncated) value and the
        // error/return contract are identical here — the warning is elided.
        match ty {
            PY_T_BOOL => {
                let is_true = std::ptr::eq(
                    value,
                    (&raw mut crate::abi_types::Py_True).cast::<PyObject>(),
                );
                let is_false = std::ptr::eq(
                    value,
                    (&raw mut crate::abi_types::Py_False).cast::<PyObject>(),
                );
                if !is_true && !is_false {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_TypeError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"attribute value type must be bool".as_ptr(),
                    );
                    return -1;
                }
                *field.cast::<i8>() = is_true as i8;
                0
            }
            PY_T_BYTE => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *field.cast::<i8>() = v as i8;
                0
            }
            PY_T_UBYTE => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut u8) = v as u8;
                0
            }
            PY_T_SHORT => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut i16) = v as i16;
                0
            }
            PY_T_USHORT => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut u16) = v as u16;
                0
            }
            PY_T_INT => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut i32) = v as i32;
                0
            }
            PY_T_UINT => {
                // CPython accepts negative ints for compatibility (falls back to
                // the signed converter after clearing the OverflowError).
                let mut u = crate::api::numbers::PyLong_AsUnsignedLong(value);
                if u == c_ulong::MAX && err_set() {
                    crate::api::errors::PyErr_Clear();
                    let s = crate::api::numbers::PyLong_AsLong(value);
                    if s == -1 && err_set() {
                        return -1;
                    }
                    u = s as c_ulong;
                }
                *(field as *mut u32) = u as u32;
                0
            }
            PY_T_LONG => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut std::os::raw::c_long) = v;
                0
            }
            PY_T_ULONG => {
                let mut u = crate::api::numbers::PyLong_AsUnsignedLong(value);
                if u == c_ulong::MAX && err_set() {
                    crate::api::errors::PyErr_Clear();
                    let s = crate::api::numbers::PyLong_AsLong(value);
                    if s == -1 && err_set() {
                        return -1;
                    }
                    u = s as c_ulong;
                }
                *(field as *mut std::os::raw::c_ulong) = u;
                0
            }
            PY_T_PYSSIZET => {
                let v = crate::api::numbers::PyLong_AsSsize_t(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut isize) = v;
                0
            }
            PY_T_LONGLONG => {
                let v = crate::api::numbers::PyLong_AsLongLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                // 8-byte member: `field` may be only 4-aligned on a C-minted
                // (wasm32, struct-align-4) object — see PyMember_GetOne's
                // read_unaligned for the same class (a98ef2978e). An aligned
                // write here would be UB (misaligned dereference).
                std::ptr::write_unaligned(field as *mut c_longlong, v);
                0
            }
            PY_T_ULONGLONG => {
                let mut u = crate::api::numbers::PyLong_AsUnsignedLongLong(value);
                if u == c_ulonglong::MAX && err_set() {
                    crate::api::errors::PyErr_Clear();
                    let s = crate::api::numbers::PyLong_AsLongLong(value);
                    if s == -1 && err_set() {
                        return -1;
                    }
                    u = s as c_ulonglong;
                }
                std::ptr::write_unaligned(field as *mut c_ulonglong, u);
                0
            }
            PY_T_FLOAT => {
                let v = crate::api::numbers::PyFloat_AsDouble(value);
                if v == -1.0 && err_set() {
                    return -1;
                }
                *(field as *mut f32) = v as f32;
                0
            }
            PY_T_DOUBLE => {
                let v = crate::api::numbers::PyFloat_AsDouble(value);
                if v == -1.0 && err_set() {
                    return -1;
                }
                // Same 8-byte alignment class as T_LONGLONG/T_ULONGLONG above.
                std::ptr::write_unaligned(field as *mut f64, v);
                0
            }
            PY_T_CHAR => {
                let mut len: Py_ssize_t = 0;
                let s = crate::api::strings::PyUnicode_AsUTF8AndSize(value, &raw mut len);
                if s.is_null() || len != 1 {
                    crate::api::errors::PyErr_BadArgument();
                    return -1;
                }
                *(field as *mut c_char) = *s;
                0
            }
            PY_T_STRING => {
                // T_STRING / T_STRING_INPLACE are readonly (CPython raises here).
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"readonly attribute".as_ptr(),
                );
                -1
            }
            PY_T_OBJECT | PY_T_OBJECT_EX => {
                let slot = field as *mut *mut PyObject;
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
                // Unknown member type: SystemError "bad memberdescr type for %s".
                let name = if (*member).name.is_null() {
                    "?".to_string()
                } else {
                    std::ffi::CStr::from_ptr((*member).name)
                        .to_string_lossy()
                        .into_owned()
                };
                let msg = format!("bad memberdescr type for {name}");
                if let Ok(c) = std::ffi::CString::new(msg) {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
                        c.as_ptr(),
                    );
                }
                -1
            }
        }
    }
}

/// Source-recompiled `Py_TYPE(op)` authority. Managed views report semantic
/// builtin identity while their physical carrier remains `MoltManaged_Type`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_TYPE(op: *mut PyObject) -> *mut PyTypeObject {
    unsafe { crate::bridge::semantic_type(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Type(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PyObject_Type called with NULL".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let tp = unsafe { crate::bridge::semantic_type(op) };
    if tp.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
    let actual = unsafe { crate::bridge::semantic_type(op) };
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
    let tp = unsafe { crate::bridge::semantic_type(op) };
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
    if std::ptr::eq(
        unsafe { (*op).ob_type },
        &raw const crate::abi_types::PyComplex_Type,
    ) {
        return unsafe { complex_hash_from_cval(op) };
    }
    // Molt-native (bridge-managed) objects hash through the runtime hash
    // authority over their handle bits (hash(int) == int, etc.), not tp_hash.
    let native = crate::bridge::GLOBAL_BRIDGE.observed_handle_for_pyobj(op);
    if let Some(value) = native {
        return crate::bridge::molt_hash_from_bits(value.bits());
    }
    // Foreign object: dispatch tp_hash.
    let tp = unsafe { (*op).ob_type };
    if !tp.is_null()
        && let Some(hash_fn) = unsafe { (*tp).tp_hash }
    {
        return unsafe { hash_fn(op) };
    }
    // CPython Objects/object.c: a NULL tp_hash means the object is unhashable —
    // PyObject_HashNotImplemented raises TypeError and returns -1. Never fabricate
    // an identity hash from the pointer (that would make an unhashable object
    // silently hashable and usable as a dict/set key).
    let name = unsafe { object_type_name(op) };
    let msg = format!("unhashable type: '{}'", &name[..name.len().min(200)]);
    if let Ok(cmsg) = std::ffi::CString::new(msg) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                cmsg.as_ptr(),
            );
        }
    }
    -1
}

// ─── PyType subtype / flags / name ────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_IsSubtype(a: *mut PyTypeObject, b: *mut PyTypeObject) -> c_int {
    if a.is_null() || b.is_null() {
        return 0;
    }
    let a_bits = GLOBAL_BRIDGE.molt_handle_for_pyobj(a.cast::<PyObject>());
    let b_bits = GLOBAL_BRIDGE.molt_handle_for_pyobj(b.cast::<PyObject>());
    if let (Some(a_bits), Some(b_bits)) = (a_bits, b_bits) {
        let hooks = crate::hooks::hooks_or_stubs();
        if unsafe { (hooks.classify_heap)(a_bits.bits()) }
            == crate::abi_types::MoltTypeTag::Type as u8
            && unsafe { (hooks.classify_heap)(b_bits.bits()) }
                == crate::abi_types::MoltTypeTag::Type as u8
        {
            return unsafe { (hooks.type_is_subtype)(a_bits.bits(), b_bits.bits()) };
        }
    }
    // CPython Objects/typeobject.c: when `a` has a materialized tp_mro, walk the
    // full MRO tuple (this is what makes MULTIPLE inheritance resolve — numpy's
    // dual-inherit scalar types, e.g. `np.int_` from both `signedinteger` and
    // `int`, are only reachable via the MRO, never the tp_base primary chain).
    let mro = unsafe { (*a).tp_mro };
    if !mro.is_null() {
        let n = unsafe { crate::api::sequences::PyTuple_Size(mro) };
        let mut i: Py_ssize_t = 0;
        while i < n {
            let entry = unsafe { crate::api::sequences::PyTuple_GetItem(mro, i) };
            if std::ptr::eq(entry.cast::<PyTypeObject>(), b) {
                return 1;
            }
            if let Some(secondary) = crate::abi_types::exc_singleton_secondary_parent(entry) {
                let secondary = secondary.cast::<PyTypeObject>();
                if std::ptr::eq(secondary, b) || unsafe { PyType_IsSubtype(secondary, b) } != 0 {
                    return 1;
                }
            }
            i += 1;
        }
        return 0;
    }
    // `a` is not completely initialized (no tp_mro yet): follow the tp_base
    // primary chain, and — matching CPython's type_is_subtype_base_chain — treat
    // every fully-walked type as a subtype of `object` at the chain end.
    let mut cursor = a;
    while !cursor.is_null() {
        if std::ptr::eq(cursor, b) {
            return 1;
        }
        if let Some(secondary) =
            crate::abi_types::exc_singleton_secondary_parent(cursor.cast::<PyObject>())
        {
            let secondary = secondary.cast::<PyTypeObject>();
            if std::ptr::eq(secondary, b) || unsafe { PyType_IsSubtype(secondary, b) } != 0 {
                return 1;
            }
        }
        cursor = unsafe { (*cursor).tp_base };
    }
    std::ptr::eq(b, &raw mut crate::abi_types::PyBaseObject_Type) as c_int
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
    // CPython Objects/typeobject.c type_name -> _PyType_Name: for a non-heap type
    // return only the segment AFTER the last '.' in tp_name (e.g. `BoolDType`,
    // not `numpy.dtypes.BoolDType`). PyType_GetQualName delegates here. Our
    // static/foreign types are all non-heap, so always strip the dotted prefix.
    let bytes = unsafe { std::ffi::CStr::from_ptr(name_ptr) }.to_bytes();
    let short = match bytes.iter().rposition(|&b| b == b'.') {
        Some(dot) => &bytes[dot + 1..],
        None => bytes,
    };
    unsafe {
        crate::api::strings::PyUnicode_FromStringAndSize(
            short.as_ptr().cast(),
            short.len() as isize,
        )
    }
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
    let tp = o.cast::<PyTypeObject>();
    let metatype = unsafe { (*o).ob_type };
    let meta_attribute = unsafe { _PyType_Lookup(metatype, name) };
    if !meta_attribute.is_null() && unsafe { PyDescr_IsData(meta_attribute) } != 0 {
        let descriptor_type = unsafe { (*meta_attribute).ob_type };
        if !descriptor_type.is_null()
            && let Some(get) = unsafe { (*descriptor_type).tp_descr_get }
        {
            return unsafe { get(meta_attribute, o, metatype.cast::<PyObject>()) };
        }
    }

    let attribute = unsafe { _PyType_Lookup(tp, name) };
    if !attribute.is_null() {
        let descriptor_type = unsafe { (*attribute).ob_type };
        if !descriptor_type.is_null()
            && let Some(get) = unsafe { (*descriptor_type).tp_descr_get }
        {
            return unsafe { get(attribute, ptr::null_mut(), o) };
        }
        unsafe { crate::api::refcount::Py_INCREF(attribute) };
        return attribute;
    }

    if !meta_attribute.is_null() {
        let descriptor_type = unsafe { (*meta_attribute).ob_type };
        if !descriptor_type.is_null()
            && let Some(get) = unsafe { (*descriptor_type).tp_descr_get }
        {
            return unsafe { get(meta_attribute, o, metatype.cast::<PyObject>()) };
        }
        unsafe { crate::api::refcount::Py_INCREF(meta_attribute) };
        return meta_attribute;
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

/// Best-effort semantic type name of a live `PyObject*` for diagnostics
/// (mirrors CPython's `Py_TYPE(v)->tp_name`, defaulting to `object`).
unsafe fn object_type_name(op: *mut PyObject) -> String {
    if op.is_null() {
        return "object".to_string();
    }
    let tp = unsafe { crate::bridge::semantic_type(op) };
    if tp.is_null() {
        return "object".to_string();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        return "object".to_string();
    }
    unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// Validate that a `tp_str`/`tp_repr` slot returned an actual `str`, mirroring
/// CPython's `__str__/__repr__ returned non-string (type %.200s)` guard.
/// Consumes `res` on the error path (Py_DECREF) and returns NULL with a
/// pending `TypeError`; otherwise returns `res` unchanged.
unsafe fn check_stringifier_result(res: *mut PyObject, dunder: &str) -> *mut PyObject {
    if res.is_null() {
        // The slot already set the exception — propagate as-is.
        return ptr::null_mut();
    }
    if unsafe { crate::api::strings::PyUnicode_Check(res) } == 0 {
        let mut name = unsafe { object_type_name(res) };
        name.truncate(200);
        unsafe { crate::api::refcount::Py_DECREF(res) };
        let msg = format!("{dunder} returned non-string (type {name})");
        if let Ok(cmsg) = std::ffi::CString::new(msg) {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    cmsg.as_ptr(),
                );
            }
        }
        return ptr::null_mut();
    }
    res
}

/// Materialize the runtime str/repr bytes of a Molt-native (bridge-managed)
/// object into a fresh `str`. Fails closed with `NULL` + `MemoryError` when the
/// string allocation fails (the CPython contract), never a fabricated value.
unsafe fn native_stringify(bits: u64, want_repr: bool) -> *mut PyObject {
    let bytes = if want_repr {
        crate::bridge::molt_repr_string(bits)
    } else {
        crate::bridge::molt_str_string(bits)
    };
    let Some(bytes) = bytes else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"object has no native string representation".as_ptr(),
            );
        }
        return ptr::null_mut();
    };
    // PyUnicode_FromStringAndSize routes through the runtime `alloc_str` hook and
    // sets MemoryError on failure, so the native path stays fail-closed.
    unsafe {
        crate::api::strings::PyUnicode_FromStringAndSize(
            bytes.as_ptr().cast(),
            bytes.len() as isize,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_native_repr(op: *mut PyObject) -> *mut PyObject {
    let native = crate::bridge::GLOBAL_BRIDGE.observed_handle_for_pyobj(op);
    match native {
        Some(value) => unsafe { native_stringify(value.bits(), true) },
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_native_str(op: *mut PyObject) -> *mut PyObject {
    let native = crate::bridge::GLOBAL_BRIDGE.observed_handle_for_pyobj(op);
    match native {
        Some(value) => unsafe { native_stringify(value.bits(), false) },
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Repr(op: *mut PyObject) -> *mut PyObject {
    // CPython Objects/object.c PyObject_Repr: NULL -> "<NULL>".
    if op.is_null() {
        return unsafe { crate::api::strings::PyUnicode_FromString(c"<NULL>".as_ptr()) };
    }
    let tp = unsafe { crate::bridge::semantic_type(op) };
    if !tp.is_null()
        && let Some(reprfunc) = unsafe { (*tp).tp_repr }
    {
        if unsafe {
            crate::api::memory::Py_EnterRecursiveCall(
                c" while getting the repr of an object".as_ptr(),
            )
        } != 0
        {
            return ptr::null_mut();
        }
        let res = unsafe { reprfunc(op) };
        unsafe { crate::api::memory::Py_LeaveRecursiveCall() };
        return unsafe { check_stringifier_result(res, "__repr__") };
    }
    let name = unsafe { object_type_name(op) };
    let rendered = format!("<{name} object at {op:p}>");
    match std::ffi::CString::new(rendered) {
        Ok(c) => unsafe { crate::api::strings::PyUnicode_FromString(c.as_ptr()) },
        Err(_) => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Str(op: *mut PyObject) -> *mut PyObject {
    // CPython Objects/object.c PyObject_Str: NULL -> "<NULL>".
    if op.is_null() {
        return unsafe { crate::api::strings::PyUnicode_FromString(c"<NULL>".as_ptr()) };
    }
    // Exact-str fast path: str(s) is s (identity, incref) — CPython
    // PyUnicode_CheckExact branch. Generic managed strings keep an honest
    // physical carrier, so exactness is resolved through semantic Py_TYPE.
    let tp = unsafe { crate::bridge::semantic_type(op) };
    if tp == &raw mut crate::abi_types::PyUnicode_Type {
        unsafe { crate::api::refcount::Py_INCREF(op) };
        return op;
    }
    if !tp.is_null()
        && let Some(strfunc) = unsafe { (*tp).tp_str }
    {
        if unsafe {
            crate::api::memory::Py_EnterRecursiveCall(
                c" while getting the str of an object".as_ptr(),
            )
        } != 0
        {
            return ptr::null_mut();
        }
        let res = unsafe { strfunc(op) };
        unsafe { crate::api::memory::Py_LeaveRecursiveCall() };
        return unsafe { check_stringifier_result(res, "__str__") };
    }
    unsafe { PyObject_Repr(op) }
}

// Comparison opcodes (CPython Include/object.h): Py_LT..Py_GE = 0..5.
const CMP_LT: c_int = 0;
const CMP_LE: c_int = 1;
const CMP_EQ: c_int = 2;
const CMP_NE: c_int = 3;
const CMP_GT: c_int = 4;
const CMP_GE: c_int = 5;

/// `_Py_SwappedOp[op]` — the reflected comparison operator.
#[inline]
fn swapped_op(op: c_int) -> c_int {
    match op {
        CMP_LT => CMP_GT,
        CMP_LE => CMP_GE,
        CMP_GT => CMP_LT,
        CMP_GE => CMP_LE,
        other => other, // EQ/NE are self-reflected
    }
}

#[inline]
fn cmp_opstring(op: c_int) -> &'static str {
    match op {
        CMP_LT => "<",
        CMP_LE => "<=",
        CMP_EQ => "==",
        CMP_NE => "!=",
        CMP_GT => ">",
        CMP_GE => ">=",
        _ => "?",
    }
}

#[inline]
fn is_not_implemented(res: *mut PyObject) -> bool {
    std::ptr::eq(res, &raw mut crate::abi_types::Py_NotImplementedSentinel)
}

/// Call a `tp_richcompare` slot, returning `Some(result)` when the slot exists
/// (NULL result = pending error, NotImplemented = "not handled") or `None` when
/// the type carries no slot.
unsafe fn try_slot_richcompare(
    a: *mut PyObject,
    b: *mut PyObject,
    op: c_int,
) -> Option<*mut PyObject> {
    let tp = unsafe { crate::bridge::semantic_type(a) };
    if tp.is_null() {
        return None;
    }
    let f = unsafe { (*tp).tp_richcompare }?;
    Some(unsafe { f(a, b, op) })
}

#[inline]
fn cmp_bool_result(b: bool) -> *mut PyObject {
    let res = if b {
        (&raw mut crate::abi_types::Py_True).cast::<PyObject>()
    } else {
        (&raw mut crate::abi_types::Py_False).cast::<PyObject>()
    };
    unsafe { crate::api::refcount::Py_INCREF(res) };
    res
}

#[inline]
fn ordering_to_result(ord: std::cmp::Ordering, op: c_int) -> Option<*mut PyObject> {
    use std::cmp::Ordering::*;
    let b = match op {
        CMP_LT => ord == Less,
        CMP_LE => ord != Greater,
        CMP_EQ => ord == Equal,
        CMP_NE => ord != Equal,
        CMP_GT => ord == Greater,
        CMP_GE => ord != Less,
        _ => return None,
    };
    Some(cmp_bool_result(b))
}

/// Value comparison for two Molt-native (bridge-resolvable) operands, playing
/// the role of CPython's `long_richcompare` / `float_richcompare` /
/// `unicode_richcompare` slots (bridge-minted natives carry no tp_richcompare).
/// Returns `None` when this pair is not a natively comparable combination —
/// callers then continue with slot dispatch / NotImplemented resolution.
unsafe fn native_value_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> Option<*mut PyObject> {
    let (vb, wb) = {
        let bridge = &*crate::bridge::GLOBAL_BRIDGE;
        (
            bridge.molt_handle_for_pyobj(v),
            bridge.molt_handle_for_pyobj(w),
        )
    };
    let (vb, wb) = (vb?, wb?);
    let (mv, mw) = (vb.decode(), wb.decode());

    // Numeric pair (inline int / bool exact in i64; float via f64 — inline ints
    // are 47-bit, exact in f64, so a mixed compare loses nothing).
    let as_num = |m: &molt_lang_obj_model::MoltObject| -> Option<(Option<i64>, f64)> {
        if m.is_bool() {
            let i = m.as_bool().unwrap_or(false) as i64;
            Some((Some(i), i as f64))
        } else if m.is_int() {
            let i = m.as_int()?;
            Some((Some(i), i as f64))
        } else if m.is_float() {
            Some((None, m.as_float()?))
        } else {
            None
        }
    };
    if let (Some((vi, vf)), Some((wi, wf))) = (as_num(&mv), as_num(&mw)) {
        // int-vs-int stays exact; any float operand compares as f64.
        let ord = match (vi, wi) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => vf.partial_cmp(&wf)?, // NaN: fall through to slot path
        };
        return ordering_to_result(ord, op);
    }

    // str pair: byte-lexicographic == code-point-lexicographic under UTF-8.
    let str_bytes = |bits: u64| -> Option<Vec<u8>> {
        let h = crate::hooks::hooks_or_stubs();
        let m = molt_lang_obj_model::MoltObject::from_bits(bits);
        if !m.is_ptr() {
            return None;
        }
        if unsafe { (h.classify_heap)(bits) } != crate::abi_types::MoltTypeTag::Str as u8 {
            return None;
        }
        let mut len: usize = 0;
        let p = unsafe { (h.str_data)(bits, &raw mut len) };
        if p.is_null() {
            return Some(Vec::new());
        }
        Some(unsafe { std::slice::from_raw_parts(p, len) }.to_vec())
    };
    if let (Some(a), Some(b)) = (str_bytes(vb.bits()), str_bytes(wb.bits())) {
        return ordering_to_result(a.cmp(&b), op);
    }

    // bytes pair: lexicographic over the raw bytes as UNSIGNED (CPython
    // `Objects/bytesobject.c bytes_richcompare` ordering — `Py_CHARMASK` +
    // `memcmp`, shorter-is-less on a full-prefix tie; `Vec<u8>::cmp` is exactly
    // that). Mirrors the `str_bytes` path above for the `bytes` builtin.
    let bytes_bytes = |bits: u64| -> Option<Vec<u8>> {
        let h = crate::hooks::hooks_or_stubs();
        let m = molt_lang_obj_model::MoltObject::from_bits(bits);
        if !m.is_ptr() {
            return None;
        }
        if unsafe { (h.classify_heap)(bits) } != crate::abi_types::MoltTypeTag::Bytes as u8 {
            return None;
        }
        let mut len: usize = 0;
        let p = unsafe { (h.bytes_data)(bits, &raw mut len) };
        if p.is_null() {
            return Some(Vec::new());
        }
        Some(unsafe { std::slice::from_raw_parts(p, len) }.to_vec())
    };
    if let (Some(a), Some(b)) = (bytes_bytes(vb.bits()), bytes_bytes(wb.bits())) {
        return ordering_to_result(a.cmp(&b), op);
    }

    // Same handle bits: identity implies equality for EQ/NE.
    if vb == wb && (op == CMP_EQ || op == CMP_NE) {
        return Some(cmp_bool_result(op == CMP_EQ));
    }
    None
}

/// Return a new reference to `NotImplemented` — CPython's contract when a
/// `tp_richcompare` slot does not handle the operand pair (the caller then tries
/// the reflected slot / resolves EQ-NE by identity). Mirrors
/// `api::sequences::richcompare_not_implemented`.
#[inline]
unsafe fn richcmp_not_implemented() -> *mut PyObject {
    let ni = &raw mut crate::abi_types::Py_NotImplementedSentinel;
    unsafe { crate::api::refcount::Py_INCREF(ni) };
    ni
}

// ─── Builtin value-type `tp_richcompare` slots (CLASS1-SLOTS) ────────────────
//
// numpy 2.4.2 `DUAL_INHERIT`/`DUAL_INHERIT2` (`multiarraymodule.c:4827-4835`)
// copies `tp_richcompare`/`tp_hash` straight off molt's builtin
// `PyFloat_Type`/`PyComplex_Type`/`PyBytes_Type`/`PyUnicode_Type` (and the
// `Long` scalar inherits `PyLong_Type`'s via the `tp_base` chain at
// `PyType_Ready`). Those slots were NULL (zeroed-shell statics), leaving numpy's
// Double/CDouble/String/Unicode scalar types non-comparable and unhashable and
// breaking `_multiarray_umath` init. These slots close that as a batch, mirroring
// the landed `molt_tuple_richcompare`: each guards the concrete builtin type and
// routes value comparison through the single `native_value_richcompare`
// authority (so `do_richcompare`'s fast path and the slot never drift), deferring
// with `NotImplemented` for cross-type pairs exactly where CPython does.

/// CPython `Objects/longobject.c` `long_richcompare` (:3312). `CHECK_BINOP`: if
/// either operand is not a `PyLong` → `NotImplemented` (int-vs-float defers to
/// float's reflected slot). Value order via the runtime int authority.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_long_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if unsafe { crate::api::numbers::PyLong_Check(v) } == 0
        || unsafe { crate::api::numbers::PyLong_Check(w) } == 0
    {
        return unsafe { richcmp_not_implemented() };
    }
    match unsafe { native_value_richcompare(v, w, op) } {
        Some(res) => res,
        None => unsafe { richcmp_not_implemented() },
    }
}

/// CPython `Objects/floatobject.c` `float_richcompare` (:417). `float` compares
/// with `float` and (exactly) with `int`; any other `w` → `NotImplemented`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_float_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if unsafe { crate::api::numbers::PyFloat_Check(v) } == 0 {
        return unsafe { richcmp_not_implemented() };
    }
    if unsafe { crate::api::numbers::PyFloat_Check(w) } == 0
        && unsafe { crate::api::numbers::PyLong_Check(w) } == 0
    {
        return unsafe { richcmp_not_implemented() };
    }
    match unsafe { native_value_richcompare(v, w, op) } {
        Some(res) => res,
        None => unsafe { richcmp_not_implemented() },
    }
}

/// CPython `Objects/unicodeobject.c` `PyUnicode_RichCompare` (:10952). Non-`str`
/// operand → `NotImplemented`; else lexicographic by code point (UTF-8 byte
/// order == code-point order).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_str_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if unsafe { crate::api::strings::PyUnicode_Check(v) } == 0
        || unsafe { crate::api::strings::PyUnicode_Check(w) } == 0
    {
        return unsafe { richcmp_not_implemented() };
    }
    match unsafe { native_value_richcompare(v, w, op) } {
        Some(res) => res,
        None => unsafe { richcmp_not_implemented() },
    }
}

/// CPython `Objects/bytesobject.c` `bytes_richcompare` (:1544). Non-`bytes`
/// operand → `NotImplemented`; else lexicographic over the raw unsigned bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytes_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if unsafe { crate::api::strings::PyBytes_Check(v) } == 0
        || unsafe { crate::api::strings::PyBytes_Check(w) } == 0
    {
        return unsafe { richcmp_not_implemented() };
    }
    match unsafe { native_value_richcompare(v, w, op) } {
        Some(res) => res,
        None => unsafe { richcmp_not_implemented() },
    }
}

/// CPython `Objects/complexobject.c` `complex_richcompare` (:582). `complex`
/// supports ONLY `==`/`!=` (ordering → `NotImplemented` → TypeError). Self-
/// contained (molt complex is a C-layout `PyComplexObject`, not a bridge handle):
/// vs `int` with zero imag defers to `float(real)`-vs-int; vs `float` equal iff
/// `imag==0 && real==w`; vs `complex` both parts match exactly; else
/// `NotImplemented`. Faithful to numpy's `CDouble` DUAL_INHERIT layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_complex_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if op != CMP_EQ && op != CMP_NE {
        return unsafe { richcmp_not_implemented() };
    }
    if unsafe { crate::api::numbers::PyComplex_Check(v) } == 0 {
        return unsafe { richcmp_not_implemented() };
    }
    let i = unsafe { crate::api::numbers::PyComplex_AsCComplex(v) };
    let equal: bool;
    if unsafe { crate::api::numbers::PyLong_Check(w) } != 0 {
        if i.imag == 0.0 {
            // Defer to `float(real) <op> int` so the exact float/int comparison
            // (and its NotImplemented rules) applies. Matches CPython.
            let j = unsafe { crate::api::numbers::PyFloat_FromDouble(i.real) };
            if j.is_null() {
                return ptr::null_mut();
            }
            let sub = unsafe { PyObject_RichCompare(j, w, op) };
            unsafe { crate::api::refcount::Py_DECREF(j) };
            return sub;
        }
        equal = false;
    } else if unsafe { crate::api::numbers::PyFloat_Check(w) } != 0 {
        let wd = unsafe { crate::api::numbers::PyFloat_AsDouble(w) };
        equal = i.real == wd && i.imag == 0.0;
    } else if unsafe { crate::api::numbers::PyComplex_Check(w) } != 0 {
        let j = unsafe { crate::api::numbers::PyComplex_AsCComplex(w) };
        equal = i.real == j.real && i.imag == j.imag;
    } else {
        return unsafe { richcmp_not_implemented() };
    }
    cmp_bool_result(equal == (op == CMP_EQ))
}

// ─── Builtin value-type `tp_hash` slot (CLASS1-SLOTS) ────────────────────────

/// Read `ob_fval` off a foreign object whose type is `float`-layout-compatible
/// (CPython `PyFloatObject = {PyObject_HEAD; double}`; `np.float64` shares it —
/// which is exactly why numpy DUAL_INHERITs `PyFloat_Type`'s slots onto its
/// Double scalar). Unaligned: a statically C-minted object may be pointer- (4-byte
/// on wasm32) aligned while the `double` wants 8 — same UB class the complex
/// readers guard.
#[inline]
unsafe fn read_foreign_ob_fval(op: *mut PyObject) -> f64 {
    let field = unsafe { (op as *const u8).add(std::mem::size_of::<PyObject>()) as *const f64 };
    unsafe { std::ptr::read_unaligned(field) }
}

/// CPython `Objects/complexobject.c` `complex_hash` (:405):
/// `hash(z) = hash(z.real) + _PyHASH_IMAG * hash(z.imag)` in wrapping unsigned
/// arithmetic, `-1 → -2`. `_PyHASH_IMAG = 1000003` (`Include/pyhash.h`). Each part
/// hashes through the runtime float-hash authority (`molt_hash_from_bits`), so
/// when `imag == 0` the result is exactly `hash(float real)` — preserving the
/// cross-type invariant `hash(x+0j) == hash(x)` within molt.
#[inline]
unsafe fn complex_hash_from_cval(op: *mut PyObject) -> isize {
    let cval = unsafe { crate::api::numbers::PyComplex_AsCComplex(op) };
    let part_hash = |d: f64| -> isize {
        crate::bridge::molt_hash_from_bits(molt_lang_obj_model::MoltObject::from_float(d).bits())
    };
    const PY_HASH_IMAG: usize = 1000003;
    let hr = part_hash(cval.real) as usize;
    let hi = part_hash(cval.imag) as usize;
    let combined = hr.wrapping_add(PY_HASH_IMAG.wrapping_mul(hi)) as isize;
    if combined == -1 { -2 } else { combined }
}

/// Set the CPython `unhashable type: '<name>'` TypeError and return the `-1`
/// error sentinel — the `PyObject_HashNotImplemented` contract. Used when a
/// foreign object reaches a copied builtin hash slot but has no molt-native
/// handle and no known-compatible C layout (never fabricate an identity hash).
#[inline]
unsafe fn hash_not_implemented(op: *mut PyObject) -> isize {
    let name = unsafe { object_type_name(op) };
    let msg = format!("unhashable type: '{}'", &name[..name.len().min(200)]);
    if let Ok(cmsg) = std::ffi::CString::new(msg) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                cmsg.as_ptr(),
            );
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_HashNotImplemented(op: *mut PyObject) -> isize {
    unsafe { hash_not_implemented(op) }
}

/// Generic `tp_hash` slot for the builtin value types (int/bool/float/str/bytes/
/// complex). numpy DUAL_INHERIT copies these off molt's statics; a NULL slot
/// leaves numpy's scalars unhashable and breaks init. Routes a molt-native value
/// through `bridge::molt_hash_from_bits` (the same authority `PyObject_Hash`
/// uses — `hash(int)==int`, consistent, no drift); a foreign `complex`/`float`
/// through its layout-compatible C struct; else honest `unhashable` TypeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_generic_hash(op: *mut PyObject) -> isize {
    if op.is_null() {
        return -1;
    }
    if std::ptr::eq(
        unsafe { (*op).ob_type },
        &raw const crate::abi_types::PyComplex_Type,
    ) {
        return unsafe { complex_hash_from_cval(op) };
    }
    // Molt-native value. Decode-safe converter excludes a raw-registered foreign
    // object's `0xA11C` identity anchor (Class-2 mis-decode), so it is NEVER
    // hashed as a garbage float. Resolve then drop the bridge lock before hashing.
    let native = crate::bridge::GLOBAL_BRIDGE.observed_handle_for_pyobj(op);
    if let Some(bits) = native {
        return crate::bridge::molt_hash_from_bits(bits.bits());
    }
    // Foreign object carrying a copied builtin hash slot. complex and float have
    // CPython-defined layouts numpy's CDouble/Double scalars share.
    if unsafe { crate::api::numbers::PyComplex_Check(op) } != 0 {
        return unsafe { complex_hash_from_cval(op) };
    }
    if unsafe { crate::api::numbers::PyFloat_Check(op) } != 0 {
        let d = unsafe { read_foreign_ob_fval(op) };
        return crate::bridge::molt_hash_from_bits(
            molt_lang_obj_model::MoltObject::from_float(d).bits(),
        );
    }
    // A TYPE object (class) reaching this value-type hash slot — a numpy
    // metatype inherits/copies molt_generic_hash via DUAL_INHERIT, so hashing a
    // DType CLASS during numpy.dtypes registration lands here — is hashable by
    // IDENTITY. Types are always hashable in CPython (`object.__hash__` =
    // _Py_HashPointer); this is the genuine identity hash for a type object, NOT
    // the forbidden fabrication for an unhashable INSTANCE (non-type foreign
    // objects still fall through to the honest hash_not_implemented below).
    if unsafe { PyType_Check(op) } != 0 {
        return unsafe { molt_type_identity_hash(op) };
    }
    unsafe { hash_not_implemented(op) }
}

/// `tp_hash` for `type` objects (the metatype). CPython's `type` inherits
/// `object.__hash__`, i.e. `_Py_HashPointer`: a CLASS is hashable by its
/// identity (address). `numpy.dtypes` registration hashes its DType CLASSES
/// into a dict during `_multiarray_umath` `Py_mod_exec`; a NULL `tp_hash` on
/// `PyType_Type` reports the class "unhashable type: 'type'" and aborts init.
/// This is the genuine CPython identity hash for type objects — NOT the
/// forbidden fabrication of an identity hash for an unhashable INSTANCE (that
/// stays a hard `hash_not_implemented`). numpy's `_DTypeMeta` (tp_base =
/// &PyType_Type) inherits this slot via PyType_Ready, matching CPython.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_type_identity_hash(op: *mut PyObject) -> isize {
    if op.is_null() {
        return -1;
    }
    // CPython `Python/pyhash.c` `_Py_HashPointer`: rotate the address right by 4
    // (usize::BITS matches SIZEOF_VOID_P*8 on both wasm32 and native), then map
    // the -1 error sentinel to -2. Stable per-object; never the -1 error value.
    let p = op as usize;
    let x = (p >> 4) | (p << (usize::BITS - 4));
    let h = x as isize;
    if h == -1 { -2 } else { h }
}

/// Faithful port of CPython `Objects/object.c` `do_richcompare`: reflected
/// (subtype-priority) slot first, then v's slot, then w's; NULL propagates as an
/// error; a both-NotImplemented result resolves EQ/NE by identity and raises
/// TypeError for ordering — never leaks NotImplemented to the caller.
unsafe fn do_richcompare(v: *mut PyObject, w: *mut PyObject, op: c_int) -> *mut PyObject {
    // Molt-native value comparison (the natives' "type slots").
    if let Some(res) = unsafe { native_value_richcompare(v, w, op) } {
        return res;
    }
    // Slot dispatch follows semantic `Py_TYPE`, not the physical carrier.
    // Generic managed views deliberately use `MoltManaged_Type` so C code can
    // never read a list/dict/string layout that is not present. The semantic
    // resolver preserves the corresponding builtin type identity and slots.
    let tv = unsafe { crate::bridge::semantic_type(v) };
    let tw = unsafe { crate::bridge::semantic_type(w) };
    let mut checked_reverse = false;

    // Reflected op on w first when Py_TYPE(w) is a PROPER subtype of Py_TYPE(v).
    if !std::ptr::eq(tv, tw)
        && unsafe { PyType_IsSubtype(tw, tv) } == 1
        && let Some(res) = unsafe { try_slot_richcompare(w, v, swapped_op(op)) }
    {
        checked_reverse = true;
        if res.is_null() {
            return ptr::null_mut();
        }
        if !is_not_implemented(res) {
            return res;
        }
        unsafe { crate::api::refcount::Py_DECREF(res) };
    }
    // v's own slot.
    if let Some(res) = unsafe { try_slot_richcompare(v, w, op) } {
        if res.is_null() {
            return ptr::null_mut();
        }
        if !is_not_implemented(res) {
            return res;
        }
        unsafe { crate::api::refcount::Py_DECREF(res) };
    }
    // w's slot (unless already tried as the reflected op above).
    if !checked_reverse && let Some(res) = unsafe { try_slot_richcompare(w, v, swapped_op(op)) } {
        if res.is_null() {
            return ptr::null_mut();
        }
        if !is_not_implemented(res) {
            return res;
        }
        unsafe { crate::api::refcount::Py_DECREF(res) };
    }
    // Neither side handled it: identity for EQ/NE, TypeError for ordering.
    match op {
        CMP_EQ | CMP_NE => {
            let equal = std::ptr::eq(v, w);
            let want = if op == CMP_EQ { equal } else { !equal };
            let res = if want {
                (&raw mut crate::abi_types::Py_True).cast::<PyObject>()
            } else {
                (&raw mut crate::abi_types::Py_False).cast::<PyObject>()
            };
            unsafe { crate::api::refcount::Py_INCREF(res) };
            res
        }
        _ => {
            let msg = format!(
                "'{}' not supported between instances of '{}' and '{}'",
                cmp_opstring(op),
                {
                    let mut n = unsafe { object_type_name(v) };
                    n.truncate(100);
                    n
                },
                {
                    let mut n = unsafe { object_type_name(w) };
                    n.truncate(100);
                    n
                },
            );
            if let Ok(c) = std::ffi::CString::new(msg) {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_TypeError)
                            .cast::<crate::abi_types::PyObject>(),
                        c.as_ptr(),
                    );
                }
            }
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_RichCompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    // CPython PyObject_RichCompare: a NULL operand is a BadInternalCall.
    if v.is_null() || w.is_null() {
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
        }
        return ptr::null_mut();
    }
    unsafe { do_richcompare(v, w, op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_RichCompareBool(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> c_int {
    // CPython Objects/object.c: identity implies equality — v == w shortcuts
    // EQ->1 / NE->0 BEFORE any slot dispatch (so [nan] == [nan] is True).
    if std::ptr::eq(v, w) {
        if op == CMP_EQ {
            return 1;
        } else if op == CMP_NE {
            return 0;
        }
    }
    let res = unsafe { PyObject_RichCompare(v, w, op) };
    if res.is_null() {
        return -1;
    }
    // PyBool_Check fast path, else route the result through PyObject_IsTrue.
    let ok = if std::ptr::eq(res, (&raw mut crate::abi_types::Py_True).cast::<PyObject>()) {
        1
    } else if std::ptr::eq(
        res,
        (&raw mut crate::abi_types::Py_False).cast::<PyObject>(),
    ) {
        0
    } else {
        unsafe { crate::api::object::PyObject_IsTrue(res) }
    };
    unsafe { crate::api::refcount::Py_DECREF(res) };
    ok
}

#[cfg(test)]
mod class2_decode_tests {
    use super::*;
    use crate::abi_types::{PyNumberMethods, PyTypeObject};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static HASH_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REPR_CALLS: AtomicUsize = AtomicUsize::new(0);
    static FLOAT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RICHCOMPARE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static BOOL_CALLS: AtomicUsize = AtomicUsize::new(0);

    static mut REPR_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };

    unsafe extern "C" fn foreign_hash(_op: *mut PyObject) -> isize {
        HASH_CALLS.fetch_add(1, Ordering::SeqCst);
        4242
    }

    unsafe extern "C" fn foreign_repr(_op: *mut PyObject) -> *mut PyObject {
        REPR_CALLS.fetch_add(1, Ordering::SeqCst);
        &raw mut REPR_RESULT
    }

    unsafe extern "C" fn foreign_float(_op: *mut PyObject) -> *mut PyObject {
        FLOAT_CALLS.fetch_add(1, Ordering::SeqCst);
        unsafe { crate::api::numbers::PyFloat_FromDouble(42.5) }
    }

    unsafe extern "C" fn foreign_richcompare(
        _left: *mut PyObject,
        _right: *mut PyObject,
        _op: c_int,
    ) -> *mut PyObject {
        RICHCOMPARE_CALLS.fetch_add(1, Ordering::SeqCst);
        unsafe { crate::api::object::Py_NewRef((&raw mut crate::abi_types::Py_True).cast()) }
    }

    unsafe extern "C" fn foreign_bool(_op: *mut PyObject) -> c_int {
        BOOL_CALLS.fetch_add(1, Ordering::SeqCst);
        0
    }

    unsafe fn release_slot_table(ty: &mut PyTypeObject, wrapper: SlotWrapper) {
        unsafe {
            match wrapper {
                SlotWrapper::Direct(_) => {}
                SlotWrapper::Number(_) => {
                    drop(Box::from_raw(
                        ty.tp_as_number.cast::<crate::abi_types::PyNumberMethods>(),
                    ));
                    ty.tp_as_number = ptr::null_mut();
                }
                SlotWrapper::Sequence(_) => {
                    drop(Box::from_raw(
                        ty.tp_as_sequence
                            .cast::<crate::abi_types::PySequenceMethods>(),
                    ));
                    ty.tp_as_sequence = ptr::null_mut();
                }
                SlotWrapper::Mapping(_) => {
                    drop(Box::from_raw(
                        ty.tp_as_mapping
                            .cast::<crate::abi_types::PyMappingMethods>(),
                    ));
                    ty.tp_as_mapping = ptr::null_mut();
                }
                SlotWrapper::Async(_) => {
                    drop(Box::from_raw(
                        ty.tp_as_async.cast::<crate::abi_types::PyAsyncMethods>(),
                    ));
                    ty.tp_as_async = ptr::null_mut();
                }
                SlotWrapper::Buffer(_) => {
                    drop(Box::from_raw(
                        ty.tp_as_buffer.cast::<crate::abi_types::PyBufferProcs>(),
                    ));
                    ty.tp_as_buffer = ptr::null_mut();
                }
            }
        }
    }

    #[test]
    fn every_stable_slot_has_one_symmetric_storage_authority() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        crate::bridge::molt_cpython_abi_init();
        for id in 1..=81 {
            unsafe { crate::api::errors::PyErr_Clear() };
            let wrapper = stable_slot_wrapper(id).expect("all public slot ids are mapped");
            let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };

            // A valid but unset slot returns NULL without manufacturing an error,
            // including a protocol slot whose parent table does not yet exist.
            assert!(unsafe { PyType_GetSlot(&raw mut ty, id) }.is_null());
            assert!(unsafe { crate::api::errors::PyErr_Occurred() }.is_null());

            let storage = unsafe { slot_wrapper_storage(&raw mut ty, wrapper, true) };
            assert!(!storage.is_null());
            let sentinel = std::ptr::without_provenance_mut::<c_void>(0x1000 + id as usize * 16);
            unsafe { storage.write(sentinel) };
            assert_eq!(unsafe { PyType_GetSlot(&raw mut ty, id) }, sentinel);
            assert!(unsafe { crate::api::errors::PyErr_Occurred() }.is_null());

            unsafe { release_slot_table(&mut ty, wrapper) };
        }
        for id in [-1, 0, 82, i32::MAX] {
            unsafe { crate::api::errors::PyErr_Clear() };
            let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
            assert!(unsafe { PyType_GetSlot(&raw mut ty, id) }.is_null());
            assert!(!unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
        }
        unsafe { crate::api::errors::PyErr_Clear() };
    }

    #[test]
    fn raw_registered_foreign_object_never_decodes_as_molt_value() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        crate::bridge::molt_cpython_abi_init();
        HASH_CALLS.store(0, Ordering::SeqCst);
        REPR_CALLS.store(0, Ordering::SeqCst);
        FLOAT_CALLS.store(0, Ordering::SeqCst);
        RICHCOMPARE_CALLS.store(0, Ordering::SeqCst);
        BOOL_CALLS.store(0, Ordering::SeqCst);

        let mut number: PyNumberMethods = unsafe { std::mem::zeroed() };
        number.nb_float = foreign_float as *mut c_void;
        number.nb_bool = foreign_bool as *mut c_void;
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_name = c"numpy_like_foreign".as_ptr();
        ty.tp_hash = Some(foreign_hash);
        ty.tp_repr = Some(foreign_repr);
        ty.tp_richcompare = Some(foreign_richcompare);
        ty.tp_as_number = (&raw mut number).cast();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        unsafe {
            REPR_RESULT.ob_type = &raw mut crate::abi_types::PyUnicode_Type;
            crate::bridge::GLOBAL_BRIDGE.register_foreign_pyobj(&raw mut obj);
        }

        assert_eq!(unsafe { PyObject_Hash(&raw mut obj) }, 4242);
        assert_eq!(HASH_CALLS.load(Ordering::SeqCst), 1);

        assert_eq!(unsafe { PyObject_Repr(&raw mut obj) }, &raw mut REPR_RESULT);
        assert_eq!(REPR_CALLS.load(Ordering::SeqCst), 1);

        assert_eq!(
            unsafe { crate::api::numbers::PyFloat_AsDouble(&raw mut obj) },
            42.5
        );
        assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);

        assert!(unsafe { native_value_richcompare(&raw mut obj, &raw mut obj, CMP_EQ) }.is_none());
        assert_eq!(
            unsafe { PyObject_RichCompare(&raw mut obj, &raw mut obj, CMP_EQ) },
            (&raw mut crate::abi_types::Py_True).cast()
        );
        assert_eq!(RICHCOMPARE_CALLS.load(Ordering::SeqCst), 1);

        assert_eq!(
            unsafe { crate::api::object::PyObject_IsTrue(&raw mut obj) },
            0
        );
        assert_eq!(BOOL_CALLS.load(Ordering::SeqCst), 1);

        assert_eq!(
            crate::bridge::GLOBAL_BRIDGE.release_pyobj(&raw mut obj),
            crate::bridge::PyObjRelease::Untracked
        );
    }
}

#[cfg(test)]
mod subclass_registry_tests {
    use super::*;
    use crate::abi_types::{
        Py_TPFLAGS_VALID_VERSION_TAG, PyTuple_Type, PyTupleObject, PyVarObject,
    };

    #[repr(C)]
    struct RawTuple2 {
        base: PyTupleObject,
        second: *mut PyObject,
    }

    fn raw_tuple(items: &[*mut PyTypeObject]) -> RawTuple2 {
        assert!(!items.is_empty() && items.len() <= 2);
        RawTuple2 {
            base: PyTupleObject {
                ob_base: PyVarObject {
                    ob_base: PyObject {
                        ob_refcnt: 1,
                        ob_type: &raw mut PyTuple_Type,
                    },
                    ob_size: items.len() as Py_ssize_t,
                },
                ob_item: [items[0].cast()],
            },
            second: items.get(1).copied().unwrap_or(ptr::null_mut()).cast(),
        }
    }

    fn blank_type(refcnt: isize) -> Box<PyTypeObject> {
        let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
        ty.ob_base.ob_base.ob_refcnt = refcnt;
        ty.tp_flags = Py_TPFLAGS_READY | Py_TPFLAGS_VALID_VERSION_TAG;
        ty.tp_version_tag = 41;
        ty
    }

    #[test]
    fn subclass_registry_is_non_owning_and_address_reuse_gets_new_generation() {
        let mut base = blank_type(17);
        let mut child = blank_type(23);
        let base_ptr = &raw mut *base;
        let child_ptr = &raw mut *child;

        unsafe { register_subclass(base_ptr, child_ptr) };
        assert_eq!(base.ob_base.ob_base.ob_refcnt, 17);
        assert_eq!(child.ob_base.ob_base.ob_refcnt, 23);
        let old_identity = {
            let registry = TYPE_SUBCLASSES.lock();
            TypeIdentity {
                address: child_ptr.addr(),
                generation: registry.live[&child_ptr.addr()],
            }
        };

        unregister_type_address(child_ptr.addr());
        let new_identity = {
            let mut registry = TYPE_SUBCLASSES.lock();
            type_identity(&mut registry, child_ptr).expect("re-registered type identity")
        };

        assert_ne!(new_identity, old_identity);
        assert_eq!(base.ob_base.ob_base.ob_refcnt, 17);
        assert_eq!(child.ob_base.ob_base.ob_refcnt, 23);
        unregister_type_address(child_ptr.addr());
        unregister_type_address(base_ptr.addr());
    }

    #[test]
    fn dead_heavy_subclass_registry_compacts_in_one_linear_pass() {
        const WIDTH: usize = 2048;
        let mut base = blank_type(1);
        let base_ptr = &raw mut *base;
        let mut children: Vec<_> = (0..WIDTH).map(|_| blank_type(1)).collect();
        for child in &mut children {
            unsafe { register_subclass(base_ptr, &raw mut **child) };
        }
        for child in children.iter_mut().step_by(2) {
            unregister_type_address((&raw mut **child).addr());
        }

        unsafe { PyType_Modified(base_ptr) };

        for (index, child) in children.iter().enumerate() {
            let is_valid = child.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG != 0;
            assert_eq!(is_valid, index % 2 == 0);
        }
        let base_identity = {
            let registry = TYPE_SUBCLASSES.lock();
            TypeIdentity {
                address: base_ptr.addr(),
                generation: registry.live[&base_ptr.addr()],
            }
        };
        assert_eq!(
            TYPE_SUBCLASSES.lock().subclasses[&base_identity]
                .order
                .len(),
            WIDTH / 2
        );
        for child in &mut children {
            unregister_type_address((&raw mut **child).addr());
        }
        unregister_type_address(base_ptr.addr());
    }

    #[test]
    fn deep_hierarchy_invalidation_is_iterative_in_registry_width() {
        const DEPTH: usize = 16_384;
        let mut types: Vec<_> = (0..DEPTH).map(|_| blank_type(1)).collect();
        for index in 1..types.len() {
            let base = &raw mut *types[index - 1];
            let child = &raw mut *types[index];
            unsafe { register_subclass(base, child) };
        }
        unsafe { PyType_Modified(&raw mut *types[0]) };
        assert!(
            types
                .iter()
                .all(|ty| ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG == 0)
        );
        for ty in &mut types {
            unregister_type_address((&raw mut **ty).addr());
        }
    }

    #[test]
    fn concurrent_subclass_registration_has_one_edge_per_child() {
        const WIDTH: usize = 1024;
        const THREADS: usize = 8;
        let mut base = blank_type(1);
        let base_address = (&raw mut *base).addr();
        let mut children: Vec<_> = (0..WIDTH).map(|_| blank_type(1)).collect();
        let child_addresses: Vec<_> = children
            .iter_mut()
            .map(|child| (&raw mut **child).addr())
            .collect();
        std::thread::scope(|scope| {
            for shard in 0..THREADS {
                let addresses = &child_addresses;
                scope.spawn(move || {
                    for child_address in addresses.iter().skip(shard).step_by(THREADS) {
                        unsafe {
                            register_subclass(
                                ptr::with_exposed_provenance_mut(base_address),
                                ptr::with_exposed_provenance_mut(*child_address),
                            )
                        };
                    }
                });
            }
        });
        let registry = TYPE_SUBCLASSES.lock();
        let base_identity = TypeIdentity {
            address: base_address,
            generation: registry.live[&base_address],
        };
        assert_eq!(registry.subclasses[&base_identity].order.len(), WIDTH);
        drop(registry);
        for address in child_addresses {
            unregister_type_address(address);
        }
        unregister_type_address(base_address);
    }

    #[test]
    fn version_tags_accept_diamonds_reject_cycles_and_never_wrap() {
        let mut root = blank_type(1);
        let mut left = blank_type(1);
        let mut right = blank_type(1);
        let mut diamond = blank_type(1);
        for ty in [&mut root, &mut left, &mut right, &mut diamond] {
            ty.tp_flags &= !Py_TPFLAGS_VALID_VERSION_TAG;
            ty.tp_version_tag = 0;
        }
        let mut left_bases = raw_tuple(&[&raw mut *root]);
        let mut right_bases = raw_tuple(&[&raw mut *root]);
        let mut diamond_bases = raw_tuple(&[&raw mut *left, &raw mut *right]);
        left.tp_bases = (&raw mut left_bases).cast();
        right.tp_bases = (&raw mut right_bases).cast();
        diamond.tp_bases = (&raw mut diamond_bases).cast();
        assert!(unsafe { assign_type_version_tag(&raw mut *diamond, &mut HashSet::new()) });
        assert!(
            [&root, &left, &right, &diamond]
                .into_iter()
                .all(|ty| ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG != 0)
        );

        let mut cycle_a = blank_type(1);
        let mut cycle_b = blank_type(1);
        cycle_a.tp_flags &= !Py_TPFLAGS_VALID_VERSION_TAG;
        cycle_b.tp_flags &= !Py_TPFLAGS_VALID_VERSION_TAG;
        let mut a_bases = raw_tuple(&[&raw mut *cycle_b]);
        let mut b_bases = raw_tuple(&[&raw mut *cycle_a]);
        cycle_a.tp_bases = (&raw mut a_bases).cast();
        cycle_b.tp_bases = (&raw mut b_bases).cast();
        assert!(!unsafe { assign_type_version_tag(&raw mut *cycle_a, &mut HashSet::new()) });

        let local_counter = AtomicU32::new(u32::MAX);
        assert_eq!(allocate_type_version_tag(&local_counter), None);
        assert_eq!(local_counter.load(Ordering::SeqCst), u32::MAX);
    }

    #[test]
    fn wide_subclass_teardown_is_linear_and_returns_registry_to_baseline() {
        const WIDTH: usize = 8192;
        let mut base = blank_type(1);
        let base_address = (&raw mut *base).addr();
        let mut children: Vec<_> = (0..WIDTH).map(|_| blank_type(1)).collect();
        let addresses: Vec<_> = children
            .iter_mut()
            .map(|child| (&raw mut **child).addr())
            .collect();
        for address in &addresses {
            unsafe {
                register_subclass(
                    ptr::with_exposed_provenance_mut(base_address),
                    ptr::with_exposed_provenance_mut(*address),
                )
            };
        }
        let started = std::time::Instant::now();
        for address in &addresses {
            unregister_type_address(*address);
        }
        let teardown = started.elapsed();
        assert!(
            teardown < std::time::Duration::from_secs(5),
            "8192-wide teardown exceeded linear receipt: {teardown:?}"
        );
        let registry = TYPE_SUBCLASSES.lock();
        assert!(
            addresses
                .iter()
                .all(|address| !registry.live.contains_key(address))
        );
        drop(registry);
        unregister_type_address(base_address);
    }
}
