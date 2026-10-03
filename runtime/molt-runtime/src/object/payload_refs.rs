//! Reference publication for compiler closure and suspension payloads.
//!
//! The caller owns layout admission and keeps the payload alive across callback
//! reentry. These operations hold no Rust borrow across a reference release.

use crate::{MoltObject, PyToken, dec_ref_bits, inc_ref_bits, obj_from_bits};

/// Consume one incoming owner, publish it, and return the displaced owner.
/// Multi-field retirement can exchange every edge before releasing any of them.
///
/// # Safety
/// `owner` is a live object payload under the GIL; `offset` addresses an aligned,
/// initialized reference word admitted by its closure layout or task shape.
#[inline]
pub(crate) unsafe fn exchange_owned(
    py: &PyToken<'_>,
    owner: *mut u8,
    offset: usize,
    value: u64,
) -> u64 {
    crate::gil_assert();
    if obj_from_bits(value).as_ptr().is_some() {
        unsafe { super::object_mark_has_ptrs(py, owner) };
    }
    unsafe { owner.add(offset).cast::<u64>().replace(value) }
}

/// Store a borrowed value; self-assignment retains the existing sole owner.
/// The safety contract is the same as `exchange_owned`.
#[inline]
pub(crate) unsafe fn store_borrowed(py: &PyToken<'_>, owner: *mut u8, offset: usize, value: u64) {
    crate::gil_assert();
    if unsafe { *owner.add(offset).cast::<u64>() } == value {
        if obj_from_bits(value).as_ptr().is_some() {
            unsafe { super::object_mark_has_ptrs(py, owner) };
        }
        return;
    }
    inc_ref_bits(py, value);
    unsafe { store_owned(py, owner, offset, value) };
}

/// Consume an incoming owner even if its identity equals the displaced edge.
/// Publish before release; never overwrite a finalizer's reentrant replacement.
/// The safety contract is the same as `exchange_owned`.
#[inline]
pub(crate) unsafe fn store_owned(py: &PyToken<'_>, owner: *mut u8, offset: usize, value: u64) {
    let previous = unsafe { exchange_owned(py, owner, offset, value) };
    dec_ref_bits(py, previous);
}

/// Transfer a reference out without running a callback.
/// The safety contract is the same as `exchange_owned`.
#[inline]
pub(crate) unsafe fn take(owner: *mut u8, offset: usize) -> u64 {
    crate::gil_assert();
    unsafe {
        owner
            .add(offset)
            .cast::<u64>()
            .replace(MoltObject::none().bits())
    }
}

/// Clear a compile-time-sized reference prefix before releasing any owner.
/// Storage is on the stack: retirement never allocates or partially publishes.
/// The safety contract admits all N reference words at the payload start.
#[inline]
pub(crate) unsafe fn clear_prefix<const N: usize>(py: &PyToken<'_>, owner: *mut u8) {
    let previous: [u64; N] =
        std::array::from_fn(|index| unsafe { take(owner, index * std::mem::size_of::<u64>()) });
    for value in previous {
        dec_ref_bits(py, value);
    }
}
