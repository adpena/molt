//! Buffer lifetime and resize admission. Geometry never owns storage: every
//! published export owns exactly one exporter reference and one counted pin.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::PyToken;

#[derive(Default)]
pub(crate) struct BufferExports(AtomicUsize);

impl BufferExports {
    pub(crate) const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    pub(crate) fn is_exported(&self) -> bool {
        self.count() != 0
    }

    pub(crate) fn count(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn acquire(&self) -> Result<(), ()> {
        self.0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map(|_| ())
            .map_err(|_| ())
    }

    pub(crate) fn release(&self) {
        if self
            .0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .is_err()
        {
            eprintln!("molt fatal: unbalanced buffer export release");
            std::process::abort();
        }
    }
}

/// Storage shortcuts are exact-class facts. A subclass may replace its buffer
/// slot and must enter the public protocol even when its layout is byte storage.
pub(crate) fn is_exact_builtin_storage(py: &PyToken<'_>, source: u64) -> bool {
    let class = crate::type_of_bits(py, source);
    let builtins = crate::builtin_classes(py);
    class == builtins.bytes || class == builtins.bytearray || class == builtins.memoryview
}

/// Eligibility invokes no exporter. Exact builtin storage needs no projection;
/// all other objects prepare their semantic class before the pure slot query.
/// An unavailable slot is a successful false result. Preparation failure keeps
/// its selected exception and must never select a consumer's fallback protocol.
pub(crate) fn supports_buffer(py: &PyToken<'_>, source: u64) -> Result<bool, BufferAccessError> {
    if crate::obj_from_bits(source).as_ptr().is_none() {
        return Ok(false);
    }
    if is_exact_builtin_storage(py, source) {
        return Ok(true);
    }
    unsafe {
        let object = molt_runtime_capi::project_buffer_object(source)
            .map_err(|_| BufferAccessError::Preparation)?;
        Ok(molt_cpython_abi::api::buffer::PyObject_CheckBuffer(object.as_ptr()) != 0)
    }
}

/// Acquire a counted, strong owner for a new independent export. Array leases
/// contain only Rust storage; ordinary objects remain real traced heap edges.
pub(crate) fn acquire_owner(py: &PyToken<'_>, source: u64) -> Result<u64, ()> {
    if source == 0 || crate::obj_from_bits(source).is_none() {
        return Ok(0);
    }
    unsafe {
        if let Some(ptr) = crate::obj_from_bits(source).as_ptr() {
            match crate::object_type_id(ptr) {
                crate::TYPE_ID_FOREIGN => {
                    let view = super::ops_memoryview::molt_memoryview_new(source);
                    if crate::exception_pending(py) {
                        return Err(());
                    }
                    let owner = acquire_owner(py, view);
                    crate::dec_ref_bits(py, view);
                    return owner;
                }
                crate::TYPE_ID_BYTEARRAY => {
                    let vec = super::layout::bytearray_vec_ptr(ptr);
                    let acquired = {
                        let _lock = super::backing::tracked_vec_mutation_lock(vec);
                        super::backing::tracked_vec_buffer_exports(vec).acquire()
                    };
                    if acquired.is_err() {
                        return export_overflow(py);
                    }
                }
                crate::TYPE_ID_MEMORYVIEW => {
                    if !super::memoryview::require_exportable(py, ptr) {
                        return Err(());
                    }
                    if (*super::memoryview_ptr(ptr)).exports.acquire().is_err() {
                        return export_overflow(py);
                    }
                }
                crate::TYPE_ID_OBJECT | crate::TYPE_ID_NATIVE_HANDLE => {
                    match crate::builtins::array_mod::array_buffer_owner_bits(py, source) {
                        Ok(Some(owner)) => return Ok(owner),
                        Ok(None) => {}
                        Err(()) => {
                            if !crate::exception_pending(py) {
                                let _ = crate::raise_exception::<u64>(
                                    py,
                                    "MemoryError",
                                    "cannot allocate buffer owner",
                                );
                            }
                            return Err(());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    crate::inc_ref_bits(py, source);
    Ok(source)
}

fn export_overflow(py: &PyToken<'_>) -> Result<u64, ()> {
    let _ = crate::raise_exception::<u64>(py, "OverflowError", "too many exported buffers");
    Err(())
}

/// Scoped consumer export: use across callback/GIL-release/wait boundaries.
/// The borrowed token prevents transferring this lease to an unowned thread.
pub(crate) struct ScopedBufferExport<'a, 'py> {
    py: &'a PyToken<'py>,
    owner: u64,
}

impl<'a, 'py> ScopedBufferExport<'a, 'py> {
    pub(crate) fn new(py: &'a PyToken<'py>, source: u64) -> Result<Self, ()> {
        acquire_owner(py, source).map(|owner| Self { py, owner })
    }

    pub(crate) fn into_owner(mut self) -> u64 {
        std::mem::replace(&mut self.owner, 0)
    }

    pub(crate) fn owner_bits(&self) -> u64 {
        self.owner
    }
}

impl Drop for ScopedBufferExport<'_, '_> {
    fn drop(&mut self) {
        release_owner(self.py, self.owner);
    }
}

/// Permission of the span borrowed by a raw IO callback. Readinto writes into
/// its caller's admitted storage; write only reads the supplied bytes.
#[derive(Clone, Copy)]
pub(crate) enum CallbackMemoryViewAccess {
    ReadOnly,
    Writable,
}

/// Raw IO permits the caller to release or mutate its storage on return.
/// This guard expires every Python descendant before the caller's export drops.
/// A native Py_buffer issued during the callback is likewise a scoped borrow:
/// later release is safe, but dereferencing it after return violates that API.
pub(crate) struct CallbackMemoryView<'scope> {
    root: molt_runtime_core::OwnedRuntimeValue<'scope>,
}

impl<'scope> CallbackMemoryView<'scope> {
    /// Borrow an admitted span through its actual operation owner. The owner
    /// and token lifetimes bound the scope guard; no storage is retained by views.
    ///
    /// # Safety
    /// `data..data+len` must remain live until this guard drops. Writable access
    /// requires exclusive write admission for the whole span. No overlapping
    /// Rust references may survive callback entry, and the storage cannot move.
    /// Internal IO callers own the moved Vec and serialize raw operations.
    pub(crate) unsafe fn borrow<T: ?Sized>(
        py: &'scope PyToken<'_>,
        _owner: &'scope T,
        data: *mut u8,
        len: usize,
        access: CallbackMemoryViewAccess,
    ) -> Result<Self, u64> {
        let format = crate::alloc_string(py, b"B");
        if format.is_null() {
            return Err(crate::MoltObject::none().bits());
        }
        let format_bits = crate::MoltObject::from_ptr(format).bits();
        let _format = crate::PtrDropGuard::new(format);
        let Some(mut storage) = super::memoryview::TypedStridedStorage::one_dim(
            data,
            matches!(access, CallbackMemoryViewAccess::ReadOnly),
            len,
            1,
            1,
            0,
            0,
            format_bits,
        ) else {
            return Err(crate::raise_exception(
                py,
                "BufferError",
                "invalid IO buffer geometry",
            ));
        };
        storage.kind = super::MemoryViewKind::CallbackRoot;
        let pointer = crate::alloc_memoryview_from_storage(py, storage);
        if pointer.is_null() {
            return Err(crate::MoltObject::none().bits());
        }
        Ok(Self {
            root: unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    crate::MoltObject::from_ptr(pointer).bits(),
                )
            },
        })
    }

    pub(crate) fn bits(&self) -> u64 {
        self.root.bits()
    }
}

impl Drop for CallbackMemoryView<'_> {
    fn drop(&mut self) {
        unsafe {
            let pointer = crate::obj_from_bits(self.root.bits())
                .as_ptr()
                .expect("callback view");
            // Explicit release is independent: children remain usable until
            // this scope ends even when the root itself was released earlier.
            let view = &mut *super::memoryview_ptr(pointer);
            view.kind = view.kind.expire();
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.invalidate_memoryview_view(self.root.bits());
        }
    }
}

/// Consumer admission is separate from error wording: a pending acquisition
/// exception (including a released view) must never become a generic TypeError.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BufferAccessError {
    /// Object projection or lease preparation failed before exporter dispatch.
    Preparation,
    Pending,
    Invalid,
}

impl BufferAccessError {
    pub(crate) fn raise(self, py: &PyToken<'_>, invalid_message: &str) -> u64 {
        match self {
            Self::Preparation | Self::Pending => crate::MoltObject::none().bits(),
            Self::Invalid => crate::raise_exception(py, "TypeError", invalid_message),
        }
    }
}

/// One owned buffer descriptor for readable, writable and strided consumers.
/// Acquisition may allocate or invoke exporter callbacks. No pointer observed
/// before acquisition may be retained across that boundary.
pub(crate) struct ScopedBuffer<'a, 'py> {
    py: &'a PyToken<'py>,
    export: super::memoryview::MoltBufferView,
    lease: Option<super::foreign::RuntimeBufferLease>,
}

impl<'a, 'py> ScopedBuffer<'a, 'py> {
    pub(crate) fn new(py: &'a PyToken<'py>, source: u64) -> Result<Self, BufferAccessError> {
        Self::with_flags(py, source, molt_cpython_abi::abi_types::PyBUF_FULL_RO)
    }

    pub(crate) fn with_flags(
        py: &'a PyToken<'py>,
        source: u64,
        flags: i32,
    ) -> Result<Self, BufferAccessError> {
        use molt_cpython_abi::api::buffer;
        let physical = is_exact_builtin_storage(py, source);
        if !physical {
            unsafe {
                let lease = match molt_runtime_capi::acquire_buffer(source, flags) {
                    Ok(lease) => lease,
                    Err(molt_cpython_abi::api::memory::BufferAcquireError::Preparation) => {
                        return Err(BufferAccessError::Preparation);
                    }
                    Err(molt_cpython_abi::api::memory::BufferAcquireError::Exporter) => {
                        return Err(BufferAccessError::Pending);
                    }
                };
                let descriptor = buffer::descriptor_from_pybuffer(lease.descriptor())
                    .map_err(|_| BufferAccessError::Invalid)?;
                let mut export = crate::cpython_abi_hooks::runtime_buffer_view_from_abi(descriptor);
                let rank = export.ndim as usize;
                // Py_buffer.len is logical nbytes, not the addressed span of
                // a strided export. The native lease guarantees that declared
                // storage; reuse memoryview's checked bounds for this projection.
                let span = super::memoryview::memoryview_strided_bounds(
                    &export.shape[..rank],
                    &export.strides[..rank],
                    export.itemsize as usize,
                )
                .ok_or(BufferAccessError::Invalid)?;
                export.backing_capacity = span.span_len as u64;
                return Ok(Self {
                    py,
                    export,
                    lease: Some(lease),
                });
            }
        }
        let mut export = super::memoryview::MoltBufferView::default();
        if unsafe { crate::c_api::molt_buffer_acquire(source, &mut export) } != 0 {
            return Err(if crate::exception_pending(py) {
                BufferAccessError::Pending
            } else {
                BufferAccessError::Invalid
            });
        }
        let result = Self {
            py,
            export,
            lease: None,
        };
        if flags & molt_cpython_abi::abi_types::PyBUF_WRITABLE != 0 && result.export.readonly != 0 {
            return Err(BufferAccessError::Invalid);
        }
        let abi = crate::cpython_abi_hooks::abi_buffer_view_from_runtime(result.export);
        if let Some(message) = buffer::descriptor_request_error(&abi, flags) {
            let message = std::str::from_utf8(&message[..message.len() - 1]).unwrap();
            crate::raise_exception::<()>(py, "BufferError", message);
            return Err(BufferAccessError::Pending);
        }
        Ok(result)
    }

    /// Contiguous byte arguments retain only the storage they need. Ordinary
    /// bytes avoid ABI projection, allocation and an extra immutable-owner pin.
    /// Bytearray arguments retain a counted pin across subsequent callbacks.
    pub(crate) fn simple(py: &'a PyToken<'py>, source: u64) -> Result<Self, BufferAccessError> {
        if is_exact_builtin_storage(py, source)
            && let Some(pointer) = crate::obj_from_bits(source).as_ptr()
        {
            unsafe {
                let kind = crate::object_type_id(pointer);
                if kind == crate::TYPE_ID_BYTES || kind == crate::TYPE_ID_BYTEARRAY {
                    let owner = if kind == crate::TYPE_ID_BYTEARRAY {
                        acquire_owner(py, source).map_err(|_| BufferAccessError::Pending)?
                    } else {
                        0
                    };
                    let len = crate::bytes_len(pointer);
                    let mut export = super::memoryview::MoltBufferView {
                        data: crate::bytes_data(pointer).cast_mut(),
                        len: len as u64,
                        backing_capacity: len as u64,
                        readonly: u32::from(kind == crate::TYPE_ID_BYTES),
                        owner,
                        ..Default::default()
                    };
                    export.shape[0] = len as isize;
                    export.strides[0] = 1;
                    return Ok(Self {
                        py,
                        export,
                        lease: None,
                    });
                }
            }
        }
        let buffer = Self::with_flags(py, source, molt_cpython_abi::abi_types::PyBUF_SIMPLE)?;
        buffer.contiguous_len()?;
        Ok(buffer)
    }

    /// CPython's PyBytes_Check shortcuts deliberately read subclass storage
    /// without dispatching __buffer__ (tailmatch and bytes.translate).
    pub(crate) unsafe fn physical_bytes(py: &'a PyToken<'py>, pointer: *mut u8) -> Self {
        debug_assert_eq!(
            unsafe { crate::object_type_id(pointer) },
            crate::TYPE_ID_BYTES
        );
        let len = unsafe { crate::bytes_len(pointer) };
        let mut export = super::memoryview::MoltBufferView {
            data: unsafe { crate::bytes_data(pointer).cast_mut() },
            len: len as u64,
            backing_capacity: len as u64,
            readonly: 1,
            ..Default::default()
        };
        export.shape[0] = len as isize;
        export.strides[0] = 1;
        Self {
            py,
            export,
            lease: None,
        }
    }

    /// Callers of simple have already established a contiguous byte span.
    pub(crate) fn as_slice(&self) -> &[u8] {
        if self.export.len == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.export.data, self.export.len as usize) }
    }

    pub(crate) fn view(&self) -> &super::memoryview::MoltBufferView {
        &self.export
    }

    /// Preserve the actual Py_buffer.obj identity used by immutable partition.
    /// Exact physical routes borrow the original source; native leases may own
    /// a different exporter (notably the Python __buffer__ wrapper).
    pub(crate) fn owned_exporter(&self, source: u64) -> Option<u64> {
        if let Some(lease) = &self.lease {
            let owner = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(lease.owner())
            };
            if owner.is_none() {
                crate::cpython_abi_hooks::propagate_native_failure(self.py, "buffer owner");
            }
            owner
        } else {
            crate::inc_ref_bits(self.py, source);
            Some(source)
        }
    }

    /// Semantic format belongs to the retained export, not the fixed-size
    /// geometry transport. This borrow cannot outlive its lease or physical pin.
    pub(crate) fn format(&self) -> &[u8] {
        unsafe {
            if let Some(lease) = &self.lease {
                let format = (*lease.descriptor()).format;
                return if format.is_null() {
                    b"B"
                } else {
                    std::ffi::CStr::from_ptr(format).to_bytes()
                };
            }
            if let Some(owner) = crate::obj_from_bits(self.export.owner).as_ptr()
                && crate::object_type_id(owner) == crate::TYPE_ID_MEMORYVIEW
            {
                let format = crate::obj_from_bits(crate::memoryview_format_bits(owner))
                    .as_ptr()
                    .expect("pinned memoryview owns its format");
                return std::slice::from_raw_parts(
                    crate::string_bytes(format),
                    crate::string_len(format),
                );
            }
        }
        // The other physical routes are exact bytes and bytearray storage.
        b"B"
    }

    pub(crate) fn contiguous_len(&self) -> Result<usize, BufferAccessError> {
        let export = &self.export;
        let ndim = export.ndim as usize;
        let itemsize = usize::try_from(export.itemsize).map_err(|_| BufferAccessError::Invalid)?;
        if ndim > super::memoryview::MOLT_BUFFER_MAX_NDIM
            || !super::memoryview::memoryview_is_c_contiguous(
                &export.shape[..ndim],
                &export.strides[..ndim],
                itemsize,
            )
        {
            return Err(BufferAccessError::Invalid);
        }
        let len = usize::try_from(export.len)
            .ok()
            .filter(|&len| len <= isize::MAX as usize)
            .ok_or_else(|| {
                let _ = crate::raise_exception::<u64>(
                    self.py,
                    "OverflowError",
                    "buffer length exceeds the active address space",
                );
                BufferAccessError::Pending
            })?;
        if len != 0 && export.data.is_null() {
            let _ = crate::raise_exception::<u64>(
                self.py,
                "BufferError",
                "buffer data pointer is null",
            );
            return Err(BufferAccessError::Pending);
        }
        Ok(len)
    }
}

impl Drop for ScopedBuffer<'_, '_> {
    fn drop(&mut self) {
        if self.lease.is_none() {
            unsafe { crate::c_api::molt_buffer_release(&mut self.export) };
        }
    }
}

/// Writable admission wraps the same counted owner and contiguous-span
/// validation used by readable consumers. No slices may span Python reentry.
pub(crate) struct ScopedWritableBuffer<'a, 'py> {
    buffer: ScopedBuffer<'a, 'py>,
    len: usize,
}

impl<'a, 'py> ScopedWritableBuffer<'a, 'py> {
    pub(crate) fn new(py: &'a PyToken<'py>, source: u64) -> Result<Self, BufferAccessError> {
        let buffer =
            ScopedBuffer::with_flags(py, source, molt_cpython_abi::abi_types::PyBUF_WRITABLE)?;
        if buffer.view().readonly != 0 {
            return Err(BufferAccessError::Invalid);
        }
        let len = buffer.contiguous_len()?;
        Ok(Self { buffer, len })
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Valid only while this guard lives, without overlapping Rust references.
    pub(crate) fn as_mut_ptr(&self) -> *mut u8 {
        if self.len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            self.buffer.view().data
        }
    }
}

/// Drop only the counted pin. The caller still owns the heap edge, which must
/// be detached through its normal GC/refcount sink after this transition.
pub(crate) unsafe fn detach_owner(owner: u64) {
    unsafe {
        let Some(ptr) = crate::obj_from_bits(owner).as_ptr() else {
            return;
        };
        match crate::object_type_id(ptr) {
            crate::TYPE_ID_BYTEARRAY => {
                let vec = super::layout::bytearray_vec_ptr(ptr);
                let _lock = super::backing::tracked_vec_mutation_lock(vec);
                super::backing::tracked_vec_buffer_exports(vec).release();
            }
            crate::TYPE_ID_MEMORYVIEW => (*super::memoryview_ptr(ptr)).exports.release(),
            _ => {} // Array lease Drop owns its one counter decrement.
        }
    }
}

pub(crate) fn release_owner(py: &PyToken<'_>, owner: u64) {
    if owner != 0 {
        unsafe { detach_owner(owner) };
        crate::dec_ref_bits(py, owner);
    }
}

/// Invalidate a memoryview before releasing any reference (which may invoke
/// destruction). Used by explicit release and by the heap lifecycle sink.
pub(crate) unsafe fn detach_memoryview_owner(
    ptr: *mut u8,
) -> (u64, u64, Option<crate::object::foreign::RuntimeBufferLease>) {
    unsafe {
        let view = &mut *super::memoryview_ptr(ptr);
        let owner = std::mem::replace(&mut view.owner_bits, 0);
        let base = std::mem::replace(&mut view.base_bits, 0);
        view.data = std::ptr::null_mut();
        view.released = 1;
        let native = view.native_lease.take();
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .invalidate_memoryview_view(crate::MoltObject::from_ptr(ptr).bits());
        if owner != 0 && view.kind.scope() != super::MemoryViewScope::Derived {
            detach_owner(owner);
        }
        (owner, if base != owner { base } else { 0 }, native)
    }
}

/// Every bytearray backing owns one initialized NUL outside its logical Vec
/// length. Establish this before publication: acquiring an export must never
/// allocate, because an existing descriptor may already observe the address.
pub(crate) fn bytearray_backing_from_slice(bytes: &[u8], capacity: usize) -> Option<*mut Vec<u8>> {
    let capacity = capacity.max(bytes.len()).checked_add(1)?;
    let vec = super::backing::tracked_vec_box_from_slice(bytes, capacity)?;
    unsafe { (*vec).as_mut_ptr().add((*vec).len()).write(0) };
    Some(vec)
}

pub(crate) fn bytearray_backing_zeroed(len: usize) -> Option<*mut Vec<u8>> {
    let capacity = len.checked_add(1)?;
    let vec = super::backing::tracked_vec_box_with_capacity::<u8>(capacity)?;
    unsafe {
        (*vec).resize(len, 0);
        (*vec).as_mut_ptr().add(len).write(0);
    }
    Some(vec)
}

/// Observe the live backing without allocating, copying or taking a new lease.
/// The caller must own the bytearray and exclude concurrent Python mutation;
/// any longer-lived buffer consumer must acquire the ordinary counted export.
pub(crate) unsafe fn bytearray_data(ptr: *mut u8) -> (*mut u8, usize) {
    unsafe {
        let vec = super::layout::bytearray_vec_ptr(ptr);
        let _lock = super::backing::tracked_vec_mutation_lock(vec);
        let len = (*vec).len();
        assert!(
            (*vec).capacity() > len,
            "bytearray backing lost its NUL capacity"
        );
        ((*vec).as_mut_ptr(), len)
    }
}

/// Shared Python/C-ABI resize, after the caller has finished size coercion.
pub(crate) fn bytearray_resize(py: &PyToken<'_>, bits: u64, len: usize) -> bool {
    let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
        let _ = crate::raise_exception::<u64>(py, "TypeError", "bytearray object expected");
        return false;
    };
    unsafe {
        if crate::object_type_id(ptr) != crate::TYPE_ID_BYTEARRAY {
            let _ = crate::raise_exception::<u64>(py, "TypeError", "bytearray object expected");
            return false;
        }
        bytearray_mutate(py, ptr, len, |vec| vec.resize(len, 0)).is_some()
    }
}

/// The sole bytearray length/capacity mutation admission. Coercions, iteration
/// and all other callbacks must finish before entering. The closure must not
/// allocate Molt objects, invoke Python, or change length beyond `new_len`.
/// Same-size writes retain the exported address; every actual resize is denied.
pub(crate) unsafe fn bytearray_mutate<T>(
    py: &PyToken<'_>,
    ptr: *mut u8,
    new_len: usize,
    mutate: impl FnOnce(&mut Vec<u8>) -> T,
) -> Option<T> {
    match unsafe { bytearray_try_mutate(ptr, new_len, mutate) } {
        Ok(out) => Some(out),
        Err(BufferMutationError::Exported) => crate::raise_exception(
            py,
            "BufferError",
            "Existing exports of data: object cannot be re-sized",
        ),
        Err(BufferMutationError::Allocation) => {
            crate::raise_exception(py, "MemoryError", "bytearray allocation failed")
        }
    }
}

pub(crate) enum BufferMutationError {
    Exported,
    Allocation,
}

/// Non-raising form for finalizer flushes, which must not replace an active
/// exception. The mutation and resource accounting remain the same authority.
pub(crate) unsafe fn bytearray_try_mutate<T>(
    ptr: *mut u8,
    new_len: usize,
    mutate: impl FnOnce(&mut Vec<u8>) -> T,
) -> Result<T, BufferMutationError> {
    unsafe {
        let vec = super::layout::bytearray_vec_ptr(ptr);
        let _lock = super::backing::tracked_vec_mutation_lock(vec);
        if new_len != (*vec).len() && super::backing::tracked_vec_buffer_exports(vec).is_exported()
        {
            return Err(BufferMutationError::Exported);
        }
        let capacity = new_len
            .checked_add(1)
            .ok_or(BufferMutationError::Allocation)?;
        if !super::backing::tracked_vec_reserve_for_len(vec, capacity) {
            return Err(BufferMutationError::Allocation);
        }
        let out = mutate(&mut *vec);
        assert_eq!(
            (*vec).len(),
            new_len,
            "bytearray mutation violated admitted length"
        );
        // Same-size writes retain exported addresses and never need capacity.
        (*vec).as_mut_ptr().add(new_len).write(0);
        super::backing::tracked_vec_bump_mutation_epoch(vec);
        Ok(out)
    }
}

pub(crate) unsafe fn bytearray_is_exported(ptr: *mut u8) -> bool {
    unsafe {
        super::backing::tracked_vec_buffer_exports(super::layout::bytearray_vec_ptr(ptr))
            .is_exported()
    }
}

/// BytesIO forbids write/truncate/close while exporting even if the requested
/// operation would keep the byte length unchanged. It uses the same live pins.
pub(crate) unsafe fn bytearray_require_unexported(
    py: &PyToken<'_>,
    ptr: *mut u8,
) -> Result<(), u64> {
    if unsafe { bytearray_is_exported(ptr) } {
        Err(crate::raise_exception(
            py,
            "BufferError",
            "Existing exports of data: object cannot be re-sized",
        ))
    } else {
        Ok(())
    }
}

/// Delete the monotonic, unique positions produced by slice normalization.
/// Both extended deletion spellings use one stable, linear compaction, rather
/// than moving the same suffix repeatedly for every selected element.
pub(crate) unsafe fn bytearray_remove_indices(
    py: &PyToken<'_>,
    ptr: *mut u8,
    indices: &[usize],
) -> bool {
    if indices.is_empty() {
        return true;
    }
    unsafe {
        let new_len = super::layout::bytearray_len(ptr) - indices.len();
        bytearray_mutate(py, ptr, new_len, |elems| {
            let ascending = indices.first() <= indices.last();
            let mut selected = 0;
            let mut position = 0;
            elems.retain(|_| {
                let remove = selected < indices.len()
                    && indices[if ascending {
                        selected
                    } else {
                        indices.len() - 1 - selected
                    }] == position;
                position += 1;
                selected += usize::from(remove);
                !remove
            });
        })
        .is_some()
    }
}

#[cfg(test)]
mod compaction_tests {
    use super::*;

    #[test]
    fn bytearray_backing_nul_is_initialized_before_export_and_after_mutation() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                for ptr in [
                    crate::alloc_bytearray(py, b""),
                    crate::object::builders::alloc_bytearray_with_capacity(py, b"ab", 2),
                    crate::object::builders::alloc_bytearray_with_len(py, 3),
                ] {
                    assert!(!ptr.is_null());
                    let bits = crate::MoltObject::from_ptr(ptr).bits();
                    let (data, len) = bytearray_data(ptr);
                    assert_eq!(*data.add(len), 0);
                    let owner = ScopedBufferExport::new(py, bits).unwrap();
                    assert_eq!(bytearray_data(ptr), (data, len));
                    assert!(bytearray_mutate(py, ptr, len, |v| v.fill(b'x')).is_some());
                    assert_eq!(bytearray_data(ptr), (data, len));
                    assert_eq!(*data.add(len), 0);
                    drop(owner);
                    for size in [len + 8, 1, 0, 32, 0] {
                        assert!(bytearray_resize(py, bits, size));
                        let (data, length) = bytearray_data(ptr);
                        assert_eq!(length, size);
                        assert_eq!(*data.add(size), 0);
                    }
                    crate::dec_ref_bits(py, bits);
                }
            }
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn buffer_export_contract_extended_delete_compacts_in_both_directions() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (indices, expected) in [
                (vec![], b"abcdefgh".as_slice()),
                (vec![0, 2, 4, 6], b"bdfh".as_slice()),
                (vec![6, 4, 2, 0], b"bdfh".as_slice()),
                (vec![7, 6, 5, 4, 3, 2, 1, 0], b"".as_slice()),
            ] {
                let ptr = crate::alloc_bytearray(py, b"abcdefgh");
                assert!(!ptr.is_null());
                assert!(unsafe { bytearray_remove_indices(py, ptr, &indices) });
                assert_eq!(
                    unsafe { super::super::layout::bytearray_vec_ref(ptr) },
                    expected
                );
                let (data, len) = unsafe { bytearray_data(ptr) };
                assert_eq!(unsafe { *data.add(len) }, 0);
                crate::dec_ref_bits(py, crate::MoltObject::from_ptr(ptr).bits());
                assert!(!crate::exception_pending(py));
            }
        });
    }
}
