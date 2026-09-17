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

/// Acquire a counted, strong owner for a new independent export. Array leases
/// contain only Rust storage; ordinary objects remain real traced heap edges.
pub(crate) fn acquire_owner(py: &PyToken<'_>, source: u64) -> Result<u64, ()> {
    if source == 0 || crate::obj_from_bits(source).is_none() {
        return Ok(0);
    }
    unsafe {
        if let Some(ptr) = crate::obj_from_bits(source).as_ptr() {
            match crate::object_type_id(ptr) {
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
                    if super::memoryview_released(ptr) {
                        let _ = crate::raise_released_memoryview::<u64>(py);
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
pub(crate) enum WritableBufferError {
    Pending,
    Invalid,
}

impl WritableBufferError {
    pub(crate) fn raise(self, py: &PyToken<'_>, invalid_message: &str) -> u64 {
        match self {
            Self::Pending => crate::MoltObject::none().bits(),
            Self::Invalid => crate::raise_exception(py, "TypeError", invalid_message),
        }
    }
}

/// One writable simple-buffer consumer: a counted owner plus a validated,
/// C-contiguous byte span. Shape and format affect admission, never capacity.
/// Keep this guard alive across waits/reentry; only form Rust slices for the
/// immediate non-reentrant operation, not for the guard's entire lifetime.
pub(crate) struct ScopedWritableBuffer<'a, 'py> {
    _py: &'a PyToken<'py>,
    export: super::memoryview::MoltBufferView,
    len: usize,
}

impl<'a, 'py> ScopedWritableBuffer<'a, 'py> {
    pub(crate) fn new(py: &'a PyToken<'py>, source: u64) -> Result<Self, WritableBufferError> {
        let mut export = super::memoryview::MoltBufferView::default();
        if unsafe { crate::c_api::molt_buffer_acquire(source, &mut export) } != 0 {
            return Err(if crate::exception_pending(py) {
                WritableBufferError::Pending
            } else {
                WritableBufferError::Invalid
            });
        }
        // Establish RAII before validating so every rejection discharges exactly
        // the newly acquired pin, including the distinct descriptor base edge.
        let mut buffer = Self {
            _py: py,
            export,
            len: 0,
        };
        let ndim = export.ndim as usize;
        let itemsize =
            usize::try_from(export.itemsize).map_err(|_| WritableBufferError::Invalid)?;
        if export.readonly != 0
            || ndim > super::memoryview::MOLT_BUFFER_MAX_NDIM
            || !super::memoryview::memoryview_is_c_contiguous(
                &export.shape[..ndim],
                &export.strides[..ndim],
                itemsize,
            )
        {
            return Err(WritableBufferError::Invalid);
        }
        let len = usize::try_from(export.len)
            .ok()
            .filter(|&len| len <= isize::MAX as usize)
            .ok_or_else(|| {
                let _ = crate::raise_exception::<u64>(
                    py,
                    "OverflowError",
                    "buffer length exceeds the active address space",
                );
                WritableBufferError::Pending
            })?;
        if len != 0 && export.data.is_null() {
            let _ = crate::raise_exception::<u64>(py, "BufferError", "buffer data pointer is null");
            return Err(WritableBufferError::Pending);
        }
        buffer.len = len;
        Ok(buffer)
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The pointer remains valid only while this guard lives. Mutation through
    /// it must not overlap another Rust reference or span Python reentry.
    pub(crate) fn as_mut_ptr(&self) -> *mut u8 {
        if self.len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            self.export.data
        }
    }
}

impl Drop for ScopedWritableBuffer<'_, '_> {
    fn drop(&mut self) {
        unsafe { crate::c_api::molt_buffer_release(&mut self.export) };
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
pub(crate) unsafe fn detach_memoryview_owner(ptr: *mut u8) -> (u64, u64) {
    unsafe {
        let view = &mut *super::memoryview_ptr(ptr);
        let owner = std::mem::replace(&mut view.owner_bits, 0);
        let base = std::mem::replace(&mut view.base_bits, 0);
        view.data = std::ptr::null_mut();
        view.released = 1;
        if owner != 0 {
            detach_owner(owner);
        }
        (owner, if base != owner { base } else { 0 })
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
        if !super::backing::tracked_vec_reserve_for_len(vec, new_len) {
            return Err(BufferMutationError::Allocation);
        }
        let out = mutate(&mut *vec);
        assert_eq!(
            (*vec).len(),
            new_len,
            "bytearray mutation violated admitted length"
        );
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
                crate::dec_ref_bits(py, crate::MoltObject::from_ptr(ptr).bits());
                assert!(!crate::exception_pending(py));
            }
        });
    }
}
