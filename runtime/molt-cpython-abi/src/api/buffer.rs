//! Buffer protocol entrypoints backed by the runtime-owned typed strided export.

use crate::abi_types::{
    Py_buffer, PyBUF_ANY_CONTIGUOUS, PyBUF_C_CONTIGUOUS, PyBUF_F_CONTIGUOUS, PyBUF_FORMAT,
    PyBUF_ND, PyBUF_STRIDES, PyBUF_WRITABLE, PyExc_BufferError, PyExc_TypeError, PyObject,
};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::{MOLT_BUFFER_FORMAT_CAP, MOLT_BUFFER_MAX_NDIM, MoltBufferView, hooks_or_stubs};
use std::collections::HashSet;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::{LazyLock, Mutex};

const PYBUF_C_CONTIGUOUS_BIT: c_int = PyBUF_C_CONTIGUOUS & !PyBUF_STRIDES;
const PYBUF_F_CONTIGUOUS_BIT: c_int = PyBUF_F_CONTIGUOUS & !PyBUF_STRIDES;
const PYBUF_ANY_CONTIGUOUS_BIT: c_int = PyBUF_ANY_CONTIGUOUS & !PyBUF_STRIDES;
static BUFFER_INTERNAL_REGISTRY: LazyLock<Mutex<HashSet<usize>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

unsafe fn set_buffer_error(message: &'static [u8]) {
    unsafe {
        crate::api::errors::PyErr_SetString(&raw mut PyExc_BufferError, message.as_ptr().cast());
    }
}

unsafe fn set_type_error(message: &'static [u8]) {
    unsafe {
        crate::api::errors::PyErr_SetString(&raw mut PyExc_TypeError, message.as_ptr().cast());
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BufferReleaseKind {
    Runtime,
    Raw,
}

struct BufferInternal {
    release_kind: BufferReleaseKind,
    descriptor: MoltBufferView,
}

fn register_buffer_internal(ptr: *mut BufferInternal) {
    if let Ok(mut registry) = BUFFER_INTERNAL_REGISTRY.lock() {
        registry.insert(ptr as usize);
    }
}

fn unregister_buffer_internal(ptr: *mut std::ffi::c_void) -> bool {
    if let Ok(mut registry) = BUFFER_INTERNAL_REGISTRY.lock() {
        return registry.remove(&(ptr as usize));
    }
    false
}

fn is_registered_buffer_internal(ptr: *mut std::ffi::c_void) -> bool {
    if let Ok(registry) = BUFFER_INTERNAL_REGISTRY.lock() {
        return registry.contains(&(ptr as usize));
    }
    false
}

impl BufferInternal {
    fn runtime(descriptor: MoltBufferView) -> Self {
        Self {
            release_kind: BufferReleaseKind::Runtime,
            descriptor,
        }
    }

    fn raw_descriptor(descriptor: MoltBufferView) -> Self {
        Self {
            release_kind: BufferReleaseKind::Raw,
            descriptor,
        }
    }
}

fn raw_1d_descriptor(
    buf: *mut std::ffi::c_void,
    len: isize,
    readonly: c_int,
    base: u64,
) -> MoltBufferView {
    let mut descriptor = MoltBufferView {
        data: buf.cast(),
        len: len as u64,
        backing_capacity: len as u64,
        readonly: u32::from(readonly != 0),
        ndim: 1,
        itemsize: 1,
        base,
        ..Default::default()
    };
    descriptor.shape[0] = len;
    descriptor.strides[0] = 1;
    descriptor.format[0] = b'B';
    descriptor.format[1] = 0;
    descriptor
}

unsafe fn descriptor_from_pybuffer(info: *const Py_buffer) -> Result<MoltBufferView, ()> {
    if info.is_null() {
        return Err(());
    }
    let info = unsafe { &*info };
    if info.len < 0 || info.itemsize <= 0 || info.ndim < 0 {
        return Err(());
    }
    if info.buf.is_null() && info.len != 0 {
        return Err(());
    }
    if !info.suboffsets.is_null() {
        return Err(());
    }
    if !info.internal.is_null() && is_registered_buffer_internal(info.internal) {
        return Ok(unsafe { (*info.internal.cast::<BufferInternal>()).descriptor });
    }
    let ndim = info.ndim as usize;
    if ndim > MOLT_BUFFER_MAX_NDIM {
        return Err(());
    }

    let mut descriptor = MoltBufferView {
        data: info.buf.cast(),
        len: info.len as u64,
        backing_capacity: info.len as u64,
        readonly: u32::from(info.readonly != 0),
        ndim: ndim as u32,
        itemsize: info.itemsize as u64,
        base: if info.obj.is_null() {
            0
        } else {
            GLOBAL_BRIDGE
                .lock()
                .pyobj_to_handle(info.obj)
                .unwrap_or_default()
        },
        ..Default::default()
    };

    if ndim == 0 {
        // Scalar buffers preserve CPython's zero-rank descriptor shape.
    } else if !info.shape.is_null() {
        for i in 0..ndim {
            let dim = unsafe { *info.shape.add(i) };
            if dim < 0 {
                return Err(());
            }
            descriptor.shape[i] = dim;
        }
    } else {
        descriptor.shape[0] = info.len / info.itemsize;
        for i in 1..ndim {
            descriptor.shape[i] = 1;
        }
    }

    if ndim == 0 {
        // Scalar buffers have no stride entries.
    } else if !info.strides.is_null() {
        for i in 0..ndim {
            descriptor.strides[i] = unsafe { *info.strides.add(i) };
        }
    } else {
        let mut stride = info.itemsize;
        for i in (0..ndim).rev() {
            descriptor.strides[i] = stride;
            let dim = descriptor.shape[i].max(1);
            stride = stride.checked_mul(dim).ok_or(())?;
        }
    }
    // NOTE: no C-contiguity requirement — CPython's PyMemoryView_FromBuffer
    // preserves arbitrary strides (Fortran order, sliced/strided exporters);
    // the captured strides above carry the layout. Only suboffset (PIL-style)
    // buffers are rejected earlier: `MoltBufferView` has no suboffsets field,
    // so that case fails closed with BufferError rather than mis-describing.

    if !info.format.is_null() {
        let bytes = unsafe { CStr::from_ptr(info.format) }.to_bytes();
        if bytes.len() >= MOLT_BUFFER_FORMAT_CAP {
            return Err(());
        }
        let copy_len = bytes.len().min(MOLT_BUFFER_FORMAT_CAP.saturating_sub(1));
        descriptor.format = [0; MOLT_BUFFER_FORMAT_CAP];
        descriptor.format[..copy_len].copy_from_slice(&bytes[..copy_len]);
    }

    Ok(descriptor)
}

unsafe fn reset_pybuffer(view: *mut Py_buffer) {
    unsafe {
        ptr::write_bytes(view, 0, 1);
        (*view).itemsize = 1;
        (*view).readonly = 1;
    }
}

unsafe fn apply_molt_view(
    view: *mut Py_buffer,
    obj: *mut PyObject,
    descriptor: &mut MoltBufferView,
    flags: c_int,
) {
    unsafe {
        (*view).buf = descriptor.data.cast();
        (*view).obj = obj;
        (*view).len = descriptor.len as isize;
        (*view).itemsize = descriptor.itemsize as isize;
        (*view).readonly = descriptor.readonly as c_int;
        (*view).ndim = descriptor.ndim as c_int;
        (*view).format = if (flags & PyBUF_FORMAT) != 0 {
            descriptor.format.as_mut_ptr().cast::<c_char>()
        } else {
            ptr::null_mut()
        };
        (*view).shape = if (flags & (PyBUF_ND | PyBUF_STRIDES)) != 0 {
            descriptor.shape.as_mut_ptr()
        } else {
            ptr::null_mut()
        };
        (*view).strides = if (flags & PyBUF_STRIDES) != 0 {
            descriptor.strides.as_mut_ptr()
        } else {
            ptr::null_mut()
        };
        (*view).suboffsets = ptr::null_mut();
    }
}

unsafe fn install_buffer_internal(
    view: *mut Py_buffer,
    obj: *mut PyObject,
    mut internal: Box<BufferInternal>,
    flags: c_int,
) -> c_int {
    unsafe {
        let descriptor_ok = descriptor_satisfies_flags(&internal.descriptor, flags);
        apply_molt_view(view, obj, &mut internal.descriptor, flags);
        let internal_ptr = Box::into_raw(internal);
        register_buffer_internal(internal_ptr);
        (*view).internal = internal_ptr.cast();
        if !obj.is_null() {
            crate::api::refcount::Py_INCREF(obj);
        }
        if !descriptor_ok || !pybuffer_satisfies_flags(view, flags) {
            PyBuffer_Release(view);
            set_buffer_error(b"non-contiguous buffers require PyBUF_STRIDES\0");
            return -1;
        }
    }
    0
}

unsafe fn pybuffer_is_c_contiguous(view: *const Py_buffer) -> bool {
    if view.is_null() || unsafe { (*view).ndim } == 0 {
        return true;
    }
    if unsafe { (*view).shape.is_null() || (*view).strides.is_null() } {
        return true;
    }
    let ndim = unsafe { (*view).ndim as usize };
    let mut expected = unsafe { (*view).itemsize.max(1) };
    for i in (0..ndim).rev() {
        let dim = unsafe { *(*view).shape.add(i) };
        let stride = unsafe { *(*view).strides.add(i) };
        if dim > 1 && stride != expected {
            return false;
        }
        let Some(next_expected) = expected.checked_mul(dim.max(1)) else {
            return false;
        };
        expected = next_expected;
    }
    true
}

unsafe fn pybuffer_is_f_contiguous(view: *const Py_buffer) -> bool {
    if view.is_null() || unsafe { (*view).ndim } == 0 {
        return true;
    }
    if unsafe { (*view).shape.is_null() || (*view).strides.is_null() } {
        return true;
    }
    let ndim = unsafe { (*view).ndim as usize };
    let mut expected = unsafe { (*view).itemsize.max(1) };
    for i in 0..ndim {
        let dim = unsafe { *(*view).shape.add(i) };
        let stride = unsafe { *(*view).strides.add(i) };
        if dim > 1 && stride != expected {
            return false;
        }
        let Some(next_expected) = expected.checked_mul(dim.max(1)) else {
            return false;
        };
        expected = next_expected;
    }
    true
}

fn descriptor_is_c_contiguous(descriptor: &MoltBufferView) -> bool {
    if descriptor.ndim == 0 {
        return true;
    }
    let ndim = descriptor.ndim as usize;
    if ndim > MOLT_BUFFER_MAX_NDIM {
        return false;
    }
    let Ok(mut expected) = isize::try_from(descriptor.itemsize.max(1)) else {
        return false;
    };
    for i in (0..ndim).rev() {
        let dim = descriptor.shape[i];
        let stride = descriptor.strides[i];
        if dim > 1 && stride != expected {
            return false;
        }
        let Some(next_expected) = expected.checked_mul(dim.max(1)) else {
            return false;
        };
        expected = next_expected;
    }
    true
}

fn descriptor_is_f_contiguous(descriptor: &MoltBufferView) -> bool {
    if descriptor.ndim == 0 {
        return true;
    }
    let ndim = descriptor.ndim as usize;
    if ndim > MOLT_BUFFER_MAX_NDIM {
        return false;
    }
    let Ok(mut expected) = isize::try_from(descriptor.itemsize.max(1)) else {
        return false;
    };
    for i in 0..ndim {
        let dim = descriptor.shape[i];
        let stride = descriptor.strides[i];
        if dim > 1 && stride != expected {
            return false;
        }
        let Some(next_expected) = expected.checked_mul(dim.max(1)) else {
            return false;
        };
        expected = next_expected;
    }
    true
}

fn descriptor_satisfies_flags(descriptor: &MoltBufferView, flags: c_int) -> bool {
    if (flags & PyBUF_STRIDES) == 0 && !descriptor_is_c_contiguous(descriptor) {
        return false;
    }
    if (flags & PYBUF_C_CONTIGUOUS_BIT) != 0 && !descriptor_is_c_contiguous(descriptor) {
        return false;
    }
    if (flags & PYBUF_F_CONTIGUOUS_BIT) != 0 && !descriptor_is_f_contiguous(descriptor) {
        return false;
    }
    if (flags & PYBUF_ANY_CONTIGUOUS_BIT) != 0
        && !descriptor_is_c_contiguous(descriptor)
        && !descriptor_is_f_contiguous(descriptor)
    {
        return false;
    }
    true
}

unsafe fn pybuffer_satisfies_flags(view: *const Py_buffer, flags: c_int) -> bool {
    if (flags & PYBUF_C_CONTIGUOUS_BIT) != 0 && !unsafe { pybuffer_is_c_contiguous(view) } {
        return false;
    }
    if (flags & PYBUF_F_CONTIGUOUS_BIT) != 0 && !unsafe { pybuffer_is_f_contiguous(view) } {
        return false;
    }
    if (flags & PYBUF_ANY_CONTIGUOUS_BIT) != 0
        && !unsafe { pybuffer_is_c_contiguous(view) }
        && !unsafe { pybuffer_is_f_contiguous(view) }
    {
        return false;
    }
    true
}

type BfGetBuffer = unsafe extern "C" fn(*mut PyObject, *mut Py_buffer, c_int) -> c_int;
type BfReleaseBuffer = unsafe extern "C" fn(*mut PyObject, *mut Py_buffer);

/// Read a foreign object's `tp_as_buffer->bf_getbuffer` slot, if any.
unsafe fn foreign_bf_getbuffer(obj: *mut PyObject) -> Option<BfGetBuffer> {
    let tp = unsafe { (*obj).ob_type };
    if tp.is_null() {
        return None;
    }
    let pb = unsafe { (*tp).tp_as_buffer }.cast::<crate::abi_types::PyBufferProcs>();
    if pb.is_null() {
        return None;
    }
    let raw = unsafe { (*pb).bf_getbuffer };
    if raw.is_null() {
        return None;
    }
    Some(unsafe { std::mem::transmute::<*mut std::ffi::c_void, BfGetBuffer>(raw) })
}

/// Read a foreign object's `tp_as_buffer->bf_releasebuffer` slot, if any.
unsafe fn foreign_bf_releasebuffer(obj: *mut PyObject) -> Option<BfReleaseBuffer> {
    let tp = unsafe { (*obj).ob_type };
    if tp.is_null() {
        return None;
    }
    let pb = unsafe { (*tp).tp_as_buffer }.cast::<crate::abi_types::PyBufferProcs>();
    if pb.is_null() {
        return None;
    }
    let raw = unsafe { (*pb).bf_releasebuffer };
    if raw.is_null() {
        return None;
    }
    Some(unsafe { std::mem::transmute::<*mut std::ffi::c_void, BfReleaseBuffer>(raw) })
}

/// CPython's no-buffer-slot failure: PyErr_Format(TypeError,
/// "a bytes-like object is required, not '%.100s'").
unsafe fn raise_bytes_like_type_error(obj: *mut PyObject) {
    let tp = unsafe { (*obj).ob_type };
    let name = if tp.is_null() || unsafe { (*tp).tp_name }.is_null() {
        "object".to_string()
    } else {
        unsafe { CStr::from_ptr((*tp).tp_name) }
            .to_string_lossy()
            .into_owned()
    };
    let msg = format!(
        "a bytes-like object is required, not '{:.100}'",
        name
    );
    if let Ok(c) = std::ffi::CString::new(msg) {
        unsafe { crate::api::errors::PyErr_SetString(&raw mut PyExc_TypeError, c.as_ptr()) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetBuffer(
    obj: *mut PyObject,
    view: *mut Py_buffer,
    flags: c_int,
) -> c_int {
    if view.is_null() {
        unsafe { set_type_error(b"buffer view must not be NULL\0") };
        return -1;
    }
    unsafe { reset_pybuffer(view) };
    if obj.is_null() {
        unsafe { set_type_error(b"buffer exporter must not be NULL\0") };
        return -1;
    }
    // Resolve in its own statement (NOT as a `match` scrutinee): a `match
    // GLOBAL_BRIDGE.lock()....` scrutinee keeps the MutexGuard alive for the
    // ENTIRE match statement, including the `None` arm's body (Rust temporary
    // lifetime extension). That arm calls `raise_bytes_like_type_error` ->
    // `PyErr_SetString`, which itself locks `GLOBAL_BRIDGE` — with the outer
    // guard still held, that is an immediate self-deadlock (parking_lot's
    // Mutex is not reentrant). Binding the resolved `Option` first drops the
    // guard before the `None` arm runs.
    let resolved = GLOBAL_BRIDGE.lock().pyobj_to_handle(obj);
    let bits = match resolved {
        Some(bits) => bits,
        None => {
            // Foreign C object: CPython Objects/abstract.c dispatches
            // `(*pb->bf_getbuffer)(obj, view, flags)` — the slot installed by
            // PyType_FromSpec was previously DEAD (no call site), so a
            // C-extension type (numpy's PyArray_Type) could never export a
            // buffer through the standard protocol.
            if let Some(getbuffer) = unsafe { foreign_bf_getbuffer(obj) } {
                return unsafe { getbuffer(obj, view, flags) };
            }
            // No buffer slot: TypeError with CPython's message (was BufferError).
            unsafe { raise_bytes_like_type_error(obj) };
            return -1;
        }
    };
    let hooks = hooks_or_stubs();
    let mut descriptor = MoltBufferView::default();
    if unsafe { (hooks.buffer_acquire)(bits, &mut descriptor as *mut MoltBufferView) } != 0 {
        unsafe { set_buffer_error(b"object does not export a buffer\0") };
        return -1;
    }
    if (flags & PyBUF_WRITABLE) != 0 && descriptor.readonly != 0 {
        unsafe {
            let _ = (hooks.buffer_release)(&mut descriptor as *mut MoltBufferView);
            set_buffer_error(b"writable buffer requested for readonly object\0");
        }
        return -1;
    }
    if unsafe {
        install_buffer_internal(
            view,
            obj,
            Box::new(BufferInternal::runtime(descriptor)),
            flags,
        )
    } != 0
    {
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_Release(view: *mut Py_buffer) {
    if view.is_null() {
        return;
    }
    unsafe {
        if !(*view).internal.is_null() && unregister_buffer_internal((*view).internal) {
            // Molt-registered view: release through the runtime hook.
            let mut internal = Box::from_raw((*view).internal.cast::<BufferInternal>());
            if internal.release_kind == BufferReleaseKind::Runtime {
                let _ = (hooks_or_stubs().buffer_release)(
                    &mut internal.descriptor as *mut MoltBufferView,
                );
            }
        } else if !(*view).obj.is_null() {
            // View filled by a C-extension bf_getbuffer: CPython calls
            // `pb->bf_releasebuffer(obj, view)` when present, BEFORE the obj
            // DECREF — skipping it imbalances the exporter's refcount/resources.
            if let Some(releasebuffer) = foreign_bf_releasebuffer((*view).obj) {
                releasebuffer((*view).obj, view);
            }
        }
        (*view).internal = ptr::null_mut();
        if !(*view).obj.is_null() {
            crate::api::refcount::Py_DECREF((*view).obj);
            (*view).obj = ptr::null_mut();
        }
        reset_pybuffer(view);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CheckBuffer(obj: *mut PyObject) -> c_int {
    // CPython: a PURE pointer test — `tp_as_buffer && bf_getbuffer` — with no
    // acquisition, no release, and NO mutation of the error indicator. The old
    // body actually acquired+released the buffer (real side effects) and called
    // PyErr_Clear() on failure, clobbering any pending exception.
    if obj.is_null() {
        return 0;
    }
    let bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(obj);
    match bits {
        None => {
            // Foreign object: honest slot test.
            (unsafe { foreign_bf_getbuffer(obj) }).is_some() as c_int
        }
        Some(bits) => {
            // Molt-native: side-effect-free classification. The runtime buffer
            // exporters are the bytes-like natives; memoryview/bytearray are
            // raw ABI objects and never reach this arm.
            if unsafe { crate::api::memory::PyMemoryView_Check(obj) } != 0
                || unsafe { crate::api::strings::PyByteArray_Check(obj) } != 0
            {
                return 1;
            }
            let tag = unsafe { (hooks_or_stubs().classify_heap)(bits) };
            (tag == crate::abi_types::MoltTypeTag::Bytes as u8) as c_int
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_IsContiguous(
    view: *const Py_buffer,
    order: std::os::raw::c_char,
) -> c_int {
    if view.is_null() {
        return 0;
    }
    // CPython Objects/abstract.c: a view with suboffsets is NEVER contiguous,
    // and a zero-length view is ALWAYS contiguous (both checked up front).
    if !unsafe { (*view).suboffsets }.is_null() {
        return 0;
    }
    if unsafe { (*view).len } == 0 {
        return 1;
    }
    match order as u8 {
        b'C' | b'c' => unsafe { pybuffer_is_c_contiguous(view) as c_int },
        b'F' | b'f' => unsafe { pybuffer_is_f_contiguous(view) as c_int },
        _ => unsafe { (pybuffer_is_c_contiguous(view) || pybuffer_is_f_contiguous(view)) as c_int },
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_FillInfo(
    view: *mut Py_buffer,
    obj: *mut PyObject,
    buf: *mut std::ffi::c_void,
    len: isize,
    readonly: c_int,
    flags: c_int,
) -> c_int {
    // CPython Objects/abstract.c: the NULL-view path SETS BufferError
    // ("PyBuffer_FillInfo: view==NULL argument is obsolete"); no len/buf
    // pre-validation is performed — FillInfo accepts whatever the caller
    // declares (the only semantic check is writable-vs-readonly).
    if view.is_null() {
        unsafe { set_buffer_error(b"PyBuffer_FillInfo: view==NULL argument is obsolete\0") };
        return -1;
    }
    if len < 0 {
        // Defensive (not in CPython, but a negative length would poison every
        // downstream usize cast); fail with an exception rather than bare -1.
        unsafe { set_buffer_error(b"buffer length must not be negative\0") };
        return -1;
    }
    if (flags & PyBUF_WRITABLE) != 0 && readonly != 0 {
        unsafe { set_buffer_error(b"Object is not writable.\0") };
        return -1;
    }
    let base = if obj.is_null() {
        0
    } else {
        GLOBAL_BRIDGE.lock().pyobj_to_handle(obj).unwrap_or_default()
    };
    unsafe {
        reset_pybuffer(view);
        install_buffer_internal(
            view,
            obj,
            Box::new(BufferInternal::raw_descriptor(raw_1d_descriptor(
                buf, len, readonly, base,
            ))),
            flags,
        )
    }
}

pub(crate) unsafe fn copy_pybuffer_for_memoryview(
    view: *mut Py_buffer,
    info: *const Py_buffer,
) -> c_int {
    if view.is_null() || info.is_null() {
        unsafe { set_type_error(b"memoryview buffer must not be NULL\0") };
        return -1;
    }
    let descriptor = match unsafe { descriptor_from_pybuffer(info) } {
        Ok(descriptor) => descriptor,
        Err(()) => {
            unsafe { set_buffer_error(b"invalid buffer descriptor for memoryview\0") };
            return -1;
        }
    };
    let obj = unsafe { (*info).obj };
    unsafe {
        reset_pybuffer(view);
        install_buffer_internal(
            view,
            obj,
            Box::new(BufferInternal::raw_descriptor(descriptor)),
            PyBUF_FORMAT | PyBUF_STRIDES,
        )
    }
}
