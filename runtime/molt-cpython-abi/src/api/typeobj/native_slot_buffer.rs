//! Python buffer slots use the existing native GC and memoryview lease owners.
use super::*;
use crate::api::buffer as api_buffer;

#[repr(C)]
struct PythonBufferExport {
    object: PyObject,
    exporter: *mut PyObject,
    // Stable acquisition address: exporters may point geometry into this field.
    view: Py_buffer,
}

static INITIALIZE: std::sync::Once = std::sync::Once::new();
static mut EXPORT_TYPE: PyTypeObject = unsafe { std::mem::zeroed() };
static mut EXPORT_BUFFER: PyBufferProcs = PyBufferProcs {
    bf_getbuffer: ptr::null_mut(),
    bf_releasebuffer: release_export as *const () as *mut c_void,
};

unsafe fn export_type() -> *mut PyTypeObject {
    INITIALIZE.call_once(|| unsafe {
        EXPORT_TYPE.ob_base.ob_base.ob_refcnt = 1;
        EXPORT_TYPE.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        EXPORT_TYPE.tp_name = c"_buffer_wrapper".as_ptr();
        EXPORT_TYPE.tp_basicsize = std::mem::size_of::<PythonBufferExport>() as Py_ssize_t;
        EXPORT_TYPE.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC;
        EXPORT_TYPE.tp_base = &raw mut PyBaseObject_Type;
        EXPORT_TYPE.tp_dealloc = Some(dealloc);
        EXPORT_TYPE.tp_traverse = Some(traverse);
        EXPORT_TYPE.tp_clear = Some(clear);
        EXPORT_TYPE.tp_alloc = Some(PyType_GenericAlloc);
        EXPORT_TYPE.tp_free = Some(memory::PyObject_GC_Del);
        EXPORT_TYPE.tp_as_buffer = (&raw mut EXPORT_BUFFER).cast();
    });
    let tp = &raw mut EXPORT_TYPE;
    if unsafe { PyType_Ready(tp) } < 0 {
        ptr::null_mut()
    } else {
        tp
    }
}

unsafe extern "C" fn traverse(
    object: *mut PyObject,
    visit: *mut c_void,
    context: *mut c_void,
) -> c_int {
    let visit: unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int =
        unsafe { std::mem::transmute(visit) };
    let export = object.cast::<PythonBufferExport>();
    for edge in unsafe { [(*export).exporter, (*export).view.obj] } {
        if !edge.is_null() {
            let status = unsafe { visit(edge, context) };
            if status != 0 {
                return status;
            }
        }
    }
    0
}

unsafe fn call_python(exporter: *mut PyObject, memoryview: *mut PyObject) {
    unsafe {
        let result = OwnedPyObject::from_owned(invoke::<{ ts::Py_bf_releasebuffer }>(
            exporter,
            0,
            &[memoryview],
            false,
        ));
        if result.as_ptr().is_null() {
            errors::PyErr_WriteUnraisable(exporter);
        }
    }
}

unsafe extern "C" fn clear(object: *mut PyObject) -> c_int {
    errors::with_preserved_error(|| unsafe {
        let export = object.cast::<PythonBufferExport>();
        let exporter =
            OwnedPyObject::from_owned(std::mem::replace(&mut (*export).exporter, ptr::null_mut()));
        let memoryview = OwnedPyObject::from_borrowed((*export).view.obj);
        if memoryview.as_ptr().is_null() {
            return;
        }
        let original = memory::PyMemoryView_GET_BUFFER(memoryview.as_ptr());
        let same_exporter = !original.is_null() && (*original).obj == exporter.as_ptr();
        // Remove the counted memoryview export first. Its retained original
        // object then survives the Python release callback with exact identity.
        api_buffer::PyBuffer_Release(&raw mut (*export).view);
        if !exporter.as_ptr().is_null() && !same_exporter {
            let tp = crate::bridge::semantic_type(exporter.as_ptr());
            if type_bf_releasebuffer(tp)
                .is_some_and(|slot| ptr::fn_addr_eq(slot, release as BfReleaseBuffer))
            {
                call_python(exporter.as_ptr(), memoryview.as_ptr());
            }
        }
    });
    0
}

unsafe extern "C" fn dealloc(object: *mut PyObject) {
    unsafe {
        memory::PyObject_GC_UnTrack(object.cast());
        clear(object);
        memory::PyObject_GC_Del(object.cast());
    }
}

unsafe extern "C" fn release_export(object: *mut PyObject, _view: *mut Py_buffer) {
    unsafe {
        clear(object);
    }
}

pub(super) unsafe extern "C" fn get(
    exporter: *mut PyObject,
    view: *mut Py_buffer,
    flags: c_int,
) -> c_int {
    unsafe {
        let exporter_owner = OwnedPyObject::from_borrowed(exporter);
        let flags = OwnedPyObject::from_owned(numbers::PyLong_FromLongLong(i64::from(flags)));
        if flags.as_ptr().is_null() {
            return -1;
        }
        let result = OwnedPyObject::from_owned(invoke::<{ ts::Py_bf_getbuffer }>(
            exporter,
            0,
            &[flags.as_ptr()],
            false,
        ));
        if result.as_ptr().is_null() {
            return -1;
        }
        if memory::PyMemoryView_Check(result.as_ptr()) == 0 {
            errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast(),
                c"__buffer__ returned non-memoryview object".as_ptr(),
            );
            return -1;
        }
        let tp = export_type();
        if tp.is_null() {
            return -1;
        }
        let wrapper = OwnedPyObject::from_owned(memory::_PyObject_GC_New(tp));
        if wrapper.as_ptr().is_null() {
            return -1;
        }
        let export = wrapper.as_ptr().cast::<PythonBufferExport>();
        (*export).exporter = ptr::null_mut();
        (&raw mut (*export).view).write(std::mem::zeroed());
        if api_buffer::PyObject_GetBuffer(
            result.as_ptr(),
            &raw mut (*export).view,
            numbers::PyLong_AsLong(flags.as_ptr()) as c_int,
        ) < 0
        {
            // Failed exporters own their cleanup; do not release partial state.
            (&raw mut (*export).view).write(std::mem::zeroed());
            return -1;
        }
        (*export).exporter = exporter_owner.into_ptr();
        view.write((&raw const (*export).view).read());
        (*view).obj = wrapper.into_ptr();
        memory::PyObject_GC_Track(export.cast());
        0
    }
}

pub(super) unsafe extern "C" fn release(exporter: *mut PyObject, view: *mut Py_buffer) {
    errors::with_preserved_error(|| unsafe {
        let owner = OwnedPyObject::from_borrowed(exporter);
        let temporary = OwnedPyObject::from_owned(memory::restricted_memoryview_from_buffer(view));
        if temporary.as_ptr().is_null() {
            errors::PyErr_WriteUnraisable(exporter);
        } else {
            call_python(exporter, temporary.as_ptr());
            let released =
                OwnedPyObject::from_owned(memory::memoryview_release(temporary.as_ptr()));
            if released.as_ptr().is_null() {
                errors::PyErr_WriteUnraisable(exporter);
            }
        }
        // Python overrides cannot omit a native base's resource release.
        let tp = crate::bridge::semantic_type(owner.as_ptr());
        if tp.is_null() {
            return;
        }
        let mro = OwnedPyObject::from_borrowed((*tp).tp_mro);
        if mro.as_ptr().is_null() {
            return;
        }
        let count = sequences::PyTuple_Size(mro.as_ptr());
        for index in 1..count {
            let base = sequences::PyTuple_GetItem(mro.as_ptr(), index).cast::<PyTypeObject>();
            if let Some(slot) = type_bf_releasebuffer(base)
                && !ptr::fn_addr_eq(slot, release as BfReleaseBuffer)
            {
                slot(exporter, view);
                break;
            }
        }
        if descriptors::pending() {
            errors::PyErr_WriteUnraisable(exporter);
        }
    });
}
