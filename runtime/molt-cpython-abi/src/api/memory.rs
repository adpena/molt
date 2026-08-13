//! CPython memory allocator ABI.

use crate::abi_types::{
    Py_buffer, Py_ssize_t, PyBUF_FULL_RO, PyBUF_WRITE, PyMemoryView_Type, PyMemoryViewObject,
    PyObject, PyTypeObject, PyVarObject,
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
        unsafe { native_gc_node_deallocate(ptr.addr()) };
    }
    unsafe { PyMem_Free(ptr) };
}

/// CPython `PyObject_Free` — release an object's memory. In CPython this is the
/// non-GC object deallocator (`object`'s default `tp_free`); Molt routes it
/// through the same cross-target allocation authority as
/// `PyMem_Free`/`PyObject_GC_Del` (there is no separate obmalloc arena or GC
/// tracking in the runtime). Provided
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
    if typeobj.is_null() {
        return std::ptr::null_mut();
    }
    if unsafe { (*typeobj).tp_flags } & crate::abi_types::Py_TPFLAGS_HAVE_GC != 0
        && let Some(reason) = unsafe { native_gc_type_admission_error(typeobj) }
    {
        crate::capi_trace::record_silent_failure("molt_object_alloc", Some(reason));
        return std::ptr::null_mut();
    }
    let basicsize = unsafe { (*typeobj).tp_basicsize };
    let itemsize = unsafe { (*typeobj).tp_itemsize };
    let min_size = if itemsize > 0 || nitems > 0 {
        std::mem::size_of::<PyVarObject>()
    } else {
        std::mem::size_of::<PyObject>()
    };
    let base = (basicsize.max(min_size as Py_ssize_t)) as usize;
    let extra = if itemsize > 0 && nitems > 0 {
        match (itemsize as usize).checked_mul(nitems as usize) {
            Some(extra) => extra,
            None => return std::ptr::null_mut(),
        }
    } else {
        0
    };
    let Some(size) = base.checked_add(extra) else {
        // Size overflow: CPython constructors set MemoryError before NULL.
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return std::ptr::null_mut();
    };
    let raw = unsafe { PyMem_Calloc(1, size) }.cast::<PyObject>();
    if raw.is_null() {
        // OOM: `if (op == NULL) return PyErr_NoMemory();` (Objects/object.c) —
        // a NULL from _PyObject_New/_PyObject_NewVar/_PyObject_GC_New always
        // carries an active MemoryError.
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return std::ptr::null_mut();
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
        crate::capi_trace::record_silent_failure(
            "native_gc_allocate",
            Some("runtime mixed-GC identity publication failed"),
        );
        if unsafe { (*typeobj).tp_flags } & crate::abi_types::Py_TPFLAGS_HEAPTYPE != 0 {
            unsafe { crate::api::refcount::Py_DECREF(typeobj.cast::<PyObject>()) };
        }
        unsafe { PyMem_Free(initialized.cast::<c_void>()) };
        return std::ptr::null_mut();
    }
    initialized
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
    if typeobj.is_null() {
        return std::ptr::null_mut();
    }
    unsafe { molt_object_alloc(typeobj, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_Track(op: *mut c_void) {
    if op.is_null() {
        return;
    }
    let object = op.cast::<PyObject>();
    // Managed views belong exclusively to the runtime GC. Publishing them as
    // native nodes would duplicate one object in both domains and corrupt root
    // subtraction. This lookup is non-owning and completes before the runtime
    // mixed-GC registry callback acquires its own lock.
    if crate::bridge::GLOBAL_BRIDGE
        .managed_handle_for_pyobj(object)
        .is_some()
    {
        return;
    }
    let ty = unsafe { (*object).ob_type };
    if let Some(reason) = unsafe { native_gc_type_admission_error(ty) } {
        crate::capi_trace::record_silent_failure("PyObject_GC_Track", Some(reason));
        return;
    }
    if unsafe { (crate::hooks::hooks_or_stubs().native_gc_track)(op.addr()) } < 0 {
        crate::capi_trace::record_silent_failure(
            "PyObject_GC_Track",
            Some("runtime mixed-GC tracking failed"),
        );
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_UnTrack(op: *mut c_void) {
    if op.is_null() {
        return;
    }
    let object = op.cast::<PyObject>();
    if crate::bridge::GLOBAL_BRIDGE
        .managed_handle_for_pyobj(object)
        .is_some()
    {
        return;
    }
    unsafe { (crate::hooks::hooks_or_stubs().native_gc_untrack)(op.addr()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_IsTracked(op: *mut PyObject) -> c_int {
    if op.is_null()
        || crate::bridge::GLOBAL_BRIDGE
            .managed_handle_for_pyobj(op)
            .is_some()
    {
        return 0;
    }
    unsafe { (crate::hooks::hooks_or_stubs().native_gc_is_tracked)(op.addr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_IsFinalized(op: *mut PyObject) -> c_int {
    if op.is_null()
        || crate::bridge::GLOBAL_BRIDGE
            .managed_handle_for_pyobj(op)
            .is_some()
    {
        return 0;
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

pub type NativeGcVisitProc =
    unsafe extern "C" fn(edge: NativeGcEdge, context: *mut c_void) -> c_int;

struct NativeGcVisitContext {
    visit: NativeGcVisitProc,
    context: *mut c_void,
}

unsafe extern "C" fn native_gc_node_visit_edge(
    child: *mut PyObject,
    raw_context: *mut c_void,
) -> c_int {
    if child.is_null() || raw_context.is_null() {
        return 0;
    }
    let context = unsafe { &mut *raw_context.cast::<NativeGcVisitContext>() };
    let edge = if let Some(handle) = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(child) {
        NativeGcEdge {
            kind: NativeGcEdgeKind::ManagedHandle as u8,
            reserved: [0; 7],
            value: handle,
        }
    } else {
        NativeGcEdge {
            kind: NativeGcEdgeKind::NativePointer as u8,
            reserved: [0; 7],
            value: child.addr() as u64,
        }
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
    let prior = crate::api::errors::take_current_error();
    unsafe { finalize(object) };
    if let Some(raised) = crate::api::errors::take_current_error() {
        // Report only the callback's newly raised error. The prior indicator
        // remains detached and owned across unraisable-hook reentrancy.
        crate::api::errors::restore_current_error_exact(raised);
        unsafe { crate::api::errors::PyErr_WriteUnraisable(object) };
    }
    if let Some(prior) = prior {
        crate::api::errors::restore_current_error_exact(prior);
    }
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
/// raised callback errors are reported as unraisable here while any prior C
/// error indicator is restored exactly; this boundary never conflates a valid
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
    let prior = crate::api::errors::take_current_error();
    let status = unsafe { clear(object) };
    let raised = crate::api::errors::take_current_error();
    let raised_error = raised.is_some();
    if let Some(raised) = raised {
        crate::api::errors::restore_current_error_exact(raised);
        unsafe { crate::api::errors::PyErr_WriteUnraisable(object) };
    }
    if let Some(prior) = prior {
        crate::api::errors::restore_current_error_exact(prior);
    }
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
        let claim =
            unsafe { (crate::hooks::hooks_or_stubs().native_gc_claim_finalizer)(op.addr()) };
        if claim < 0 {
            return -1;
        }
        if claim == 0 {
            return 0;
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

/// Allocate a memoryview object with an EMPTY (zeroed) view and zeroed
/// embedded descriptor storage, published as a raw pointer.
///
/// Constructors MUST fill `(*mv).view` **in place** through the returned raw
/// pointer — never fill a stack `Py_buffer` and move it into the object. A
/// `PyBuffer_FillInfo`'d view is self-referential (`shape = &view.len`,
/// `strides = &view.itemsize` — CPython's field model), and a foreign
/// `bf_getbuffer` is allowed the same trick, so the view's final home must
/// exist before it is filled. This is CPython's own construction order:
/// `PyMemoryView_FromMemory` fills `&mbuf->master` directly inside the heap
/// object (Objects/memoryobject.c).
///
/// `Box::into_raw` happens BEFORE any interior pointer is derived, so every
/// pointer later published into the view (self-pointers, embedded-storage
/// pointers) is a raw projection off the post-`into_raw` pointer and stays
/// valid until `Box::from_raw` at dealloc (Miri finding-C discipline).
fn alloc_memoryview_object() -> *mut PyMemoryViewObject {
    Box::into_raw(Box::new(PyMemoryViewObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyMemoryView_Type,
        },
        // SAFETY: `Py_buffer` is a plain `#[repr(C)]` pointers+integers struct;
        // all-zero is its canonical empty state.
        view: unsafe { std::mem::zeroed() },
        base: std::ptr::null_mut(),
        ob_shape: [0; crate::hooks::MOLT_BUFFER_MAX_NDIM],
        ob_strides: [0; crate::hooks::MOLT_BUFFER_MAX_NDIM],
        ob_format: [0; crate::hooks::MOLT_BUFFER_FORMAT_CAP],
    }))
}

/// Free a memoryview object whose view was never successfully filled (the
/// constructor's error path). The zeroed/reset view owns nothing.
unsafe fn free_unfilled_memoryview(mv: *mut PyMemoryViewObject) {
    unsafe { drop(Box::from_raw(mv)) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromMemory(
    mem: *mut c_char,
    size: Py_ssize_t,
    flags: c_int,
) -> *mut PyObject {
    if size < 0 || (mem.is_null() && size != 0) {
        return std::ptr::null_mut();
    }
    let readonly = (flags & PyBUF_WRITE == 0) as c_int;
    // The object exists FIRST; FillInfo writes its self-referential view
    // directly into its final home (see `alloc_memoryview_object`).
    let mv = alloc_memoryview_object();
    if unsafe {
        crate::api::buffer::PyBuffer_FillInfo(
            &raw mut (*mv).view,
            std::ptr::null_mut(),
            mem.cast(),
            size,
            readonly,
            PyBUF_FULL_RO,
        )
    } != 0
    {
        unsafe { free_unfilled_memoryview(mv) };
        return std::ptr::null_mut();
    }
    mv.cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromBuffer(info: *mut Py_buffer) -> *mut PyObject {
    if info.is_null() {
        return std::ptr::null_mut();
    }
    let mv = alloc_memoryview_object();
    if unsafe { crate::api::buffer::init_memoryview_from_pybuffer(mv, info.cast_const()) } != 0 {
        unsafe { free_unfilled_memoryview(mv) };
        return std::ptr::null_mut();
    }
    // Pin the exporter for the memoryview's lifetime. CPython treats
    // `info->obj` as a borrowed reference and sets `master.obj = NULL`
    // (Objects/memoryobject.c) — the copied view must NOT re-run the
    // exporter's `bf_releasebuffer` (the caller still owns `info` and its
    // exactly-once release). Molt additionally holds a strong reference in
    // `base`, dropped at dealloc, which is strictly safer than CPython's
    // borrowed pointer.
    unsafe {
        let base = (*info).obj;
        if !base.is_null() {
            crate::api::refcount::Py_INCREF(base);
        }
        (*mv).base = base;
    }
    mv.cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const PyMemoryView_Type)) as c_int
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromObject(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        return std::ptr::null_mut();
    }
    if unsafe { PyMemoryView_Check(op) } != 0 {
        // CPython returns a NEW distinct memoryview sharing the source's buffer
        // (mbuf_add_view), after CHECK_RELEASED — never the same object.
        let src = op.cast::<PyMemoryViewObject>();
        let src_view = unsafe { &(*src).view };
        if src_view.buf.is_null() && src_view.len != 0 {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_ValueError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"operation forbidden on released memoryview object".as_ptr(),
                );
            }
            return std::ptr::null_mut();
        }
        // Share the descriptor: copy the source's Py_buffer struct. Its
        // format/shape/strides pointers keep pointing into the SOURCE object's
        // storage (embedded ob_* arrays, its GetBuffer export internal, or its
        // own self-referential FillInfo fields), which the strong `base`
        // reference on the source keeps alive for the copy's whole lifetime —
        // CPython's mbuf model (the managed buffer outlives every exported
        // view). `internal` stays NULL so PyBuffer_Release on the copy only
        // drops our obj reference; the exporter release still happens exactly
        // once, in the source's dealloc.
        let mv = alloc_memoryview_object();
        unsafe {
            (&raw mut (*mv).view).write(std::ptr::read(&raw const (*src).view));
            crate::api::refcount::Py_INCREF(op);
            (*mv).view.obj = op;
            (*mv).view.internal = std::ptr::null_mut();
            (*mv).base = op;
        }
        return mv.cast();
    }
    // The object exists FIRST and GetBuffer fills its view IN PLACE: a foreign
    // `bf_getbuffer` may fill CPython-style self-referential shape/strides
    // (see `alloc_memoryview_object`), which would dangle if the view were
    // filled on the stack and moved in. Molt-native exports store their
    // descriptor in the heap `internal` allocation, which is position-
    // independent either way.
    let mv = alloc_memoryview_object();
    if unsafe { crate::api::buffer::PyObject_GetBuffer(op, &raw mut (*mv).view, PyBUF_FULL_RO) }
        != 0
    {
        unsafe { free_unfilled_memoryview(mv) };
        return std::ptr::null_mut();
    }
    unsafe {
        let base = if (*mv).view.obj.is_null() {
            crate::api::refcount::Py_INCREF(op);
            op
        } else {
            (*mv).view.obj
        };
        (*mv).view.obj = base;
        (*mv).base = base;
    }
    mv.cast()
}

pub unsafe extern "C" fn molt_memoryview_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let view = op.cast::<PyMemoryViewObject>();
    unsafe {
        // `PyBuffer_Release` drops the reference held in `view.obj` (and, for
        // molt-native exports, the runtime pin + `internal` allocation).
        let obj_before = (*view).view.obj;
        crate::api::buffer::PyBuffer_Release(&raw mut (*view).view);
        // A `PyMemoryView_FromBuffer` view keeps `view.obj` NULL (CPython's
        // `master.obj = NULL` borrowed-reference model) while `base` holds the
        // molt-added exporter pin — drop it here, exactly once. When `base`
        // aliases the released `view.obj` (FromObject / shared-copy paths),
        // the release above already dropped the only reference.
        let base = (*view).base;
        if !base.is_null() && base != obj_before {
            crate::api::refcount::Py_DECREF(base);
        }
        drop(Box::from_raw(view));
    }
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
