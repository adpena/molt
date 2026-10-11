//! Foreign (C-extension) object custody.
//!
//! A `TYPE_ID_FOREIGN` heap object is Molt's first-class representation of a
//! genuine C-extension `PyObject*` (a numpy static type, an extension instance,
//! a descriptor, …) that has crossed *into* compiled Python. Before this, such
//! objects reached Molt as the bridge's synthetic `0xA11C…` identity tokens,
//! which are NOT valid `MoltObject` bit patterns — so any Molt-side operation on
//! them (`DType.__name__`, calling `DType(...)`, `setattr(obj, k, v)`) failed
//! because the runtime could not decode the handle.
//!
//! The wrapper stores the raw C `PyObject*` and routes attribute access and
//! calls back through the object's OWN CPython type slots (`tp_getattro` /
//! `tp_setattro` / `tp_call`) via the `molt-cpython-abi` bridge. The wrapper
//! holds ONE strong reference to the C object for its lifetime: the bridge
//! `Py_INCREF`s at mint time (in `molt_value_for_pyobj`) and this module's
//! `foreign_release` calls back into the bridge to `Py_DECREF` + drop the identity
//! mapping when the Molt wrapper is collected.

use crate::{PyToken, TYPE_ID_FOREIGN};

use super::{ObjectAuxPreselection, alloc_object_zeroed_unpublished_with_aux, bits_from_ptr};

/// Allocate a `TYPE_ID_FOREIGN` wrapper around the C `PyObject*` at address
/// `c_ptr`. The wrapper payload is exactly the pointer value (`usize`); the
/// strong reference custody (`Py_INCREF`) is the caller's responsibility (the
/// bridge does it at mint time so cache hits do not double-count).
///
/// Returns the wrapper's NaN-boxed handle bits, or reserved zero on failure.
pub(crate) fn foreign_new(_py: &PyToken<'_>, c_ptr: usize) -> u64 {
    if unsafe { molt_cpython_abi::bridge::molt_foreign_object_is_gc_capable(c_ptr) }
        && !super::gc::native_gc_is_enrolled(c_ptr)
    {
        crate::raise_exception::<()>(
            _py,
            "TypeError",
            "GC-capable foreign object was not enrolled in the runtime GC authority",
        );
        return 0;
    }
    let total = std::mem::size_of::<crate::MoltHeader>() + std::mem::size_of::<usize>();
    let ptr = alloc_object_zeroed_unpublished_with_aux(
        _py,
        total,
        TYPE_ID_FOREIGN,
        ObjectAuxPreselection::Default,
    );
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        *(ptr as *mut usize) = c_ptr;
        super::gc::gc_publish_initialized(_py, ptr);
    }
    bits_from_ptr(ptr)
}

/// Read the wrapped C `PyObject*` (as a `usize`) directly from an already
/// type-checked foreign wrapper object pointer.
///
/// # Safety
/// `ptr` must point to a live `TYPE_ID_FOREIGN` object.
pub(crate) unsafe fn foreign_ptr_from_obj(ptr: *mut u8) -> usize {
    unsafe { *(ptr as *const usize) }
}

/// Drop hook for a `TYPE_ID_FOREIGN` object: release the bridge identity mapping
/// and the strong reference the wrapper held on the C object. Called from the
/// object drop dispatch when the wrapper's Molt refcount reaches zero.
pub(crate) fn foreign_detach(ptr: *mut u8) -> usize {
    unsafe { (ptr as *mut usize).replace(0) }
}

pub(crate) fn foreign_release(c_ptr: usize) {
    if c_ptr == 0 {
        return;
    }
    // Route back into the ABI bridge: remove the c_ptr <-> wrapper identity
    // entries and Py_DECREF the C object (balances the Py_INCREF the bridge did
    // when it minted this wrapper). Direct cross-crate call — the ABI is
    // statically linked into the same binary as the runtime.
    retire_native_reference(|| unsafe {
        molt_cpython_abi::bridge::molt_foreign_object_release(c_ptr);
    });
}

/// Complete native retirement before returning to a runtime operation. Foreign
/// wrapper release, owned C temporaries, and operations that return a scalar
/// after retiring their native temporaries share this completion boundary.
#[inline]
pub(crate) fn retire_native_reference<T>(release: impl FnOnce() -> T) -> T {
    let result = release();
    // A valid native deallocator may move the active raised exception between
    // channels through PyErr_GetRaisedException/PyErr_SetRaisedException.
    // Complete that native reentry before returning to a runtime consumer.
    crate::cpython_abi_hooks::transfer_pending_cpython_exception();
    result
}

/// Ordinary C-reference retirement returning to a runtime consumer. Native
/// deallocators see the current error, then the existing completion authority
/// imports any legal C-channel restoration before runtime error checks resume.
pub(crate) enum NativeReferenceRetirement {}

unsafe impl molt_cpython_abi::api::refcount::ReferenceRetirement for NativeReferenceRetirement {
    #[inline]
    fn retire(release: impl FnOnce()) {
        retire_native_reference(release);
    }
}

pub(crate) type NativeObjectOwner =
    molt_cpython_abi::api::refcount::PyObjectOwner<NativeReferenceRetirement>;

pub(crate) type RuntimeBufferLease =
    molt_cpython_abi::api::memory::BufferLease<NativeReferenceRetirement>;

const _: () = {
    assert!(std::mem::size_of::<RuntimeBufferLease>() == std::mem::size_of::<usize>());
    assert!(std::mem::align_of::<RuntimeBufferLease>() == std::mem::align_of::<usize>());
    assert!(std::mem::size_of::<NativeObjectOwner>() == std::mem::size_of::<usize>());
    assert!(std::mem::align_of::<NativeObjectOwner>() == std::mem::align_of::<usize>());
};
