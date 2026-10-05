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
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map(|_| ())
            .map_err(|_| ())
    }

    pub(crate) fn release(&self) {
        if self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .is_err()
        {
            eprintln!("molt fatal: unbalanced buffer export release");
            std::process::abort();
        }
    }
}

/// Side-effect-free buffer admission, shared by internal comparisons and C
/// PyObject_CheckBuffer. A released memoryview still owns the protocol; only
/// acquisition may reject its current state.
pub(crate) fn supports_buffer(py: &PyToken<'_>, source: u64) -> bool {
    let Some(ptr) = crate::obj_from_bits(source).as_ptr() else {
        return false;
    };
    match unsafe { crate::object_type_id(ptr) } {
        crate::TYPE_ID_BYTES | crate::TYPE_ID_BYTEARRAY | crate::TYPE_ID_MEMORYVIEW => true,
        crate::TYPE_ID_FOREIGN => unsafe {
            molt_cpython_abi::api::buffer::PyObject_CheckBuffer(
                std::ptr::with_exposed_provenance_mut(super::foreign::foreign_ptr_from_obj(ptr)),
            ) != 0
        },
        crate::TYPE_ID_OBJECT | crate::TYPE_ID_NATIVE_HANDLE => {
            crate::builtins::array_mod::array_supports_buffer(py, source)
        }
        _ => false,
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

/// Consumer admission is separate from error wording: a pending acquisition
/// exception (including a released view) must never become a generic TypeError.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BufferAccessError {
    Pending,
    Invalid,
}

impl BufferAccessError {
    pub(crate) fn raise(self, py: &PyToken<'_>, invalid_message: &str) -> u64 {
        match self {
            Self::Pending => crate::MoltObject::none().bits(),
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
}

impl<'a, 'py> ScopedBuffer<'a, 'py> {
    pub(crate) fn new(py: &'a PyToken<'py>, source: u64) -> Result<Self, BufferAccessError> {
        let mut export = super::memoryview::MoltBufferView::default();
        if unsafe { crate::c_api::molt_buffer_acquire(source, &mut export) } != 0 {
            return Err(if crate::exception_pending(py) {
                BufferAccessError::Pending
            } else {
                BufferAccessError::Invalid
            });
        }
        Ok(Self { py, export })
    }

    pub(crate) fn view(&self) -> &super::memoryview::MoltBufferView {
        &self.export
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
        unsafe { crate::c_api::molt_buffer_release(&mut self.export) };
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
        let buffer = ScopedBuffer::new(py, source)?;
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
) -> (
    u64,
    u64,
    Option<molt_cpython_abi::api::memory::MemoryViewLease>,
) {
    unsafe {
        let view = &mut *super::memoryview_ptr(ptr);
        let owner = std::mem::replace(&mut view.owner_bits, 0);
        let base = std::mem::replace(&mut view.base_bits, 0);
        view.data = std::ptr::null_mut();
        view.released = 1;
        let native = view.native_lease.take();
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .invalidate_memoryview_view(crate::MoltObject::from_ptr(ptr).bits());
        if owner != 0 {
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
