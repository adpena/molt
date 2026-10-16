//! Shared native-reference retirement for runtime and extracted consumers.
//! Storage, buffer descriptors, and release cookies remain owned by the ABI.

pub use molt_cpython_abi as cpython_abi;
pub use molt_cpython_abi::abi_types::{PyBUF_FULL_RO, PyBUF_SIMPLE, PyBUF_WRITABLE};
pub use molt_cpython_abi::api::memory::BufferAcquireError;

use crate::ffi::__molt_native_reference_complete;

/// Complete native retirement before returning to a runtime operation. Foreign
/// wrapper release, owned C temporaries, and operations that return a scalar
/// after retiring their native temporaries share this completion boundary.
#[inline]
pub fn retire_native_reference<T>(release: impl FnOnce() -> T) -> T {
    let result = release();
    // A valid native deallocator may move the active raised exception between
    // channels through PyErr_GetRaisedException/PyErr_SetRaisedException.
    // Complete that native reentry before returning to a runtime consumer.
    unsafe {
        __molt_native_reference_complete();
    }
    result
}

/// Ordinary C-reference retirement returning to a runtime consumer. Native
/// deallocators see the current error, then the existing completion authority
/// imports any legal C-channel restoration before runtime error checks resume.
pub enum NativeReferenceRetirement {}

unsafe impl molt_cpython_abi::api::refcount::ReferenceRetirement for NativeReferenceRetirement {
    #[inline]
    fn retire(release: impl FnOnce()) {
        retire_native_reference(release);
    }
}

pub type NativeObjectOwner =
    molt_cpython_abi::api::refcount::PyObjectOwner<NativeReferenceRetirement>;

pub type RuntimeBufferLease = molt_cpython_abi::api::memory::BufferLease<NativeReferenceRetirement>;

const _: () = {
    assert!(std::mem::size_of::<RuntimeBufferLease>() == std::mem::size_of::<usize>());
    assert!(std::mem::align_of::<RuntimeBufferLease>() == std::mem::align_of::<usize>());
    assert!(std::mem::size_of::<NativeObjectOwner>() == std::mem::size_of::<usize>());
    assert!(std::mem::align_of::<NativeObjectOwner>() == std::mem::align_of::<usize>());
};

/// Acquire the existing stable descriptor from a runtime handle. The caller
/// holds runtime execution custody; errors are transferred by the same native
/// retirement policy used by all owners, never cleared or replaced here.
pub unsafe fn acquire_buffer(
    bits: u64,
    flags: i32,
) -> Result<RuntimeBufferLease, BufferAcquireError> {
    unsafe {
        let object = project_buffer_object(bits)?;
        let result = acquire_buffer_from_object(object.as_ptr(), flags);
        drop(object);
        result
    }
}

/// Projection precedes exporter dispatch. Preserve every preparation failure,
/// including metadata and publication errors; Clinic may normalize only errors
/// from the subsequently invoked buffer protocol.
pub unsafe fn project_buffer_object(bits: u64) -> Result<NativeObjectOwner, BufferAcquireError> {
    unsafe {
        let object = NativeObjectOwner::from_owned(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits),
        );
        if object.as_ptr().is_null() {
            __molt_native_reference_complete();
            return Err(BufferAcquireError::Preparation);
        }
        Ok(object)
    }
}

/// Acquire from an already projected, live borrowed native reference. The
/// caller keeps that reference alive until this call completes; the returned
/// lease owns its exporter independently. All descriptor validation and native
/// error completion are shared with runtime-handle acquisition.
pub unsafe fn acquire_buffer_from_object(
    object: *mut molt_cpython_abi::abi_types::PyObject,
    flags: i32,
) -> Result<RuntimeBufferLease, BufferAcquireError> {
    unsafe {
        let result = RuntimeBufferLease::acquire(
            object,
            flags,
            molt_cpython_abi::api::buffer::PyObject_GetBuffer,
        );
        // Preparation and exporter failures retain their distinct typed result,
        // while the existing completion owner imports the native error.
        __molt_native_reference_complete();
        let lease = result?;
        let descriptor = &*lease.descriptor();
        if descriptor.len < 0 || (descriptor.len != 0 && descriptor.buf.is_null()) {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_BufferError).cast(),
                c"invalid buffer span".as_ptr(),
            );
            drop(lease);
            return Err(BufferAcquireError::Exporter);
        }
        Ok(lease)
    }
}

/// Borrow exact semantic bytes without exporting the value or allocating a
/// buffer master. Subclasses and mutable exporters require the owned protocol.
/// The canonical live class projection owns admission; physical bytes layout
/// is consulted only after that semantic check succeeds.
///
/// # Safety
/// `bits` must remain a live externally owned operand for the returned borrow.
/// The caller must retain uninterrupted execution custody and must not retire
/// its real owner or invoke a callback while using the slice. The token bounds
/// the borrow; it does not itself retain the operand.
pub unsafe fn exact_bytes<'py>(
    _py: &'py crate::PyToken,
    bits: u64,
) -> Result<Option<&'py [u8]>, BufferAcquireError> {
    unsafe {
        let class = molt_cpython_abi::bridge::GLOBAL_BRIDGE.runtime_class_view(bits);
        if class.is_null() {
            __molt_native_reference_complete();
            return Err(BufferAcquireError::Preparation);
        }
        if !std::ptr::eq(class, &raw mut molt_cpython_abi::abi_types::PyBytes_Type) {
            return Ok(None);
        }
        Ok(crate::rt_bytes_as_slice(bits))
    }
}

/// Read a PyBUF_SIMPLE span after all intervening Python conversions. The
/// caller must not hold the returned Rust reference across Python reentry.
pub unsafe fn buffer_bytes(lease: &RuntimeBufferLease) -> &[u8] {
    let descriptor = unsafe { &*lease.descriptor() };
    if descriptor.len == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(descriptor.buf.cast(), descriptor.len as usize) }
}

/// One scoped byte input, reusing the existing immutable-value owner or the
/// existing buffer lease. This owns no independent release protocol.
pub struct ByteInput<'py>(ByteInputStorage<'py>);

enum ByteInputStorage<'py> {
    Exact(crate::OwnedRuntimeValue<'py>),
    Export(RuntimeBufferLease),
}

impl<'py> ByteInput<'py> {
    /// Acquire a contiguous readonly byte input under live execution custody.
    /// Exact immutable bytes retain their runtime owner without a C projection
    /// or buffer master; all other objects use the public SIMPLE protocol.
    ///
    /// # Safety
    /// `bits` must be a live borrowed operand when this call starts.
    pub unsafe fn acquire(py: &'py crate::PyToken, bits: u64) -> Result<Self, BufferAcquireError> {
        unsafe {
            if exact_bytes(py, bits)?.is_some() {
                return Ok(Self(ByteInputStorage::Exact(
                    crate::OwnedRuntimeValue::retain(py, bits),
                )));
            }
            acquire_buffer(bits, PyBUF_SIMPLE).map(|lease| Self(ByteInputStorage::Export(lease)))
        }
    }

    /// Rank reported by the admitted provider; SIMPLE consumers such as the
    /// hash APIs may impose their own dimensionality rule.
    pub fn ndim(&self) -> i32 {
        match &self.0 {
            ByteInputStorage::Exact(_) => 1,
            ByteInputStorage::Export(lease) => unsafe { (*lease.descriptor()).ndim },
        }
    }

    /// Geometry check for consumers whose Clinic buffer role requires C order.
    /// This reuses the ABI descriptor authority and invokes no exporter callback.
    pub fn is_c_contiguous(&self) -> bool {
        match &self.0 {
            ByteInputStorage::Exact(_) => true,
            ByteInputStorage::Export(lease) => unsafe {
                cpython_abi::api::buffer::PyBuffer_IsContiguous(lease.descriptor(), b'C' as _) != 0
            },
        }
    }

    /// Borrow the current contents after argument conversions/reentry.
    ///
    /// # Safety
    /// No callback may execute while this slice is in use: an export prevents
    /// resizing, but does not prevent mutation of existing bytes.
    pub unsafe fn bytes<'view>(&'view self, py: &'view crate::PyToken) -> &'view [u8] {
        match &self.0 {
            ByteInputStorage::Exact(owner) => owner.bytes_slice(py).expect("admitted exact bytes"),
            ByteInputStorage::Export(lease) => unsafe { buffer_bytes(lease) },
        }
    }
}

/// Transfer an existing typed Arc through its root native-handle bridge.
///
/// # Safety
/// `new` must be the concrete root bridge declared for this exact `T`; it must
/// consume the raw Arc on both success and allocation failure.
#[inline]
pub unsafe fn transfer_native_arc<T>(
    owner: std::sync::Arc<std::sync::Mutex<T>>,
    new: unsafe extern "C" fn(*const std::ffi::c_void) -> u64,
) -> u64 {
    unsafe { new(std::sync::Arc::into_raw(owner).cast()) }
}

/// Reconstruct the one typed Arc reference returned by its root clone bridge.
///
/// # Safety
/// `clone` must be the concrete root bridge declared for this exact `T`.
#[inline]
pub unsafe fn clone_native_arc<T>(
    bits: u64,
    clone: unsafe extern "C" fn(u64) -> *const std::ffi::c_void,
) -> Option<std::sync::Arc<std::sync::Mutex<T>>> {
    let pointer = unsafe { clone(bits) };
    if pointer.is_null() {
        return None;
    }
    Some(unsafe { std::sync::Arc::from_raw(pointer.cast()) })
}
