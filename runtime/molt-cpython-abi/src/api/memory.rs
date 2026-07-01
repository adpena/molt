//! CPython memory allocator ABI.

use crate::abi_types::{
    Py_buffer, Py_ssize_t, PyBUF_FULL_RO, PyBUF_WRITABLE, PyMemoryView_Type, PyMemoryViewObject,
    PyObject, PyTypeObject, PyVarObject,
};
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Malloc(size: usize) -> *mut c_void {
    unsafe { libc::malloc(size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Calloc(nelem: usize, elsize: usize) -> *mut c_void {
    unsafe { libc::calloc(nelem, elsize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMem_Realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    unsafe { libc::realloc(ptr, new_size) }
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
        return std::ptr::null_mut();
    };
    let raw = unsafe { PyMem_Calloc(1, size) }.cast::<PyObject>();
    if raw.is_null() {
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
    if op.is_null() || typeobj.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        (*op).ob_refcnt = 1;
        (*op).ob_type = typeobj;
    }
    op
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_InitVar(
    op: *mut PyVarObject,
    typeobj: *mut PyTypeObject,
    size: Py_ssize_t,
) -> *mut PyVarObject {
    if op.is_null() || typeobj.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        (*op).ob_base.ob_refcnt = 1;
        (*op).ob_base.ob_type = typeobj;
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
        unsafe { finalize(op) };
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_EnterRecursiveCall(_where: *const c_char) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_LeaveRecursiveCall() {}

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
    let readonly = (flags & PyBUF_WRITABLE == 0) as c_int;
    if unsafe {
        crate::api::buffer::PyBuffer_FillInfo(
            &mut view,
            std::ptr::null_mut(),
            mem.cast(),
            size,
            readonly,
            PyBUF_FULL_RO | (flags & PyBUF_WRITABLE),
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
        unsafe { crate::api::refcount::Py_INCREF(op) };
        return op;
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
