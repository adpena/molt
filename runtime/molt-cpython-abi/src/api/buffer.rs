//! Buffer protocol entrypoints backed by the runtime-owned typed strided export.
//!
//! Release discrimination is CPython's model (Objects/abstract.c
//! `PyBuffer_Release` dispatches on the exporter OBJECT `view->obj`): a
//! molt-native exporter's view carries a right-sized [`ExportInternal`]
//! allocation in `view.internal`, a foreign exporter's release goes through its
//! own `bf_releasebuffer` slot, and nothing ever dereferences a foreign
//! `view.internal` (an exporter-private cookie that may be a small integer or
//! unmapped memory). The former global `Mutex<HashSet>` registry of boxed
//! 1112 B `BufferInternal` descriptors is GONE — it existed only to answer
//! "is this internal ours?", which the exporter object itself already answers
//! for free (and without a process-wide serialization point).

use crate::abi_types::{
    Py_buffer, PyBUF_ANY_CONTIGUOUS, PyBUF_C_CONTIGUOUS, PyBUF_F_CONTIGUOUS, PyBUF_FORMAT,
    PyBUF_ND, PyBUF_STRIDES, PyBUF_WRITABLE, PyExc_BufferError, PyExc_TypeError, PyObject,
};
use crate::api::typeobj::{type_bf_getbuffer, type_bf_releasebuffer};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::{MOLT_BUFFER_FORMAT_CAP, MOLT_BUFFER_MAX_NDIM, MoltBufferView, hooks_or_stubs};
use std::alloc::Layout;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

const PYBUF_C_CONTIGUOUS_BIT: c_int = PyBUF_C_CONTIGUOUS & !PyBUF_STRIDES;
const PYBUF_F_CONTIGUOUS_BIT: c_int = PyBUF_F_CONTIGUOUS & !PyBUF_STRIDES;
const PYBUF_ANY_CONTIGUOUS_BIT: c_int = PyBUF_ANY_CONTIGUOUS & !PyBUF_STRIDES;

unsafe fn set_buffer_error(message: &'static [u8]) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut PyExc_BufferError).cast::<crate::abi_types::PyObject>(),
            message.as_ptr().cast(),
        );
    }
}

unsafe fn set_type_error(message: &'static [u8]) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            message.as_ptr().cast(),
        );
    }
}

/// Header of the right-sized per-export allocation installed in
/// `Py_buffer.internal` by [`PyObject_GetBuffer`] for a molt-native exporter.
///
/// The allocation is this header followed by an `[isize; 2 * ndim]` tail
/// (`ndim` shape values, then `ndim` stride values); the C-visible
/// `view.format`/`shape`/`strides` pointers point into it for the whole view
/// lifetime. It replaces the former 1112 B `BufferInternal` box (a wholesale
/// `MoltBufferView` copy): the runtime release hook (`molt_buffer_release`)
/// consumes `owner` and a distinct `base`, and C consumes only `format` plus
/// `ndim`-many shape/stride entries, so that is all we store — a 40 B header
/// plus two isizes per dimension (16 B/dim on 64-bit) instead of a fixed 1112 B.
///
/// `ndim` is stored in the header (self-describing) so release re-derives the
/// exact allocation [`Layout`] from the allocation itself rather than trusting
/// the C-owned `view.ndim` field.
#[repr(C)]
struct ExportInternal {
    /// Runtime pin handle (`MoltBufferView.owner`): the array export lease /
    /// strong ref minted by `molt_buffer_acquire`, dropped exactly once at
    /// release via the `buffer_release` hook.
    owner: u64,
    /// Descriptor base edge retained by acquisition when distinct from owner.
    /// Preserve even equal/zero values: the runtime release hook alone decides
    /// whether this is a second owned reference, avoiding duplicate decrefs.
    base: u64,
    /// Number of dimensions = half the tail length. Bounded by
    /// [`MOLT_BUFFER_MAX_NDIM`], enforced before allocation.
    ndim: u32,
    _reserved: u32,
    format: [u8; MOLT_BUFFER_FORMAT_CAP],
}

/// Layout of the [`ExportInternal`] allocation for `ndim` dimensions, plus the
/// byte offset of the `[isize; 2 * ndim]` dims tail.
fn export_internal_layout(ndim: usize) -> (Layout, usize) {
    debug_assert!(ndim <= MOLT_BUFFER_MAX_NDIM);
    let (layout, dims_offset) = Layout::new::<ExportInternal>()
        .extend(Layout::array::<isize>(2 * ndim).expect("ndim is bounded"))
        .expect("export internal layout");
    (layout.pad_to_align(), dims_offset)
}

/// Raw pointer to the shape tail of `internal` (strides follow at `+ ndim`).
///
/// # Safety
/// `internal` must be a live allocation created by [`export_internal_new`]
/// whose header `ndim` equals `ndim`.
unsafe fn export_internal_dims(internal: *mut ExportInternal, ndim: usize) -> *mut isize {
    let (_, dims_offset) = export_internal_layout(ndim);
    unsafe { internal.cast::<u8>().add(dims_offset).cast::<isize>() }
}

/// Allocate and fill the export internal from an acquired runtime descriptor.
/// Returns null on allocation failure (the caller must drop the runtime pin
/// and raise `MemoryError`).
///
/// Every store goes through RAW pointers derived from the `std::alloc::alloc`
/// pointer — no reference (`&`/`&mut`) is ever formed over the allocation, so
/// the `format`/`shape`/`strides` pointers later published into the C-visible
/// `Py_buffer` carry whole-allocation provenance that no retag can pop. (Miri
/// finding C: the previous Box-based path minted reference-derived interior
/// tags that `Box::into_raw`'s Unique retag invalidated; an `alloc`-derived
/// raw pointer has no such lifecycle. See `docs/agent/MIRI_STRICT_PROVENANCE.md`.)
///
/// # Safety
/// `descriptor.ndim` must be `<= MOLT_BUFFER_MAX_NDIM` (checked by the caller).
unsafe fn export_internal_new(descriptor: &MoltBufferView) -> *mut ExportInternal {
    let ndim = descriptor.ndim as usize;
    let (layout, dims_offset) = export_internal_layout(ndim);
    // SAFETY: the layout has non-zero size (the header alone is 40 bytes).
    let internal = unsafe { std::alloc::alloc(layout) }.cast::<ExportInternal>();
    if internal.is_null() {
        return internal;
    }
    unsafe {
        (&raw mut (*internal).owner).write(descriptor.owner);
        (&raw mut (*internal).base).write(descriptor.base);
        (&raw mut (*internal).ndim).write(descriptor.ndim);
        (&raw mut (*internal)._reserved).write(0);
        (&raw mut (*internal).format).write(descriptor.format);
        let dims = internal.cast::<u8>().add(dims_offset).cast::<isize>();
        for i in 0..ndim {
            dims.add(i).write(descriptor.shape[i]);
            dims.add(ndim + i).write(descriptor.strides[i]);
        }
    }
    internal
}

/// Release a molt-native export: pass the exact acquired owner/base pair to
/// the runtime release hook, then free the right-sized allocation using the
/// layout re-derived from the header's own `ndim`. The hook is supplied by the
/// caller so this operation does not install or mutate a global hook table.
///
/// # Safety
/// `internal` must be a live allocation created by [`export_internal_new`];
/// it is freed here and must not be used afterwards.
unsafe fn export_internal_release(
    internal: *mut ExportInternal,
    release_hook: unsafe extern "C" fn(*mut MoltBufferView) -> c_int,
) {
    unsafe {
        let owner = (&raw const (*internal).owner).read();
        let base = (&raw const (*internal).base).read();
        let ndim = (&raw const (*internal).ndim).read() as usize;
        let mut descriptor = MoltBufferView {
            owner,
            base,
            ..Default::default()
        };
        let _ = release_hook(&mut descriptor as *mut MoltBufferView);
        let (layout, _) = export_internal_layout(ndim);
        std::alloc::dealloc(internal.cast(), layout);
    }
}

/// Borrow one normalized export through a scoped consumer. The canonical
/// Py_buffer stays in place (FillInfo may publish self-pointers), and its one
/// release preserves any conversion error. Byte consumers need geometry, not
/// PEP 3118 format text, so format length cannot reject a valid byte copy.
///
/// # Safety
/// `object` must be a live canonical PyObject for the duration of the call.
/// The consumer must not retain the descriptor or its borrowed data pointers.
pub unsafe fn with_buffer_descriptor<T>(
    object: *mut PyObject,
    consume: impl FnOnce(&MoltBufferView) -> T,
) -> Result<T, crate::ErrorIndicatorSet> {
    struct Release(*mut Py_buffer);
    impl Drop for Release {
        fn drop(&mut self) {
            crate::api::errors::with_preserved_error(|| unsafe { PyBuffer_Release(self.0) });
        }
    }
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    let status =
        unsafe { PyObject_GetBuffer(object, &raw mut view, crate::abi_types::PyBUF_FULL_RO) };
    // A failed exporter owns its cleanup. A completed export is ours even if
    // its callback incorrectly returned success with an exception pending.
    let _release = (status >= 0).then(|| Release(&raw mut view));
    if unsafe { crate::api::errors::check_native_status(status, "native buffer callback") } < 0 {
        return Err(crate::ErrorIndicatorSet);
    }
    match unsafe { descriptor_from_pybuffer(&raw const view) } {
        Ok(descriptor) => Ok(consume(&descriptor)),
        Err(InvalidBufferDescriptor) => {
            unsafe { set_buffer_error(b"invalid or indirect buffer descriptor for bytes\0") };
            Err(crate::ErrorIndicatorSet)
        }
    }
}

/// Buffer geometry cannot be normalized; the consumer chooses the Python error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidBufferDescriptor;

pub unsafe fn descriptor_from_pybuffer(
    info: *const Py_buffer,
) -> Result<MoltBufferView, InvalidBufferDescriptor> {
    if info.is_null() {
        return Err(InvalidBufferDescriptor);
    }
    let info = unsafe { &*info };
    if info.len < 0 || info.itemsize <= 0 || info.ndim < 0 {
        return Err(InvalidBufferDescriptor);
    }
    if info.buf.is_null() && info.len != 0 {
        return Err(InvalidBufferDescriptor);
    }
    if !info.suboffsets.is_null() {
        return Err(InvalidBufferDescriptor);
    }
    let ndim = info.ndim as usize;
    if ndim > MOLT_BUFFER_MAX_NDIM {
        return Err(InvalidBufferDescriptor);
    }

    // NOTE: `view.internal` is NEVER consulted, let alone dereferenced — for a
    // foreign exporter it is a private cookie (possibly a small integer or
    // unmapped memory). The general field-read path below is correct for every
    // conforming `Py_buffer`, including molt's own exports. `base`/`owner`
    // stay 0: this descriptor is a normalization vehicle for the embedded
    // memoryview copy, it never reaches the runtime release hook.
    let mut descriptor = MoltBufferView {
        data: info.buf.cast(),
        len: info.len as u64,
        backing_capacity: info.len as u64,
        readonly: u32::from(info.readonly != 0),
        ndim: ndim as u32,
        itemsize: info.itemsize as u64,
        ..Default::default()
    };

    // CPython-style SELF-REFERENTIAL views (`PyBuffer_FillInfo`:
    // `shape = &view->len`, `strides = &view->itemsize`) are read via the
    // FIELDS, never through their interior pointers. This is a provenance
    // requirement, not a style choice: a Rust caller that re-borrows the
    // `Py_buffer` (`&mut view`) between the fill and this call mints a Unique
    // retag over the struct that pops the stored self-pointer tags (Stacked
    // Borrows), so dereferencing `info.shape` here would read through a dead
    // tag even though the ADDRESS is still correct. Address equality is
    // access-free (tags don't participate), making the detection exact and
    // safe. A self-referential view is 1-D by construction; any other rank
    // with a self-pointer is malformed and fails closed.
    let shape_is_self = ptr::eq(info.shape.cast_const(), &raw const info.len);
    let strides_is_self = ptr::eq(info.strides.cast_const(), &raw const info.itemsize);

    if ndim == 0 {
        // Scalar buffers preserve CPython's zero-rank descriptor shape.
    } else if shape_is_self {
        if ndim != 1 {
            return Err(InvalidBufferDescriptor);
        }
        descriptor.shape[0] = info.len;
    } else if !info.shape.is_null() {
        for i in 0..ndim {
            let dim = unsafe { *info.shape.add(i) };
            if dim < 0 {
                return Err(InvalidBufferDescriptor);
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
    } else if strides_is_self {
        if ndim != 1 {
            return Err(InvalidBufferDescriptor);
        }
        descriptor.strides[0] = info.itemsize;
    } else if !info.strides.is_null() {
        for i in 0..ndim {
            descriptor.strides[i] = unsafe { *info.strides.add(i) };
        }
    } else {
        let mut stride = info.itemsize;
        for i in (0..ndim).rev() {
            descriptor.strides[i] = stride;
            let dim = descriptor.shape[i].max(1);
            stride = stride.checked_mul(dim).ok_or(InvalidBufferDescriptor)?;
        }
    }
    // NOTE: no C-contiguity requirement — CPython's PyMemoryView_FromBuffer
    // preserves arbitrary strides (Fortran order, sliced/strided exporters);
    // the captured strides above carry the layout. Only suboffset (PIL-style)
    // buffers are rejected earlier: `MoltBufferView` has no suboffsets field,
    // so that case fails closed with BufferError rather than mis-describing.

    Ok(descriptor)
}

unsafe fn reset_pybuffer(view: *mut Py_buffer) {
    unsafe {
        ptr::write_bytes(view, 0, 1);
        (*view).itemsize = 1;
        (*view).readonly = 1;
    }
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
    let msg = format!("a bytes-like object is required, not '{:.100}'", name);
    if let Ok(c) = std::ffi::CString::new(msg) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c.as_ptr(),
            )
        };
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
    // Both header facades now enter here. Admit the canonical object prefix
    // before any foreign slot access; registered private storage has no such
    // prefix and cannot acquire the linked buffer protocol by registration.
    let Some(resolved) = crate::bridge::observe_pyobject(obj) else {
        return -1;
    };
    let bits = match resolved {
        crate::bridge::ResolvedPyObject::ManagedMolt(bits) => bits,
        crate::bridge::ResolvedPyObject::Foreign => {
            // Foreign C object: CPython Objects/abstract.c dispatches
            // `(*pb->bf_getbuffer)(obj, view, flags)` — the slot installed by
            // PyType_FromSpec was previously DEAD (no call site), so a
            // C-extension type (numpy's PyArray_Type) could never export a
            // buffer through the standard protocol.
            if let Some(getbuffer) = unsafe { type_bf_getbuffer((*obj).ob_type) } {
                return unsafe { getbuffer(obj, view, flags) };
            }
            // No buffer slot: TypeError with CPython's message (was BufferError).
            unsafe { raise_bytes_like_type_error(obj) };
            return -1;
        }
    };
    let hooks = hooks_or_stubs();
    let mut descriptor = MoltBufferView::default();
    if unsafe { (hooks.buffer_acquire)(bits.bits(), &mut descriptor as *mut MoltBufferView) } != 0 {
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { set_buffer_error(b"object does not export a buffer\0") };
        }
        return -1;
    }
    if descriptor.ndim as usize > MOLT_BUFFER_MAX_NDIM {
        // Fail closed BEFORE any shape/strides indexing or allocation sizing:
        // a descriptor beyond the inline capacity cannot be represented.
        unsafe {
            let _ = (hooks.buffer_release)(&mut descriptor as *mut MoltBufferView);
            set_buffer_error(b"buffer descriptor exceeds the supported dimension cap\0");
        }
        return -1;
    }
    if (flags & PyBUF_WRITABLE) != 0 && descriptor.readonly != 0 {
        unsafe {
            let _ = (hooks.buffer_release)(&mut descriptor as *mut MoltBufferView);
            set_buffer_error(b"writable buffer requested for readonly object\0");
        }
        return -1;
    }
    // Contiguity is validated on the descriptor VALUES before anything is
    // allocated or published. (The view-level re-check the old install path
    // did was strictly weaker: it read back the same values through the
    // freshly published pointers, or vacuously passed where flags left them
    // NULL.)
    if !descriptor_satisfies_flags(&descriptor, flags) {
        unsafe {
            let _ = (hooks.buffer_release)(&mut descriptor as *mut MoltBufferView);
            set_buffer_error(b"non-contiguous buffers require PyBUF_STRIDES\0");
        }
        return -1;
    }
    // SAFETY: ndim capped above.
    let internal = unsafe { export_internal_new(&descriptor) };
    if internal.is_null() {
        unsafe {
            let _ = (hooks.buffer_release)(&mut descriptor as *mut MoltBufferView);
            crate::api::errors::PyErr_NoMemory();
        }
        return -1;
    }
    // Publish the view. `format`/`shape`/`strides` are raw projections into
    // the `internal` allocation (see `export_internal_new` for the provenance
    // contract); everything else is copied by value.
    let ndim = descriptor.ndim as usize;
    unsafe {
        (*view).buf = descriptor.data.cast();
        (*view).obj = obj;
        (*view).len = descriptor.len as isize;
        (*view).itemsize = descriptor.itemsize as isize;
        (*view).readonly = descriptor.readonly as c_int;
        (*view).ndim = descriptor.ndim as c_int;
        let dims = export_internal_dims(internal, ndim);
        (*view).format = if (flags & PyBUF_FORMAT) != 0 {
            (&raw mut (*internal).format).cast::<c_char>()
        } else {
            ptr::null_mut()
        };
        (*view).shape = if (flags & (PyBUF_ND | PyBUF_STRIDES)) != 0 {
            dims
        } else {
            ptr::null_mut()
        };
        (*view).strides = if (flags & PyBUF_STRIDES) != 0 {
            dims.add(ndim)
        } else {
            ptr::null_mut()
        };
        (*view).suboffsets = ptr::null_mut();
        if (flags & PyBUF_FORMAT) != 0 && crate::api::memory::PyMemoryView_Check(obj) != 0 {
            (*view).format = (*crate::api::memory::PyMemoryView_GET_BUFFER(obj)).format;
        }
        (*view).internal = internal.cast();
        crate::api::refcount::Py_INCREF(obj);
    }
    0
}

/// Retire the managed export lease without consuming the public view's owned
/// exporter reference. Installed base release slots and PyBuffer_Release use
/// this same transaction; only PyBuffer_Release retires view.obj.
pub(crate) unsafe fn release_managed_export(view: *mut Py_buffer) {
    if view.is_null() {
        return;
    }
    let internal = unsafe { (*view).internal };
    if !internal.is_null() {
        unsafe {
            (*view).internal = ptr::null_mut();
            export_internal_release(
                internal.cast::<ExportInternal>(),
                hooks_or_stubs().buffer_release,
            );
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_Release(view: *mut Py_buffer) {
    if view.is_null() {
        return;
    }
    unsafe {
        let obj = (*view).obj;
        if obj.is_null() {
            // CPython Objects/abstract.c: `if (obj == NULL) return;` — a view
            // with no exporter has nothing to release (FillInfo(obj=NULL) and
            // the embedded-memoryview copies both land here; their descriptor
            // storage lives inside `*view` / the memoryview object itself).
            // Molt additionally resets the struct so a released view reads as
            // empty rather than stale.
            reset_pybuffer(view);
            return;
        }
        // Discriminate the release path by the NATURE OF THE EXPORTER OBJECT —
        // CPython's model (dispatch on `Py_TYPE(obj)->tp_as_buffer`) — never a
        // registry, and never a dereference of `view.internal` (for a foreign
        // exporter it is a private cookie that may be unmapped memory or a
        // small integer). `obj` is always dereferenceable here: the view holds
        // a strong reference from export until the DECREF below, which also
        // pins the bridge identity entry, so this classification cannot drift
        // from the one `PyObject_GetBuffer` made at export time.
        let is_molt_native = GLOBAL_BRIDGE.molt_handle_for_pyobj(obj).is_some();
        if is_molt_native {
            // Only managed acquisition publishes this internal layout.
            release_managed_export(view);
        } else if let Some(releasebuffer) = type_bf_releasebuffer((*obj).ob_type) {
            // View filled by a C-extension bf_getbuffer: CPython calls
            // `pb->bf_releasebuffer(obj, view)` when present, BEFORE the obj
            // DECREF — skipping it imbalances the exporter's refcount/resources.
            releasebuffer(obj, view);
        }
        (*view).internal = ptr::null_mut();
        // bf_releasebuffer observes the original descriptor. Retire it before
        // the last exporter reference can invoke a reentrant finalizer.
        (*view).obj = ptr::null_mut();
        reset_pybuffer(view);
        crate::api::refcount::Py_DECREF(obj);
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
    // Same classifier as `PyObject_GetBuffer`/`PyBuffer_Release`: a
    // raw-registered C object is FOREIGN (its synthetic identity bits are not
    // a `MoltObject`, so `classify_heap` on them would be garbage) — it gets
    // the honest slot test.
    let Some(resolved) = crate::bridge::resolve_pyobject(obj) else {
        return 0;
    };
    match resolved {
        crate::bridge::ResolvedPyObject::Foreign => {
            // Foreign object: honest slot test.
            (unsafe { type_bf_getbuffer((*obj).ob_type) }).is_some() as c_int
        }
        crate::bridge::ResolvedPyObject::ManagedMolt(bits) => {
            // Runtime metadata owns managed exporter eligibility. This is a
            // slot/support query, never an acquisition probe or callback.
            unsafe { (hooks_or_stubs().buffer_supports)(bits.bits()) }
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
    match crate::platform::c_char_to_u8(order) {
        b'C' | b'c' => unsafe { pybuffer_is_c_contiguous(view) as c_int },
        b'F' | b'f' => unsafe { pybuffer_is_f_contiguous(view) as c_int },
        _ => unsafe { (pybuffer_is_c_contiguous(view) || pybuffer_is_f_contiguous(view)) as c_int },
    }
}

/// The static format string FillInfo publishes for `PyBUF_FORMAT` — CPython
/// hands out the string literal `"B"` (Objects/abstract.c). C never writes
/// through `view.format`; the `*mut` in the ABI struct is historical.
static FILLINFO_FORMAT_B: [u8; 2] = *b"B\0";

/// CPython-exact `PyBuffer_FillInfo` (Objects/abstract.c, 3.12): fills the raw
/// 1-D byte view **allocation-free**. `format` points at a static `"B"`,
/// `shape` at `&view->len`, `strides` at `&view->itemsize`, and `internal` is
/// NULL.
///
/// # The self-referential contract (do not move a filled view)
/// Because `shape`/`strides` point INTO `*view`, a filled view must be used
/// and released **in place** — memcpy'ing it to a new home leaves those
/// pointers at the old address. This is CPython's own field model; CPython's
/// memoryview honors it by filling the master view directly inside the heap
/// object (`PyBuffer_FillInfo(&mbuf->master, …)`) and by re-pointing copied
/// views into the copy's own `ob_array`. Molt's memoryview constructors do the
/// same (fill `(*mv).view` in place / copy values into the object's embedded
/// storage) — see the runtime MemoryView and its bridge projection.
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
    if (flags & PyBUF_WRITABLE) == PyBUF_WRITABLE && readonly != 0 {
        unsafe { set_buffer_error(b"Object is not writable.\0") };
        return -1;
    }
    unsafe {
        (*view).obj = obj;
        if !obj.is_null() {
            crate::api::refcount::Py_INCREF(obj);
        }
        (*view).buf = buf;
        (*view).len = len;
        (*view).readonly = readonly;
        (*view).itemsize = 1;
        (*view).format = if (flags & PyBUF_FORMAT) == PyBUF_FORMAT {
            FILLINFO_FORMAT_B.as_ptr().cast::<c_char>().cast_mut()
        } else {
            ptr::null_mut()
        };
        (*view).ndim = 1;
        // Self-referential raw projections (CPython: `view->shape = &(view->len)`);
        // valid exactly as long as the view is not moved — see the doc comment.
        (*view).shape = if (flags & PyBUF_ND) == PyBUF_ND {
            &raw mut (*view).len
        } else {
            ptr::null_mut()
        };
        (*view).strides = if (flags & PyBUF_STRIDES) == PyBUF_STRIDES {
            &raw mut (*view).itemsize
        } else {
            ptr::null_mut()
        };
        (*view).suboffsets = ptr::null_mut();
        (*view).internal = ptr::null_mut();
    }
    0
}

#[cfg(test)]
mod export_internal_tests {
    use super::*;

    std::thread_local! {
        static FINALIZER_VIEW: std::cell::Cell<*mut Py_buffer> = const { std::cell::Cell::new(ptr::null_mut()) };
        static FINALIZER_SAW_RETIRED_VIEW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    unsafe extern "C" fn reenter_buffer_release_after_exporter_dealloc(_object: *mut PyObject) {
        FINALIZER_VIEW.with(|slot| {
            let view = slot.get();
            if !view.is_null() {
                let retired = unsafe { (*view).obj.is_null() && (*view).len == 0 };
                FINALIZER_SAW_RETIRED_VIEW.with(|observed| observed.set(retired));
                if retired {
                    unsafe { PyBuffer_Release(view) };
                }
            }
        });
    }

    #[test]
    fn buffer_release_retires_descriptor_before_exporter_finalizer_reentry() {
        let mut exporter_type: crate::abi_types::PyTypeObject = unsafe { std::mem::zeroed() };
        exporter_type.tp_dealloc = Some(reenter_buffer_release_after_exporter_dealloc);
        let mut exporter = PyObject {
            ob_refcnt: 1,
            ob_type: &mut exporter_type,
        };
        let mut view: Py_buffer = unsafe { std::mem::zeroed() };
        view.obj = &mut exporter;
        view.len = 7;
        FINALIZER_SAW_RETIRED_VIEW.with(|observed| observed.set(false));
        FINALIZER_VIEW.with(|slot| slot.set(&mut view));
        unsafe { PyBuffer_Release(&mut view) };
        FINALIZER_VIEW.with(|slot| slot.set(ptr::null_mut()));
        FINALIZER_SAW_RETIRED_VIEW.with(|observed| assert!(observed.get()));
        assert_eq!(exporter.ob_refcnt, 0);
        assert!(view.obj.is_null());
    }

    /// Right-sized-allocation gate: the per-export internal for a molt-native
    /// `PyObject_GetBuffer` is a 40 B header + two isizes/dim tail — NOT the former
    /// fixed 1112 B `BufferInternal` box. If a field is added to
    /// [`ExportInternal`] this updates deliberately.
    #[test]
    fn export_internal_is_right_sized() {
        assert_eq!(std::mem::size_of::<ExportInternal>(), 40);
        for ndim in [0usize, 1, 2, 3, 4, MOLT_BUFFER_MAX_NDIM] {
            let (layout, dims_offset) = export_internal_layout(ndim);
            assert_eq!(dims_offset, std::mem::size_of::<ExportInternal>());
            assert_eq!(
                layout.size(),
                std::mem::size_of::<ExportInternal>() + 2 * ndim * std::mem::size_of::<isize>(),
                "export internal must stay right-sized (header + 2*ndim isize)",
            );
        }
    }

    /// Alloc→read→release roundtrip: the tail carries shape then strides, the
    /// header carries format/ndim/owner/base, and release (under stub hooks) frees
    /// the allocation with the layout re-derived from the header's `ndim`.
    #[test]
    fn export_internal_roundtrip_preserves_dims_and_format() {
        let mut descriptor = MoltBufferView {
            ndim: 3,
            itemsize: 8,
            ..Default::default()
        };
        descriptor.shape[..3].copy_from_slice(&[4, 5, 6]);
        descriptor.strides[..3].copy_from_slice(&[240, 48, 8]);
        descriptor.format[0] = b'd';
        descriptor.format[1] = 0;
        unsafe {
            let internal = export_internal_new(&descriptor);
            assert!(!internal.is_null());
            let dims = export_internal_dims(internal, 3);
            assert_eq!([*dims.add(0), *dims.add(1), *dims.add(2)], [4, 5, 6]);
            assert_eq!([*dims.add(3), *dims.add(4), *dims.add(5)], [240, 48, 8]);
            let format = (&raw const (*internal).format).cast::<u8>();
            assert_eq!(*format, b'd');
            assert_eq!((&raw const (*internal).ndim).read(), 3);
            export_internal_release(internal, crate::hooks::STUB_HOOKS.buffer_release);
        }
    }

    std::thread_local! {
        static RELEASE_PAYLOADS: std::cell::RefCell<Vec<(u64, u64)>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    unsafe extern "C" fn record_release_payload(descriptor: *mut MoltBufferView) -> c_int {
        let owner = unsafe { (&raw const (*descriptor).owner).read() };
        let base = unsafe { (&raw const (*descriptor).base).read() };
        RELEASE_PAYLOADS.with(|payloads| payloads.borrow_mut().push((owner, base)));
        0
    }

    #[test]
    fn buffer_export_contract_compact_release_preserves_owner_and_base() {
        RELEASE_PAYLOADS.with(|payloads| payloads.borrow_mut().clear());
        let cases = [(7, 11), (7, 7), (7, 0), (0, 0)];
        for (owner, base) in cases {
            let descriptor = MoltBufferView {
                owner,
                base,
                ndim: 2,
                ..Default::default()
            };
            unsafe {
                let internal = export_internal_new(&descriptor);
                assert!(!internal.is_null());
                // Exercise the exact release-to-hook payload, not just field
                // storage. Equality is preserved for runtime deduplication.
                export_internal_release(internal, record_release_payload);
            }
        }
        RELEASE_PAYLOADS.with(|payloads| {
            assert_eq!(payloads.borrow().as_slice(), &cases);
            payloads.borrow_mut().clear();
        });
    }
}
