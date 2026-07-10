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
    unsafe { libc::malloc(size.max(1)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Calloc(nelem: usize, elsize: usize) -> *mut c_void {
    // CPython: a 0-element/0-size request still returns a unique pointer.
    if nelem == 0 || elsize == 0 {
        return unsafe { libc::calloc(1, 1) };
    }
    unsafe { libc::calloc(nelem, elsize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    // CPython: Realloc(p, 0) behaves like Realloc(p, 1) — it never frees `p`
    // and never returns NULL-on-success (realloc(p, 0) may do both in C).
    unsafe { libc::realloc(ptr, new_size.max(1)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Free(ptr: *mut c_void) {
    unsafe { libc::free(ptr) };
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
    unsafe { PyMem_Free(ptr) };
}

/// CPython `PyObject_Free` — release an object's memory. In CPython this is the
/// non-GC object deallocator (`object`'s default `tp_free`); Molt routes it
/// through the same `libc::free` path as `PyMem_Free`/`PyObject_GC_Del` (there
/// is no separate obmalloc arena or GC tracking in the wasm runtime). Provided
/// so `PyType_Ready` can install CPython's `tp_free` default: a static
/// C-extension type that leaves `tp_free` NULL (e.g. numpy's
/// `PyBoundArrayMethod_Type`) inherits `object.tp_free == PyObject_Free`, and
/// its `tp_dealloc`'s `Py_TYPE(self)->tp_free(self)` must resolve.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Free(ptr: *mut c_void) {
    unsafe { PyMem_Free(ptr) };
}

pub(crate) unsafe fn molt_object_alloc(
    typeobj: *mut PyTypeObject,
    nitems: Py_ssize_t,
) -> *mut PyObject {
    if typeobj.is_null() {
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
    if nitems > 0 || itemsize > 0 {
        unsafe { PyObject_InitVar(raw.cast::<PyVarObject>(), typeobj, nitems) }.cast::<PyObject>()
    } else {
        unsafe { PyObject_Init(raw, typeobj) }
    }
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
pub unsafe extern "C" fn PyObject_GC_Track(_op: *mut c_void) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_UnTrack(_op: *mut c_void) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GC_IsFinalized(_op: *mut PyObject) -> c_int {
    0
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
        // CPython Objects/object.c: temporarily resurrect to refcount 1 so the
        // finalizer runs against a live object, then detect resurrection — a
        // finalizer that stored a new reference leaves refcnt > 1, and the
        // deallocator must ABORT the free (return -1) instead of freeing a
        // live object (use-after-free).
        unsafe { (*op).ob_refcnt = 1 };
        unsafe { finalize(op) };
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
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyGC_Enable() {}

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
                    &raw mut crate::abi_types::PyExc_RecursionError,
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromMemory(
    mem: *mut c_char,
    size: Py_ssize_t,
    flags: c_int,
) -> *mut PyObject {
    if size < 0 || (mem.is_null() && size != 0) {
        return std::ptr::null_mut();
    }
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    let readonly = (flags & PyBUF_WRITE == 0) as c_int;
    if unsafe {
        crate::api::buffer::PyBuffer_FillInfo(
            &mut view,
            std::ptr::null_mut(),
            mem.cast(),
            size,
            readonly,
            PyBUF_FULL_RO,
        )
    } != 0
    {
        return std::ptr::null_mut();
    }
    let object = Box::new(PyMemoryViewObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyMemoryView_Type,
        },
        view,
        base: std::ptr::null_mut(),
    });
    Box::into_raw(object).cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromBuffer(info: *mut Py_buffer) -> *mut PyObject {
    if info.is_null() {
        return std::ptr::null_mut();
    }
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    if unsafe { crate::api::buffer::copy_pybuffer_for_memoryview(&mut view, info.cast_const()) }
        != 0
    {
        return std::ptr::null_mut();
    }
    let base = view.obj;
    let object = Box::new(PyMemoryViewObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyMemoryView_Type,
        },
        view,
        base,
    });
    Box::into_raw(object).cast()
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
                    &raw mut crate::abi_types::PyExc_ValueError,
                    c"operation forbidden on released memoryview object".as_ptr(),
                );
            }
            return std::ptr::null_mut();
        }
        // Share the descriptor; the new view holds its own reference to the
        // SOURCE memoryview (obj), so the exporter release still happens
        // exactly once — in the source's dealloc. `internal` stays NULL so
        // PyBuffer_Release on the copy only drops our obj reference.
        let mut view: Py_buffer = unsafe { std::ptr::read(&raw const (*src).view) };
        unsafe { crate::api::refcount::Py_INCREF(op) };
        view.obj = op;
        view.internal = std::ptr::null_mut();
        let object = Box::new(PyMemoryViewObject {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut PyMemoryView_Type,
            },
            view,
            base: op,
        });
        return Box::into_raw(object).cast();
    }
    let mut view = Py_buffer {
        buf: std::ptr::null_mut(),
        obj: std::ptr::null_mut(),
        len: 0,
        itemsize: 1,
        readonly: 1,
        ndim: 0,
        format: std::ptr::null_mut(),
        shape: std::ptr::null_mut(),
        strides: std::ptr::null_mut(),
        suboffsets: std::ptr::null_mut(),
        internal: std::ptr::null_mut(),
    };
    if unsafe { crate::api::buffer::PyObject_GetBuffer(op, &raw mut view, PyBUF_FULL_RO) } != 0 {
        return std::ptr::null_mut();
    }
    let base = if view.obj.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(op) };
        op
    } else {
        view.obj
    };
    view.obj = base;
    let object = Box::new(PyMemoryViewObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyMemoryView_Type,
        },
        view,
        base,
    });
    Box::into_raw(object).cast()
}

pub unsafe extern "C" fn molt_memoryview_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let view = op.cast::<PyMemoryViewObject>();
    unsafe {
        crate::api::buffer::PyBuffer_Release(&raw mut (*view).view);
        drop(Box::from_raw(view));
    }
}
