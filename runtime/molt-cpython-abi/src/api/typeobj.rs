//! Type object API — PyType_Ready, PyType_GenericAlloc, Py_TYPE checks.

use crate::abi_types::{
    Py_TPFLAGS_HAVE_GC, Py_TPFLAGS_READY, Py_ssize_t, PyMethodDef, PyObject, PyType_Spec,
    PyTypeObject,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_longlong, c_ulong, c_ulonglong};
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
            crate::bridge::GLOBAL_BRIDGE.register_raw_pyobj(tp.cast::<PyObject>());
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
        crate::bridge::GLOBAL_BRIDGE.register_raw_pyobj(tp.cast::<PyObject>());
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
                        &raw mut crate::abi_types::PyExc_SystemError,
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
    Repr,
    Hash,
    Call,
    Str,
    GetAttr,
    SetAttr,
    RichCompare,
    Iter,
    IterNext,
    DescrGet,
    DescrSet,
    Init,
    Finalize,
}

#[derive(Clone, Copy)]
enum NumberSlot {
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

unsafe fn direct_slot_ptr(tp: *mut PyTypeObject, slot: DirectSlot) -> *mut c_void {
    macro_rules! fn_ptr {
        ($field:ident) => {
            (*tp)
                .$field
                .map_or(ptr::null_mut(), |f| f as *const () as *mut c_void)
        };
    }
    unsafe {
        match slot {
            DirectSlot::Repr => fn_ptr!(tp_repr),
            DirectSlot::Hash => fn_ptr!(tp_hash),
            DirectSlot::Call => fn_ptr!(tp_call),
            DirectSlot::Str => fn_ptr!(tp_str),
            DirectSlot::GetAttr => fn_ptr!(tp_getattro),
            DirectSlot::SetAttr => fn_ptr!(tp_setattro),
            DirectSlot::RichCompare => fn_ptr!(tp_richcompare),
            DirectSlot::Iter => fn_ptr!(tp_iter),
            DirectSlot::IterNext => fn_ptr!(tp_iternext),
            DirectSlot::DescrGet => fn_ptr!(tp_descr_get),
            DirectSlot::DescrSet => fn_ptr!(tp_descr_set),
            DirectSlot::Init => fn_ptr!(tp_init),
            DirectSlot::Finalize => fn_ptr!(tp_finalize),
        }
    }
}

unsafe fn slot_wrapper_ptr(tp: *mut PyTypeObject, slot: SlotWrapper) -> *mut c_void {
    unsafe {
        match slot {
            SlotWrapper::Direct(slot) => direct_slot_ptr(tp, slot),
            SlotWrapper::Number(slot) => {
                let table = (*tp)
                    .tp_as_number
                    .cast::<crate::abi_types::PyNumberMethods>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    NumberSlot::Add => (*table).nb_add,
                    NumberSlot::Subtract => (*table).nb_subtract,
                    NumberSlot::Multiply => (*table).nb_multiply,
                    NumberSlot::Remainder => (*table).nb_remainder,
                    NumberSlot::Power => (*table).nb_power,
                    NumberSlot::Negative => (*table).nb_negative,
                    NumberSlot::Positive => (*table).nb_positive,
                    NumberSlot::Absolute => (*table).nb_absolute,
                    NumberSlot::Bool => (*table).nb_bool,
                    NumberSlot::Invert => (*table).nb_invert,
                    NumberSlot::LShift => (*table).nb_lshift,
                    NumberSlot::RShift => (*table).nb_rshift,
                    NumberSlot::And => (*table).nb_and,
                    NumberSlot::Xor => (*table).nb_xor,
                    NumberSlot::Or => (*table).nb_or,
                    NumberSlot::Int => (*table).nb_int,
                    NumberSlot::Float => (*table).nb_float,
                    NumberSlot::InPlaceAdd => (*table).nb_inplace_add,
                    NumberSlot::InPlaceSubtract => (*table).nb_inplace_subtract,
                    NumberSlot::InPlaceMultiply => (*table).nb_inplace_multiply,
                    NumberSlot::InPlaceRemainder => (*table).nb_inplace_remainder,
                    NumberSlot::InPlacePower => (*table).nb_inplace_power,
                    NumberSlot::InPlaceLShift => (*table).nb_inplace_lshift,
                    NumberSlot::InPlaceRShift => (*table).nb_inplace_rshift,
                    NumberSlot::InPlaceAnd => (*table).nb_inplace_and,
                    NumberSlot::InPlaceXor => (*table).nb_inplace_xor,
                    NumberSlot::InPlaceOr => (*table).nb_inplace_or,
                    NumberSlot::FloorDivide => (*table).nb_floor_divide,
                    NumberSlot::TrueDivide => (*table).nb_true_divide,
                    NumberSlot::InPlaceFloorDivide => (*table).nb_inplace_floor_divide,
                    NumberSlot::InPlaceTrueDivide => (*table).nb_inplace_true_divide,
                    NumberSlot::Index => (*table).nb_index,
                    NumberSlot::MatrixMultiply => (*table).nb_matrix_multiply,
                    NumberSlot::InPlaceMatrixMultiply => (*table).nb_inplace_matrix_multiply,
                }
            }
            SlotWrapper::Sequence(slot) => {
                let table = (*tp)
                    .tp_as_sequence
                    .cast::<crate::abi_types::PySequenceMethods>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    SequenceSlot::Length => (*table).sq_length,
                    SequenceSlot::Concat => (*table).sq_concat,
                    SequenceSlot::Repeat => (*table).sq_repeat,
                    SequenceSlot::Item => (*table).sq_item,
                    SequenceSlot::AssItem => (*table).sq_ass_item,
                    SequenceSlot::Contains => (*table).sq_contains,
                    SequenceSlot::InPlaceConcat => (*table).sq_inplace_concat,
                    SequenceSlot::InPlaceRepeat => (*table).sq_inplace_repeat,
                }
            }
            SlotWrapper::Mapping(slot) => {
                let table = (*tp)
                    .tp_as_mapping
                    .cast::<crate::abi_types::PyMappingMethods>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    MappingSlot::Length => (*table).mp_length,
                    MappingSlot::Subscript => (*table).mp_subscript,
                    MappingSlot::AssSubscript => (*table).mp_ass_subscript,
                }
            }
            SlotWrapper::Async(slot) => {
                let table = (*tp).tp_as_async.cast::<crate::abi_types::PyAsyncMethods>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    AsyncSlot::Await => (*table).am_await,
                    AsyncSlot::Iter => (*table).am_aiter,
                    AsyncSlot::Next => (*table).am_anext,
                }
            }
            SlotWrapper::Buffer(slot) => {
                let table = (*tp).tp_as_buffer.cast::<crate::abi_types::PyBufferProcs>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    BufferSlot::Get => (*table).bf_getbuffer,
                    BufferSlot::Release => (*table).bf_releasebuffer,
                }
            }
        }
    }
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
        let handle = crate::bridge::GLOBAL_BRIDGE.register_raw_pyobj(ptr);
        crate::bridge::GLOBAL_BRIDGE.register_pyobj_for_handle(ptr, handle);
        ptr
    }
}

unsafe fn add_operators_to_dict(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let dict = (*tp).tp_dict;
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
            // The destination type is spelled out explicitly per call site (rather
            // than left for inference) to satisfy clippy's
            // `missing_transmute_annotations`: each `$dest` below is copied
            // verbatim from the corresponding `PyTypeObject` field type in
            // `abi_types.rs`, so a drift between the two would be a visible diff
            // here, not a silent inferred cast.
            macro_rules! set_fn {
                ($field:ident, $dest:ty) => {{
                    (*ty).$field = ::std::mem::transmute::<*mut c_void, $dest>(pfunc);
                }};
            }
            match id {
                // ── tp_* slots ────────────────────────────────────────────────
                ts::Py_tp_alloc => set_fn!(
                    tp_alloc,
                    Option<unsafe extern "C" fn(*mut PyTypeObject, Py_ssize_t) -> *mut PyObject>
                ),
                ts::Py_tp_base => (*ty).tp_base = pfunc.cast::<PyTypeObject>(),
                ts::Py_tp_bases => (*ty).tp_bases = pfunc.cast::<PyObject>(),
                ts::Py_tp_call => set_fn!(
                    tp_call,
                    Option<
                        unsafe extern "C" fn(
                            *mut PyObject,
                            *mut PyObject,
                            *mut PyObject,
                        ) -> *mut PyObject,
                    >
                ),
                ts::Py_tp_clear => set_fn!(
                    tp_clear,
                    Option<unsafe extern "C" fn(*mut PyObject) -> c_int>
                ),
                ts::Py_tp_dealloc => {
                    set_fn!(tp_dealloc, Option<unsafe extern "C" fn(*mut PyObject)>)
                }
                ts::Py_tp_del => set_fn!(tp_del, Option<unsafe extern "C" fn(*mut PyObject)>),
                ts::Py_tp_descr_get => {
                    set_fn!(tp_descr_get, Option<crate::abi_types::PyDescrGetFunc>)
                }
                ts::Py_tp_descr_set => {
                    set_fn!(tp_descr_set, Option<crate::abi_types::PyDescrSetFunc>)
                }
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
                ts::Py_tp_getattr => set_fn!(
                    tp_getattr,
                    Option<unsafe extern "C" fn(*mut PyObject, *const c_char) -> *mut PyObject>
                ),
                ts::Py_tp_getattro => set_fn!(
                    tp_getattro,
                    Option<unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject>
                ),
                ts::Py_tp_hash => set_fn!(
                    tp_hash,
                    Option<unsafe extern "C" fn(*mut PyObject) -> crate::abi_types::Py_hash_t>
                ),
                ts::Py_tp_init => set_fn!(
                    tp_init,
                    Option<
                        unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int,
                    >
                ),
                ts::Py_tp_is_gc => set_fn!(
                    tp_is_gc,
                    Option<unsafe extern "C" fn(*mut PyObject) -> c_int>
                ),
                ts::Py_tp_iter => set_fn!(
                    tp_iter,
                    Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>
                ),
                ts::Py_tp_iternext => set_fn!(
                    tp_iternext,
                    Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>
                ),
                ts::Py_tp_methods => (*ty).tp_methods = pfunc.cast::<PyMethodDef>(),
                ts::Py_tp_new => set_fn!(
                    tp_new,
                    Option<
                        unsafe extern "C" fn(
                            *mut PyTypeObject,
                            *mut PyObject,
                            *mut PyObject,
                        ) -> *mut PyObject,
                    >
                ),
                ts::Py_tp_repr => set_fn!(
                    tp_repr,
                    Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>
                ),
                ts::Py_tp_richcompare => set_fn!(
                    tp_richcompare,
                    Option<
                        unsafe extern "C" fn(*mut PyObject, *mut PyObject, c_int) -> *mut PyObject,
                    >
                ),
                ts::Py_tp_setattr => set_fn!(
                    tp_setattr,
                    Option<
                        unsafe extern "C" fn(*mut PyObject, *const c_char, *mut PyObject) -> c_int,
                    >
                ),
                ts::Py_tp_setattro => set_fn!(
                    tp_setattro,
                    Option<
                        unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int,
                    >
                ),
                ts::Py_tp_str => set_fn!(
                    tp_str,
                    Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>
                ),
                ts::Py_tp_traverse => set_fn!(
                    tp_traverse,
                    Option<unsafe extern "C" fn(*mut PyObject, *mut c_void, *mut c_void) -> c_int>
                ),
                ts::Py_tp_members => (*ty).tp_members = pfunc,
                ts::Py_tp_getset => (*ty).tp_getset = pfunc,
                ts::Py_tp_free => set_fn!(tp_free, Option<unsafe extern "C" fn(*mut c_void)>),
                ts::Py_tp_finalize => {
                    set_fn!(tp_finalize, Option<unsafe extern "C" fn(*mut PyObject)>)
                }
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
            (*tp).tp_bases = bases;
            if (*tp).tp_base.is_null() && crate::api::sequences::PyTuple_GET_SIZE(bases) >= 1 {
                let first = crate::api::sequences::PyTuple_GetItem(bases, 0);
                if !first.is_null() {
                    (*tp).tp_base = first.cast::<PyTypeObject>();
                }
            }
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
                &raw mut crate::abi_types::PyExc_TypeError,
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
                &raw mut crate::abi_types::PyExc_TypeError,
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
        crate::bridge::GLOBAL_BRIDGE.register_raw_pyobj(ptr);
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
        crate::bridge::GLOBAL_BRIDGE.register_raw_pyobj(ptr);
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
        let ty = (*member).type_;
        let field = addr.offset((*member).offset);
        // CPython Python/structmember.c delete (v == NULL) rules: only T_OBJECT
        // (unconditionally) and T_OBJECT_EX (when already set) may be deleted;
        // deleting a numeric/char member is a TypeError.
        if value.is_null() {
            if ty == PY_T_OBJECT_EX {
                if (*(field as *const *mut PyObject)).is_null() {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_AttributeError,
                        (*member).name,
                    );
                    return -1;
                }
            } else if ty != PY_T_OBJECT {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_TypeError,
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
                        &raw mut crate::abi_types::PyExc_TypeError,
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
                    &raw mut crate::abi_types::PyExc_TypeError,
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
                        &raw mut crate::abi_types::PyExc_SystemError,
                        c.as_ptr(),
                    );
                }
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
    // Molt-native (bridge-managed) objects hash through the runtime hash
    // authority over their handle bits (hash(int) == int, etc.), not tp_hash.
    let native = crate::bridge::GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
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
    let name = unsafe { foreign_type_name(op) };
    let msg = format!("unhashable type: '{}'", &name[..name.len().min(200)]);
    if let Ok(cmsg) = std::ffi::CString::new(msg) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
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

/// Best-effort type name of a live foreign `PyObject*` for diagnostics
/// (mirrors CPython's `Py_TYPE(v)->tp_name`, defaulting to `object`).
unsafe fn foreign_type_name(op: *mut PyObject) -> String {
    if op.is_null() {
        return "object".to_string();
    }
    let tp = unsafe { (*op).ob_type };
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
        let mut name = unsafe { foreign_type_name(res) };
        name.truncate(200);
        unsafe { crate::api::refcount::Py_DECREF(res) };
        let msg = format!("{dunder} returned non-string (type {name})");
        if let Ok(cmsg) = std::ffi::CString::new(msg) {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_TypeError,
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
                &raw mut crate::abi_types::PyExc_TypeError,
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
    let native = unsafe { crate::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(op) };
    match native {
        Some(bits) => unsafe { native_stringify(bits, true) },
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_native_str(op: *mut PyObject) -> *mut PyObject {
    let native = unsafe { crate::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(op) };
    match native {
        Some(bits) => unsafe { native_stringify(bits, false) },
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Repr(op: *mut PyObject) -> *mut PyObject {
    // CPython Objects/object.c PyObject_Repr: NULL -> "<NULL>".
    if op.is_null() {
        return unsafe { crate::api::strings::PyUnicode_FromString(c"<NULL>".as_ptr()) };
    }
    let tp = unsafe { (*op).ob_type };
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
    let name = unsafe { foreign_type_name(op) };
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
    // PyUnicode_CheckExact branch. A native bridge str carries
    // ob_type == &PyUnicode_Type.
    if unsafe { (*op).ob_type == &raw mut crate::abi_types::PyUnicode_Type } {
        unsafe { crate::api::refcount::Py_INCREF(op) };
        return op;
    }
    let tp = unsafe { (*op).ob_type };
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
    let tp = unsafe { (*a).ob_type };
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
    let name = unsafe { foreign_type_name(op) };
    let msg = format!("unhashable type: '{}'", &name[..name.len().min(200)]);
    if let Ok(cmsg) = std::ffi::CString::new(msg) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
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
    // Molt-native value. Decode-safe converter excludes a raw-registered foreign
    // object's `0xA11C` identity anchor (Class-2 mis-decode), so it is NEVER
    // hashed as a garbage float. Resolve then drop the bridge lock before hashing.
    let native = crate::bridge::GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
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
    let tv = unsafe { (*v).ob_type };
    let tw = unsafe { (*w).ob_type };
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
                    let mut n = unsafe { foreign_type_name(v) };
                    n.truncate(100);
                    n
                },
                {
                    let mut n = unsafe { foreign_type_name(w) };
                    n.truncate(100);
                    n
                },
            );
            if let Ok(c) = std::ffi::CString::new(msg) {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_TypeError,
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

    #[test]
    fn raw_registered_foreign_object_never_decodes_as_molt_value() {
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
            crate::bridge::GLOBAL_BRIDGE.register_raw_pyobj(&raw mut obj);
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

        assert!(!crate::bridge::GLOBAL_BRIDGE.release_pyobj(&raw mut obj));
    }
}
