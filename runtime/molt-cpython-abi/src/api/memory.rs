//! CPython memory allocator ABI.

use crate::abi_types::{
    Py_buffer, Py_ssize_t, PyBUF_FULL_RO, PyBUF_WRITE, PyMemoryViewObject, PyObject, PyTypeObject,
    PyVarObject,
};
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Malloc(size: usize) -> *mut c_void {
    // CPython obmalloc: `if (size == 0) size = 1;` so a 0-byte request returns a
    // unique non-NULL pointer a caller cannot mistake for allocation failure.
    unsafe { crate::platform::c_malloc(size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Calloc(nelem: usize, elsize: usize) -> *mut c_void {
    // CPython: a 0-element/0-size request still returns a unique pointer.
    let size = if nelem == 0 || elsize == 0 {
        1
    } else {
        let Some(size) = nelem.checked_mul(elsize) else {
            return std::ptr::null_mut();
        };
        size
    };
    unsafe { crate::platform::c_calloc(size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    // CPython: Realloc(p, 0) behaves like Realloc(p, 1) — it never frees `p`
    // and never returns NULL-on-success (realloc(p, 0) may do both in C).
    // Realloc may release the old address before it returns, including for
    // RawRealloc callers without the GIL. Revoke before another allocation can
    // reuse it. Initialized types are never valid realloc inputs; ordinary
    // buffers have no type identity, including on allocation failure.
    crate::api::typeobj::unregister_type_address(ptr.addr());
    unsafe { crate::platform::c_realloc(ptr, new_size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Free(ptr: *mut c_void) {
    crate::api::typeobj::unregister_type_address(ptr.addr());
    unsafe { crate::platform::c_free(ptr) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_RawMalloc(size: usize) -> *mut c_void {
    unsafe { PyMem_Malloc(size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_RawCalloc(nelem: usize, elsize: usize) -> *mut c_void {
    unsafe { PyMem_Calloc(nelem, elsize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_RawRealloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    unsafe { PyMem_Realloc(ptr, new_size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_RawFree(ptr: *mut c_void) {
    unsafe { PyMem_Free(ptr) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_Del(ptr: *mut c_void) {
    if !ptr.is_null() {
        if crate::bridge::GLOBAL_BRIDGE
            .managed_handle_for_pyobj(ptr.cast())
            .is_some()
        {
            unsafe {
                Py_FatalError(c"PyObject_GC_Del cannot free a managed runtime view".as_ptr())
            };
        }
        unsafe { native_gc_node_deallocate(ptr.addr()) };
    }
    unsafe { PyMem_Free(ptr) };
}

/// CPython `PyObject_Free` — release an object's memory. In CPython this is the
/// non-GC object deallocator (`object`'s default `tp_free`); Molt routes it
/// through the same cross-target allocation authority as
/// `PyMem_Free` (there is no separate obmalloc arena). GC allocations instead
/// retire their collector identity through `PyObject_GC_Del`. Provided
/// so `PyType_Ready` can install CPython's `tp_free` default: a static
/// C-extension type that leaves `tp_free` NULL (e.g. numpy's
/// `PyBoundArrayMethod_Type`) inherits `object.tp_free == PyObject_Free`, and
/// its `tp_dealloc`'s `Py_TYPE(self)->tp_free(self)` must resolve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Free(ptr: *mut c_void) {
    unsafe { PyMem_Free(ptr) };
}

// CPython's `PyObject_Malloc`/`Calloc`/`Realloc` are the object-domain
// allocator (`obmalloc`). Semantically they are `malloc`/`calloc`/`realloc`
// with the same "0-size returns a unique non-NULL pointer" guarantee — Molt has
// no separate obmalloc arena, so they route through the same allocation path as
// `PyMem_*`/`PyObject_Free`. A C extension (numpy) that pairs `PyObject_Malloc`
// with `PyObject_Free` must see a matched allocator, which this guarantees.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Malloc(size: usize) -> *mut c_void {
    unsafe { PyMem_Malloc(size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Calloc(nelem: usize, elsize: usize) -> *mut c_void {
    unsafe { PyMem_Calloc(nelem, elsize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    unsafe { PyMem_Realloc(ptr, new_size) }
}

pub(crate) unsafe fn molt_object_alloc(
    typeobj: *mut PyTypeObject,
    nitems: Py_ssize_t,
) -> *mut PyObject {
    unsafe { molt_object_alloc_with_tail(typeobj, nitems, false) }
}

/// The logical C layout is also the authority for negative dictionary offsets.
/// Generic allocation adds its sentinel; offset computation excludes it.
pub(crate) unsafe fn native_object_layout_size(
    typeobj: *mut PyTypeObject,
    nitems: Py_ssize_t,
    sentinel: bool,
) -> Option<usize> {
    unsafe {
        let basicsize = (*typeobj).tp_basicsize;
        let itemsize = (*typeobj).tp_itemsize;
        if basicsize < 0 || itemsize < 0 || nitems < 0 {
            return None;
        }
        let minimum = if itemsize > 0 || nitems > 0 {
            std::mem::size_of::<PyVarObject>()
        } else {
            std::mem::size_of::<PyObject>()
        };
        let base = (basicsize as usize).max(minimum);
        let count = (nitems as usize).checked_add(usize::from(sentinel))?;
        let extra = (itemsize as usize).checked_mul(count)?;
        let alignment = std::mem::size_of::<*mut c_void>();
        base.checked_add(extra)?
            .checked_add(alignment - 1)
            .map(|size| size & !(alignment - 1))
    }
}

/// GenericAlloc reserves one zero sentinel beyond the logical variable size.
/// Raw NewVar allocation has no sentinel contract. Both share checked sizing,
/// object initialization and native collector enrollment.
pub(crate) unsafe fn molt_object_alloc_with_tail(
    typeobj: *mut PyTypeObject,
    nitems: Py_ssize_t,
    sentinel: bool,
) -> *mut PyObject {
    if typeobj.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return std::ptr::null_mut();
    }
    if unsafe { (*typeobj).tp_flags } & crate::abi_types::Py_TPFLAGS_HAVE_GC != 0
        && let Some(reason) = unsafe { native_gc_type_admission_error(typeobj) }
    {
        unsafe { raise_native_gc_admission_error(reason) };
        return std::ptr::null_mut();
    }
    let basicsize = unsafe { (*typeobj).tp_basicsize };
    let itemsize = unsafe { (*typeobj).tp_itemsize };
    if nitems < 0 || basicsize < 0 || itemsize < 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return std::ptr::null_mut();
    }
    let Some(size) = (unsafe { native_object_layout_size(typeobj, nitems, sentinel) }) else {
        // Size overflow: CPython constructors set MemoryError before NULL.
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return std::ptr::null_mut();
    };
    let type_storage = typeobj == &raw mut crate::abi_types::PyType_Type
        || unsafe {
            crate::api::typeobj::PyType_IsSubtype(typeobj, &raw mut crate::abi_types::PyType_Type)
        } != 0;
    if type_storage && size < std::mem::size_of::<crate::abi_types::PyHeapTypeObject>() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return std::ptr::null_mut();
    }
    let raw = unsafe { PyMem_Calloc(1, size) }.cast::<PyObject>();
    if raw.is_null() {
        // OOM: `if (op == NULL) return PyErr_NoMemory();` (Objects/object.c) —
        // a NULL from _PyObject_New/_PyObject_NewVar/_PyObject_GC_New always
        // carries an active MemoryError.
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return std::ptr::null_mut();
    }
    if type_storage && !crate::api::typeobj::record_native_heap_type_allocation(raw.addr(), size) {
        unsafe { PyMem_Free(raw.cast()) };
        return unsafe { crate::api::errors::PyErr_NoMemory() };
    }
    let initialized = if nitems > 0 || itemsize > 0 {
        unsafe { PyObject_InitVar(raw.cast::<PyVarObject>(), typeobj, nitems) }.cast::<PyObject>()
    } else {
        unsafe { PyObject_Init(raw, typeobj) }
    };
    if !initialized.is_null()
        && unsafe { (*typeobj).tp_flags } & crate::abi_types::Py_TPFLAGS_HAVE_GC != 0
        && unsafe { (crate::hooks::hooks_or_stubs().native_gc_allocate)(initialized.addr()) } < 0
    {
        unsafe { crate::api::errors::check_native_status(-1, "native_gc_allocate") };
        crate::api::errors::with_preserved_error(|| unsafe {
            if (*typeobj).tp_flags & crate::abi_types::Py_TPFLAGS_HEAPTYPE != 0 {
                crate::api::refcount::Py_DECREF(typeobj.cast::<PyObject>());
            }
            PyMem_Free(initialized.cast::<c_void>());
        });
        return std::ptr::null_mut();
    }
    initialized
}

/// Failed native GC admission is a public C error, not optional trace data.
/// Allocation and exception construction share the same diagnostic boundary.
pub(crate) unsafe fn raise_native_gc_admission_error(reason: &'static str) {
    let message = std::ffi::CString::new(reason).expect("static GC admission message has no NUL");
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast(),
            message.as_ptr(),
        )
    };
}

/// Return the exact unsupported surface that prevents a native object type
/// from entering the runtime-owned mixed collector.  Admission is deliberately
/// fail-closed: until weakref ordering and legacy ``tp_del`` finalization are
/// modeled by the unified collector, publishing either shape would make cycle
/// reclamation observably incorrect.
pub(crate) unsafe fn native_gc_type_admission_error(
    typeobj: *mut PyTypeObject,
) -> Option<&'static str> {
    if typeobj.is_null() {
        return Some("native GC type is null");
    }
    if unsafe { (*typeobj).tp_flags } & crate::abi_types::Py_TPFLAGS_HAVE_GC == 0 {
        return Some("native GC type lacks HAVE_GC");
    }
    if unsafe { (*typeobj).tp_traverse }.is_none() {
        return Some("native GC type lacks tp_traverse");
    }
    // These slots describe runtime storage, not a native allocation protocol.
    // Readiness must not make an opaque builtin shell (or a subtype inheriting
    // its slots) newly allocatable as an invented C container layout.
    if unsafe { crate::api::typeobj::gc_uses_managed_storage(typeobj) } {
        return Some("native allocation cannot use managed builtin GC storage slots");
    }
    if unsafe { (*typeobj).tp_weaklistoffset } != 0 {
        return Some("native GC weakref ordering is not implemented");
    }
    if unsafe { (*typeobj).tp_del }.is_some() {
        return Some("native GC legacy tp_del finalization is not implemented");
    }
    None
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Init(
    op: *mut PyObject,
    typeobj: *mut PyTypeObject,
) -> *mut PyObject {
    // CPython: `if (op == NULL) return PyErr_NoMemory();` — the NULL input case
    // is an extension whose own allocation failed; it must observe MemoryError.
    if op.is_null() {
        return unsafe { crate::api::errors::PyErr_NoMemory() };
    }
    if typeobj.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return std::ptr::null_mut();
    }
    unsafe {
        (*op).ob_refcnt = 1;
        (*op).ob_type = typeobj;
        // _PyObject_Init (pycore_object.h): an instance of a HEAPTYPE owns a
        // reference to its type; without this incref the type's refcount
        // underflows into use-after-free when instances outlive creation scope.
        if (*typeobj).tp_flags & crate::abi_types::Py_TPFLAGS_HEAPTYPE != 0 {
            crate::api::refcount::Py_INCREF(typeobj.cast::<PyObject>());
        }
    }
    op
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_InitVar(
    op: *mut PyVarObject,
    typeobj: *mut PyTypeObject,
    size: Py_ssize_t,
) -> *mut PyVarObject {
    if op.is_null() {
        return unsafe { crate::api::errors::PyErr_NoMemory() }.cast::<PyVarObject>();
    }
    if typeobj.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return std::ptr::null_mut();
    }
    unsafe {
        // _PyObject_InitVar routes through _PyObject_Init (heap-type incref
        // included), then Py_SET_SIZE.
        PyObject_Init(op.cast::<PyObject>(), typeobj);
        (*op).ob_size = size;
    }
    op
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_New(typeobj: *mut PyTypeObject) -> *mut PyObject {
    unsafe { molt_object_alloc(typeobj, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_NewVar(
    typeobj: *mut PyTypeObject,
    nitems: Py_ssize_t,
) -> *mut PyVarObject {
    unsafe { molt_object_alloc(typeobj, nitems) }.cast::<PyVarObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_GC_New(typeobj: *mut PyTypeObject) -> *mut PyObject {
    unsafe { molt_object_alloc(typeobj, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyObject_GC_NewVar(
    typeobj: *mut PyTypeObject,
    nitems: Py_ssize_t,
) -> *mut PyVarObject {
    unsafe { molt_object_alloc(typeobj, nitems) }.cast::<PyVarObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_Track(op: *mut c_void) {
    if op.is_null() {
        return;
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let object = op.cast::<PyObject>();
    // Physical managed views must never become native nodes. The borrowed
    // lookup releases bridge locks before the runtime membership transaction.
    if let Some(bits) = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(object) {
        if unsafe {
            (crate::hooks::hooks_or_stubs().managed_gc_control)(
                bits,
                crate::hooks::ManagedGcAction::Track,
            )
        } != 0
        {
            unsafe {
                Py_FatalError(
                    c"PyObject_GC_Track requires an untracked GC-capable managed object".as_ptr(),
                )
            };
        }
        return;
    }
    let ty = unsafe { (*object).ob_type };
    if let Some(reason) = unsafe { native_gc_type_admission_error(ty) } {
        eprintln!("PyObject_GC_Track: {reason}");
        unsafe { Py_FatalError(c"PyObject_GC_Track rejected native GC admission".as_ptr()) };
    }
    if unsafe { (crate::hooks::hooks_or_stubs().native_gc_is_tracked)(op.addr()) } != 0 {
        unsafe { Py_FatalError(c"PyObject_GC_Track object is already tracked".as_ptr()) };
    }
    if unsafe { (crate::hooks::hooks_or_stubs().native_gc_track)(op.addr()) } != 0 {
        unsafe {
            Py_FatalError(c"PyObject_GC_Track requires a registered native allocation".as_ptr())
        };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_UnTrack(op: *mut c_void) {
    if op.is_null() {
        return;
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    if let Some(bits) = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(op.cast()) {
        if unsafe {
            (crate::hooks::hooks_or_stubs().managed_gc_control)(
                bits,
                crate::hooks::ManagedGcAction::Untrack,
            )
        } != 0
        {
            unsafe { Py_FatalError(c"PyObject_GC_UnTrack lost managed runtime identity".as_ptr()) };
        }
        return;
    }
    unsafe { (crate::hooks::hooks_or_stubs().native_gc_untrack)(op.addr()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_IsTracked(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    if let Some(bits) = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(op) {
        return unsafe {
            (crate::hooks::hooks_or_stubs().managed_gc_control)(
                bits,
                crate::hooks::ManagedGcAction::IsTracked,
            )
        };
    }
    unsafe { (crate::hooks::hooks_or_stubs().native_gc_is_tracked)(op.addr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_IsFinalized(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    if let Some(bits) = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(op) {
        return unsafe {
            (crate::hooks::hooks_or_stubs().managed_gc_control)(
                bits,
                crate::hooks::ManagedGcAction::IsFinalized,
            )
        };
    }
    unsafe { (crate::hooks::hooks_or_stubs().native_gc_is_finalized)(op.addr()) }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeGcEdgeKind {
    ManagedHandle = 0,
    NativePointer = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeGcEdge {
    pub kind: u8,
    pub reserved: [u8; 7],
    pub value: u64,
}

impl NativeGcEdge {
    /// Classify a borrowed physical C edge without acquiring ownership or
    /// manufacturing a foreign wrapper. The caller pins the source object.
    pub fn from_pyobj(bridge: &crate::bridge::ObjectBridge, child: *mut PyObject) -> Option<Self> {
        if child.is_null() {
            return None;
        }
        Some(
            if let Some(handle) = bridge.managed_handle_for_pyobj(child) {
                Self {
                    kind: NativeGcEdgeKind::ManagedHandle as u8,
                    reserved: [0; 7],
                    value: handle,
                }
            } else {
                Self {
                    kind: NativeGcEdgeKind::NativePointer as u8,
                    reserved: [0; 7],
                    value: child.addr() as u64,
                }
            },
        )
    }
}

pub type NativeGcVisitProc =
    unsafe extern "C" fn(edge: NativeGcEdge, context: *mut c_void) -> c_int;

type PyVisitProc = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int;

struct ManagedGcVisitContext {
    visit: PyVisitProc,
    argument: *mut c_void,
}

unsafe extern "C" fn managed_gc_visit_edge(edge: NativeGcEdge, context: *mut c_void) -> c_int {
    let context = unsafe { &*context.cast::<ManagedGcVisitContext>() };
    let object = match edge.kind {
        value if value == NativeGcEdgeKind::ManagedHandle as u8 => unsafe {
            crate::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(edge.value)
        },
        value if value == NativeGcEdgeKind::NativePointer as u8 => {
            let Ok(address) = usize::try_from(edge.value) else {
                return -1;
            };
            core::ptr::with_exposed_provenance_mut::<PyObject>(address)
        }
        _ => return -1,
    };
    if object.is_null() {
        unsafe {
            crate::bridge::ensure_result_error(c"GC edge could not acquire its canonical C view")
        };
        return -1;
    }
    unsafe { (context.visit)(object, context.argument) }
}

/// Public builtin slots project the one runtime graph. They never enroll the
/// facade as a second native GC node and never commit mutable C projections.
pub unsafe extern "C" fn molt_managed_gc_traverse(
    object: *mut PyObject,
    visit: *mut c_void,
    argument: *mut c_void,
) -> c_int {
    let _gil = crate::hooks::RuntimeGilGuard::ensure();
    let Some(bits) = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(object) else {
        unsafe {
            raise_native_gc_admission_error("builtin GC traversal requires runtime-owned storage")
        };
        return -1;
    };
    if visit.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let mut context = ManagedGcVisitContext {
        visit: unsafe { std::mem::transmute::<*mut c_void, PyVisitProc>(visit) },
        argument,
    };
    unsafe {
        (crate::hooks::hooks_or_stubs().managed_gc_traverse)(
            bits,
            managed_gc_visit_edge,
            std::ptr::from_mut(&mut context).cast(),
        )
    }
}

pub unsafe extern "C" fn molt_managed_gc_clear(object: *mut PyObject) -> c_int {
    let _gil = crate::hooks::RuntimeGilGuard::ensure();
    let Some(bits) = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(object) else {
        unsafe {
            raise_native_gc_admission_error("builtin GC clear requires runtime-owned storage")
        };
        return -1;
    };
    let result = unsafe { (crate::hooks::hooks_or_stubs().managed_gc_clear)(bits) };
    if result < 0 {
        unsafe { crate::bridge::ensure_result_error(c"runtime builtin GC clear failed") };
    }
    result
}

/// A real native list prefix may be traversed by an extension's own layout
/// lifecycle. This does not admit allocation through managed builtin slots.
pub unsafe extern "C" fn molt_list_gc_traverse(
    object: *mut PyObject,
    visit: *mut c_void,
    argument: *mut c_void,
) -> c_int {
    let _gil = crate::hooks::RuntimeGilGuard::ensure();
    if crate::bridge::GLOBAL_BRIDGE
        .managed_handle_for_pyobj(object)
        .is_some()
    {
        return unsafe { molt_managed_gc_traverse(object, visit, argument) };
    }
    if object.is_null() || visit.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let list = object.cast::<crate::abi_types::PyListObject>();
    let length = unsafe { (*list).ob_base.ob_size };
    let items = unsafe { (*list).ob_item };
    if length < 0 || (length != 0 && items.is_null()) {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let visit: PyVisitProc = unsafe { std::mem::transmute(visit) };
    for index in (0..length).rev() {
        let child = unsafe { *items.offset(index) };
        if !child.is_null() {
            let status = unsafe { visit(child, argument) };
            if status != 0 {
                return status;
            }
        }
    }
    0
}

struct NativeGcVisitContext {
    visit: NativeGcVisitProc,
    context: *mut c_void,
}

unsafe extern "C" fn native_gc_node_visit_edge(
    child: *mut PyObject,
    raw_context: *mut c_void,
) -> c_int {
    if raw_context.is_null() {
        return 0;
    }
    let context = unsafe { &mut *raw_context.cast::<NativeGcVisitContext>() };
    let Some(edge) = NativeGcEdge::from_pyobj(&crate::bridge::GLOBAL_BRIDGE, child) else {
        return 0;
    };
    unsafe { (context.visit)(edge, context.context) }
}

/// Allocation-free edge projection for the runtime-owned mixed GC graph. The
/// runtime calls this only while its epoch/STW authority pins ``addr`` live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn native_gc_node_visit(
    addr: usize,
    visit: NativeGcVisitProc,
    context: *mut c_void,
) -> c_int {
    if addr == 0 {
        return -1;
    }
    let object = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
    let ty = unsafe { (*object).ob_type };
    let Some(traverse) = (!ty.is_null())
        .then(|| unsafe { (*ty).tp_traverse })
        .flatten()
    else {
        return -1;
    };
    let mut visit_context = NativeGcVisitContext { visit, context };
    unsafe {
        traverse(
            object,
            native_gc_node_visit_edge as *const () as *mut c_void,
            (&mut visit_context as *mut NativeGcVisitContext).cast::<c_void>(),
        )
    }
}

pub unsafe fn native_gc_node_refcount(addr: usize) -> isize {
    if addr == 0 {
        return 0;
    }
    unsafe { (*core::ptr::with_exposed_provenance_mut::<PyObject>(addr)).ob_refcnt }
}

pub unsafe fn native_gc_node_incref(addr: usize) {
    if addr != 0 {
        unsafe {
            crate::api::refcount::Py_INCREF(core::ptr::with_exposed_provenance_mut::<PyObject>(
                addr,
            ))
        };
    }
}

pub unsafe fn native_gc_node_decref(addr: usize) {
    if addr != 0 {
        unsafe {
            crate::api::refcount::Py_DECREF(core::ptr::with_exposed_provenance_mut::<PyObject>(
                addr,
            ))
        };
    }
}

pub(crate) unsafe fn native_gc_node_deallocate(addr: usize) {
    if addr != 0 {
        unsafe { (crate::hooks::hooks_or_stubs().native_gc_deallocate)(addr) };
    }
}

unsafe fn run_native_finalizer_preserving_error(
    object: *mut PyObject,
    finalize: unsafe extern "C" fn(*mut PyObject),
) {
    crate::api::errors::with_preserved_error(|| unsafe {
        finalize(object);
        // Both outer channels remain detached through callback execution,
        // reporting and owner retirement; report only this callback's error.
        crate::api::errors::PyErr_WriteUnraisable(object);
    });
}

/// Invoke ``tp_finalize`` when present.  Return 1 only when a finalizer ran,
/// 0 for an admitted node without a finalizer, and -1 for an invalid node.
/// This keeps the runtime's finalized bit faithful to CPython: merely visiting
/// a node without ``tp_finalize`` must not make PyObject_GC_IsFinalized true.
pub unsafe fn native_gc_node_finalize(addr: usize) -> c_int {
    if addr == 0 {
        return -1;
    }
    let object = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
    let ty = unsafe { (*object).ob_type };
    if ty.is_null() {
        return -1;
    }
    if let Some(finalize) = unsafe { (*ty).tp_finalize } {
        let claim = unsafe { (crate::hooks::hooks_or_stubs().native_gc_claim_finalizer)(addr) };
        if claim <= 0 {
            return claim;
        }
        unsafe { run_native_finalizer_preserving_error(object, finalize) };
        return 1;
    }
    0
}

/// Break a native node's outgoing edges.
///
/// C-stable tri-state result: 0 means the type's ``tp_clear`` completed, 1
/// means the admitted node has no ``tp_clear`` (valid for immutable nodes),
/// and -1 means invalid input or a bare negative ``tp_clear`` status. Newly
/// raised callback errors are reported as unraisable here while both prior
/// error channels are restored exactly; this boundary never conflates a valid
/// no-op with failure or consumes unrelated error state.
pub unsafe fn native_gc_node_clear(addr: usize) -> c_int {
    if addr == 0 {
        return -1;
    }
    let object = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
    let ty = unsafe { (*object).ob_type };
    let Some(clear) = (!ty.is_null()).then(|| unsafe { (*ty).tp_clear }).flatten() else {
        return 1;
    };
    let (status, raised_error) = crate::api::errors::with_preserved_error(|| unsafe {
        let status = clear(object);
        let raised_error = crate::api::errors::raised_error_pending();
        crate::api::errors::PyErr_WriteUnraisable(object);
        (status, raised_error)
    });
    // A callback error has already been reported and suppressed, matching the
    // collector's continue-after-unraisable behavior. A bare negative status
    // remains an honest resource failure.
    if status < 0 && !raised_error { -1 } else { 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CallFinalizerFromDealloc(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let typeobj = unsafe { (*op).ob_type };
    if typeobj.is_null() {
        return 0;
    }
    if let Some(finalize) = unsafe { (*typeobj).tp_finalize } {
        // Only GC-capable objects have a collector identity/finalized bit.
        // CPython clears the finalized state of non-GC objects on resurrection,
        // so their next terminal attempt must invoke the finalizer again.
        if unsafe { (*typeobj).tp_flags } & crate::abi_types::Py_TPFLAGS_HAVE_GC != 0 {
            let claim =
                unsafe { (crate::hooks::hooks_or_stubs().native_gc_claim_finalizer)(op.addr()) };
            if claim < 0 {
                return -1;
            }
            if claim == 0 {
                return 0;
            }
        }
        // CPython Objects/object.c: temporarily resurrect to refcount 1 so the
        // finalizer runs against a live object, then detect resurrection — a
        // finalizer that stored a new reference leaves refcnt > 1, and the
        // deallocator must ABORT the free (return -1) instead of freeing a
        // live object (use-after-free).
        unsafe { (*op).ob_refcnt = 1 };
        unsafe { run_native_finalizer_preserving_error(op, finalize) };
        let refcnt = unsafe { (*op).ob_refcnt };
        if refcnt > 1 {
            // Object resurrected: undo the temporary reference.
            unsafe { (*op).ob_refcnt -= 1 };
            return -1;
        }
        unsafe { (*op).ob_refcnt = 0 };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGC_Disable() -> c_int {
    unsafe { (crate::hooks::hooks_or_stubs().gc_disable)() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGC_Enable() -> c_int {
    unsafe { (crate::hooks::hooks_or_stubs().gc_enable)() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGC_IsEnabled() -> c_int {
    unsafe { (crate::hooks::hooks_or_stubs().gc_is_enabled)() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGC_Collect() -> Py_ssize_t {
    unsafe { (crate::hooks::hooks_or_stubs().gc_collect)() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_FatalError(message: *const c_char) -> ! {
    if !message.is_null() {
        let rendered = unsafe { std::ffi::CStr::from_ptr(message) }.to_string_lossy();
        eprintln!("molt-cpython-abi fatal error: {rendered}");
    } else {
        eprintln!("molt-cpython-abi fatal error");
    }
    std::process::abort()
}

// CPython 3.12 pycore_ceval.h C_RECURSION_LIMIT — the C-stack guard bound.
const C_RECURSION_LIMIT: usize = 800;

thread_local! {
    static C_RECURSION_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_EnterRecursiveCall(where_: *const c_char) -> c_int {
    // CPython _Py_CheckRecursiveCall: when the C recursion budget is exhausted,
    // raise RecursionError "maximum recursion depth exceeded%s" and return -1 —
    // converting unbounded C recursion into a catchable error instead of a
    // wasm stack trap. The previous body returned 0 unconditionally (no guard).
    let depth = C_RECURSION_DEPTH.with(|d| {
        let v = d.get() + 1;
        d.set(v);
        v
    });
    if depth > C_RECURSION_LIMIT {
        C_RECURSION_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        let suffix = if where_.is_null() {
            String::new()
        } else {
            unsafe { std::ffi::CStr::from_ptr(where_) }
                .to_string_lossy()
                .into_owned()
        };
        let msg = format!("maximum recursion depth exceeded{suffix}");
        if let Ok(c) = std::ffi::CString::new(msg) {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_RecursionError)
                        .cast::<crate::abi_types::PyObject>(),
                    c.as_ptr(),
                );
            }
        }
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_LeaveRecursiveCall() {
    C_RECURSION_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTraceMalloc_Track(_domain: u32, _ptr: usize, _size: usize) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTraceMalloc_Untrack(_domain: u32, _ptr: usize) -> c_int {
    0
}

/// Stable native export transaction. This is a resource of the runtime
/// MemoryView, not a second Python object or an address registry. The master
/// is allocated before getbuffer because exporters may point into it.
struct MemoryViewExport {
    references: std::sync::atomic::AtomicUsize,
    master: Py_buffer,
}

pub struct MemoryViewLease(std::ptr::NonNull<MemoryViewExport>);

impl MemoryViewLease {
    /// The declaring slot is passed explicitly by __buffer__ adapters; ordinary
    /// acquisition passes PyObject_GetBuffer. Failed callbacks own their cleanup.
    pub unsafe fn acquire(
        object: *mut PyObject,
        flags: c_int,
        get: unsafe extern "C" fn(*mut PyObject, *mut Py_buffer, c_int) -> c_int,
    ) -> Result<Self, ()> {
        let owner = unsafe { crate::api::refcount::OwnedPyObject::from_borrowed(object) };
        let pointer = unsafe { PyMem_Calloc(1, std::mem::size_of::<MemoryViewExport>()) }
            .cast::<MemoryViewExport>();
        let Some(pointer) = std::ptr::NonNull::new(pointer) else {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return Err(());
        };
        unsafe {
            (&raw mut (*pointer.as_ptr()).references).write(std::sync::atomic::AtomicUsize::new(1))
        };
        let lease = Self(pointer);
        let status = unsafe { get(owner.as_ptr(), &raw mut (*pointer.as_ptr()).master, flags) };
        if unsafe { crate::api::errors::check_native_status(status, "native buffer callback") } < 0
        {
            if status < 0 {
                unsafe { (&raw mut (*pointer.as_ptr()).master).write(std::mem::zeroed()) };
            }
            return Err(());
        }
        Ok(lease)
    }

    pub fn owner(&self) -> *mut PyObject {
        unsafe { (*self.0.as_ptr()).master.obj }
    }
    pub fn descriptor(&self) -> *const Py_buffer {
        unsafe { &raw const (*self.0.as_ptr()).master }
    }
    pub fn gc_edge(&self) -> Option<NativeGcEdge> {
        NativeGcEdge::from_pyobj(&crate::bridge::GLOBAL_BRIDGE, self.owner())
    }
}

impl Clone for MemoryViewLease {
    fn clone(&self) -> Self {
        // Like Arc, an unreachable reference-count overflow must never wrap and
        // free live storage. No fallible allocation occurs during sharing.
        let old = unsafe {
            (*self.0.as_ptr())
                .references
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        };
        if old >= isize::MAX as usize {
            std::process::abort();
        }
        unsafe { crate::api::refcount::Py_XINCREF(self.owner()) };
        Self(self.0)
    }
}

impl Drop for MemoryViewLease {
    fn drop(&mut self) {
        let export = self.0.as_ptr();
        if unsafe {
            (*export)
                .references
                .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
        } != 1
        {
            unsafe { crate::api::errors::release_preserving_error(&[self.owner()]) };
            return;
        }
        crate::api::errors::with_preserved_error(|| unsafe {
            crate::api::buffer::PyBuffer_Release(&raw mut (*export).master);
            PyMem_Free(export.cast());
        });
    }
}

unsafe fn runtime_memoryview_from_descriptor(
    info: *const Py_buffer,
    lease: *const MemoryViewLease,
) -> *mut PyObject {
    let descriptor = match unsafe { crate::api::buffer::descriptor_from_pybuffer(info) } {
        Ok(view) => view,
        Err(()) => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_BufferError).cast(),
                    c"invalid or indirect buffer descriptor for memoryview".as_ptr(),
                )
            };
            return std::ptr::null_mut();
        }
    };
    let format = if unsafe { (*info).format.is_null() } {
        c"B".as_ptr()
    } else {
        unsafe { (*info).format }
    };
    let result = unsafe {
        (crate::hooks::hooks_or_stubs().memoryview_from_buffer)(&descriptor, format, lease.cast())
    };
    unsafe { crate::bridge::GLOBAL_BRIDGE.owned_result_to_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromMemory(
    mem: *mut c_char,
    size: Py_ssize_t,
    flags: c_int,
) -> *mut PyObject {
    if size < 0 || (mem.is_null() && size != 0) {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return std::ptr::null_mut();
    }
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    if unsafe {
        crate::api::buffer::PyBuffer_FillInfo(
            &raw mut view,
            std::ptr::null_mut(),
            mem.cast(),
            size,
            (flags & PyBUF_WRITE == 0) as c_int,
            PyBUF_FULL_RO,
        )
    } < 0
    {
        return std::ptr::null_mut();
    }
    unsafe { runtime_memoryview_from_descriptor(&raw const view, std::ptr::null()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromBuffer(info: *mut Py_buffer) -> *mut PyObject {
    // CPython copies descriptor values but does not own info.obj or its release.
    // The caller retains responsibility for the raw storage's lifetime.
    unsafe { runtime_memoryview_from_descriptor(info, std::ptr::null()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_Check(op: *mut PyObject) -> c_int {
    crate::bridge::GLOBAL_BRIDGE.molt_handle_for_pyobj(op)
        .is_some_and(|bits| unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(bits.bits()) }
            == crate::abi_types::MoltTypeTag::MemoryView as u8) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_GET_BASE(op: *mut PyObject) -> *mut PyObject {
    if unsafe { PyMemoryView_Check(op) } == 0 {
        return std::ptr::null_mut();
    }
    unsafe { (*op.cast::<PyMemoryViewObject>()).base }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_GET_BUFFER(op: *mut PyObject) -> *mut Py_buffer {
    if unsafe { PyMemoryView_Check(op) } == 0 {
        return std::ptr::null_mut();
    }
    unsafe { &raw mut (*op.cast::<PyMemoryViewObject>()).view }
}

pub(crate) unsafe fn memoryview_from_buffer_proc(
    op: *mut PyObject,
    flags: c_int,
    get: unsafe extern "C" fn(*mut PyObject, *mut Py_buffer, c_int) -> c_int,
) -> *mut PyObject {
    if op.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return std::ptr::null_mut();
    }
    let Ok(lease) = (unsafe { MemoryViewLease::acquire(op, flags, get) }) else {
        return std::ptr::null_mut();
    };
    unsafe { runtime_memoryview_from_descriptor(lease.descriptor(), &lease) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromObject(op: *mut PyObject) -> *mut PyObject {
    let Some(value) = (unsafe { crate::bridge::RuntimeValue::acquire(op) }) else {
        return std::ptr::null_mut();
    };
    let result = unsafe { (crate::hooks::hooks_or_stubs().memoryview_new)(value.bits()) };
    unsafe { crate::bridge::GLOBAL_BRIDGE.owned_result_to_pyobj(result) }
}

pub(crate) unsafe fn memoryview_release(op: *mut PyObject) -> *mut PyObject {
    let Some(value) = (unsafe { crate::bridge::RuntimeValue::acquire(op) }) else {
        return std::ptr::null_mut();
    };
    let result = unsafe { (crate::hooks::hooks_or_stubs().memoryview_release)(value.bits()) };
    unsafe { crate::bridge::GLOBAL_BRIDGE.owned_result_to_pyobj(result) }
}

pub unsafe extern "C" fn memoryview_releasebuffer(_op: *mut PyObject, view: *mut Py_buffer) {
    unsafe { crate::api::buffer::release_managed_export(view) };
}

pub unsafe extern "C" fn memoryview_item(op: *mut PyObject, index: Py_ssize_t) -> *mut PyObject {
    let key = unsafe {
        crate::api::refcount::OwnedPyObject::from_owned(crate::api::numbers::PyLong_FromSsize_t(
            index,
        ))
    };
    if key.as_ptr().is_null() {
        return std::ptr::null_mut();
    }
    unsafe { crate::api::object::PyObject_GetItem(op, key.as_ptr()) }
}

pub unsafe extern "C" fn memoryview_ass_item(
    op: *mut PyObject,
    index: Py_ssize_t,
    value: *mut PyObject,
) -> c_int {
    let key = unsafe {
        crate::api::refcount::OwnedPyObject::from_owned(crate::api::numbers::PyLong_FromSsize_t(
            index,
        ))
    };
    if key.as_ptr().is_null() {
        return -1;
    }
    unsafe { crate::api::object::molt_ass_subscript(op, key.as_ptr(), value) }
}

#[cfg(test)]
mod object_allocator_tests {
    use super::*;

    unsafe extern "C" fn noop_traverse(
        _op: *mut PyObject,
        _visit: *mut c_void,
        _arg: *mut c_void,
    ) -> c_int {
        0
    }

    unsafe extern "C" fn noop_del(_op: *mut PyObject) {}

    unsafe fn install_static_error(exc_type: *mut PyObject) {
        unsafe { crate::api::refcount::Py_INCREF(exc_type) };
        crate::api::errors::restore_current_error_exact(crate::api::errors::OwnedCError {
            exc_type,
            value: std::ptr::null_mut(),
            traceback: std::ptr::null_mut(),
        });
    }

    unsafe extern "C" fn callback_raises_type_error(_op: *mut PyObject) -> c_int {
        unsafe {
            install_static_error((&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>())
        };
        -1
    }

    unsafe extern "C" fn finalizer_raises_type_error(_op: *mut PyObject) {
        unsafe {
            install_static_error((&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>())
        };
    }

    unsafe extern "C" fn callback_bare_failure(_op: *mut PyObject) -> c_int {
        -1
    }

    #[test]
    fn native_gc_admission_rejects_unmodeled_lifecycle_surfaces() {
        let mut type_: PyTypeObject = unsafe { std::mem::zeroed() };
        type_.tp_flags = crate::abi_types::Py_TPFLAGS_HAVE_GC;
        type_.tp_traverse = Some(noop_traverse);
        assert_eq!(
            unsafe { native_gc_type_admission_error(&raw mut type_) },
            None
        );

        type_.tp_weaklistoffset = 8;
        assert_eq!(
            unsafe { native_gc_type_admission_error(&raw mut type_) },
            Some("native GC weakref ordering is not implemented")
        );
        type_.tp_weaklistoffset = 0;
        type_.tp_del = Some(noop_del);
        assert_eq!(
            unsafe { native_gc_type_admission_error(&raw mut type_) },
            Some("native GC legacy tp_del finalization is not implemented")
        );
    }

    #[test]
    fn native_lifecycle_callbacks_preserve_prior_error_and_consume_only_new_error() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        crate::bridge::molt_cpython_abi_init();
        drop(crate::api::errors::take_current_error());
        let prior = (&raw mut crate::abi_types::PyExc_ValueError).cast::<PyObject>();
        let mut type_: PyTypeObject = unsafe { std::mem::zeroed() };
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut type_,
        };

        unsafe { install_static_error(prior) };
        unsafe { (*object.ob_type).tp_clear = Some(callback_raises_type_error) };
        assert_eq!(unsafe { native_gc_node_clear((&raw mut object).addr()) }, 0);
        let restored = crate::api::errors::take_current_error().expect("prior error restored");
        assert_eq!(restored.exc_type, prior);
        drop(restored);

        unsafe { install_static_error(prior) };
        unsafe { (*object.ob_type).tp_clear = Some(callback_bare_failure) };
        assert_eq!(
            unsafe { native_gc_node_clear((&raw mut object).addr()) },
            -1
        );
        let restored = crate::api::errors::take_current_error().expect("prior error preserved");
        assert_eq!(restored.exc_type, prior);
        drop(restored);

        unsafe { install_static_error(prior) };
        unsafe {
            run_native_finalizer_preserving_error(&raw mut object, finalizer_raises_type_error)
        };
        let restored = crate::api::errors::take_current_error().expect("prior error restored");
        assert_eq!(restored.exc_type, prior);
        drop(restored);
    }

    /// `PyObject_Malloc`/`Realloc`/`Free` round-trip real writable storage, and a
    /// 0-size `PyObject_Calloc` returns a unique non-NULL block (CPython's obmalloc
    /// contract), so a numpy pairing of `PyObject_Malloc` with `PyObject_Free`
    /// cannot fault or leak on the 0 edge.
    #[test]
    fn object_allocators_roundtrip() {
        unsafe {
            let p = PyObject_Malloc(64);
            assert!(!p.is_null(), "PyObject_Malloc(64) is NULL");
            std::ptr::write_bytes(p.cast::<u8>(), 0xAB, 64);
            let p = PyObject_Realloc(p, 256);
            assert!(!p.is_null(), "PyObject_Realloc(256) is NULL");
            assert_eq!(*p.cast::<u8>(), 0xAB, "realloc must preserve leading bytes");
            PyObject_Free(p);

            let zero = PyObject_Calloc(0, 0);
            assert!(!zero.is_null(), "PyObject_Calloc(0,0) must be non-NULL");
            PyObject_Free(zero);

            let c = PyObject_Calloc(8, 8);
            assert!(!c.is_null());
            assert_eq!(*c.cast::<u64>(), 0, "PyObject_Calloc must zero the block");
            PyObject_Free(c);
        }
    }
}
