//! Width-checked decoding for integer-carried runtime ABI addresses.
//!
//! Molt's portable all-`u64` ABI carries native and linear-memory addresses in
//! integer slots.  Turning those slots into pointers with `as usize as *mut T`
//! silently truncated malformed addresses on 32-bit targets and discarded the
//! explicit exposed-provenance reconstruction used elsewhere in the runtime.
//! Keep that policy here so every ABI consumer fails before dereference when an
//! address cannot be represented by the active target.

#[inline(always)]
pub(crate) fn address(bits: u64) -> Option<usize> {
    molt_runtime_platform::utils::usize_from_bits(bits)
}

#[inline(always)]
pub(crate) fn const_ptr<T>(bits: u64) -> Option<*const T> {
    address(bits).map(core::ptr::with_exposed_provenance::<T>)
}

#[inline(always)]
pub(crate) fn mut_ptr<T>(bits: u64) -> Option<*mut T> {
    address(bits).map(core::ptr::with_exposed_provenance_mut::<T>)
}

#[inline(always)]
pub(crate) fn expose_address<T>(ptr: *const T) -> u64 {
    u64::try_from(ptr.expose_provenance()).expect("supported targets fit data addresses in u64")
}

/// Decode an executable address. Function-table indices on wasm32 are not
/// data pointers, so this authority deliberately avoids data-provenance APIs.
#[inline(always)]
pub(crate) fn function_ptr(bits: u64) -> Option<*const ()> {
    let address = address(bits)?;
    Some(unsafe { core::mem::transmute::<usize, *const ()>(address) })
}

#[inline(always)]
pub(crate) fn expose_function_address(ptr: *const ()) -> u64 {
    let address = unsafe { core::mem::transmute::<*const (), usize>(ptr) };
    u64::try_from(address).expect("supported function-address carriers fit in u64")
}

/// Validate every mechanically checkable precondition for constructing a
/// non-empty raw slice. Allocation membership, initialization, aliasing, and
/// lifetime remain the unsafe caller's responsibility.
#[inline(always)]
pub(crate) fn checked_slice_len<T>(ptr: *const T, len_bits: u64) -> Option<usize> {
    let len = address(len_bits)?;
    if len == 0 {
        return Some(0);
    }

    let alignment = core::mem::align_of::<T>();
    let ptr_addr = ptr.addr();
    if ptr_addr == 0 || ptr_addr % alignment != 0 {
        return None;
    }

    let byte_len = len.checked_mul(core::mem::size_of::<T>())?;
    if byte_len > isize::MAX as usize {
        return None;
    }
    ptr_addr.checked_add(byte_len)?;
    Some(len)
}

/// Decode an ABI length and borrow its raw element range without permitting
/// width truncation or a non-empty null range.
///
/// # Safety
///
/// For a nonzero decoded length, `ptr` must be aligned and valid for that many
/// initialized `T` values for the returned lifetime.
#[inline(always)]
pub(crate) unsafe fn slice<'a, T>(ptr: *const T, len_bits: u64) -> Option<&'a [T]> {
    let len = checked_slice_len(ptr, len_bits)?;
    if len == 0 {
        return Some(&[]);
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

/// Mutable counterpart to [`slice`].
///
/// # Safety
///
/// For a nonzero decoded length, `ptr` must be aligned, uniquely borrowed, and
/// valid for that many initialized `T` values for the returned lifetime.
#[inline(always)]
pub(crate) unsafe fn slice_mut<'a, T>(ptr: *mut T, len_bits: u64) -> Option<&'a mut [T]> {
    let len = checked_slice_len(ptr.cast_const(), len_bits)?;
    if len == 0 {
        return Some(&mut []);
    }
    Some(unsafe { core::slice::from_raw_parts_mut(ptr, len) })
}

#[cfg(test)]
mod tests {
    use super::{
        address, const_ptr, expose_address, expose_function_address, function_ptr, mut_ptr, slice,
        slice_mut,
    };

    #[repr(align(8))]
    struct AlignedZst;

    #[test]
    fn abi_address_round_trips_the_target_width() {
        let value = usize::MAX;
        let bits = u64::try_from(value).expect("supported targets fit pointer addresses in u64");
        assert_eq!(address(bits), Some(value));
        assert_eq!(const_ptr::<u8>(bits).map(|ptr| ptr.addr()), Some(value));
        assert_eq!(mut_ptr::<u8>(bits).map(|ptr| ptr.addr()), Some(value));
    }

    #[test]
    fn data_and_function_address_authorities_round_trip_separately() {
        let value = 7_u64;
        let data = &value as *const u64;
        assert_eq!(const_ptr::<u64>(expose_address(data)), Some(data));

        extern "C" fn target() {}
        let function = target as *const ();
        assert_eq!(
            function_ptr(expose_function_address(function)),
            Some(function)
        );
    }

    #[test]
    #[cfg(target_pointer_width = "32")]
    fn abi_address_rejects_high_bits_instead_of_truncating() {
        let malformed = u64::from(u32::MAX) + 1;
        assert_eq!(address(malformed), None);
        assert_eq!(const_ptr::<u8>(malformed), None);
        assert_eq!(mut_ptr::<u8>(malformed), None);
    }

    #[test]
    fn abi_slice_accepts_empty_null_and_rejects_nonempty_null() {
        assert_eq!(unsafe { slice::<u8>(core::ptr::null(), 0) }, Some(&[][..]));
        assert!(unsafe { slice::<u8>(core::ptr::null(), 1) }.is_none());
        assert_eq!(
            unsafe { slice_mut::<u8>(core::ptr::null_mut(), 0) }.map(|value| value.len()),
            Some(0)
        );
        assert!(unsafe { slice_mut::<u8>(core::ptr::null_mut(), 1) }.is_none());
    }

    #[test]
    fn abi_slice_rejects_misaligned_and_wrapping_ranges() {
        let misaligned = core::ptr::with_exposed_provenance::<u16>(1);
        assert!(unsafe { slice(misaligned, 1) }.is_none());

        let wrapping = core::ptr::with_exposed_provenance::<u8>(usize::MAX);
        assert!(unsafe { slice(wrapping, 1) }.is_none());
    }

    #[test]
    fn abi_slice_rejects_ranges_larger_than_isize_max() {
        let overlong_bytes =
            u64::try_from(isize::MAX as usize + 1).expect("isize::MAX + 1 fits the u64 ABI");
        let byte_ptr = core::ptr::NonNull::<u8>::dangling().as_ptr();
        assert!(unsafe { slice(byte_ptr, overlong_bytes) }.is_none());

        let overlong_u16 = u64::try_from(isize::MAX as usize / 2 + 1)
            .expect("an overlong u16 element count fits the u64 ABI");
        let u16_ptr = core::ptr::NonNull::<u16>::dangling().as_ptr();
        assert!(unsafe { slice(u16_ptr, overlong_u16) }.is_none());
    }

    #[test]
    fn abi_slice_handles_zero_sized_elements_without_rejecting_valid_counts() {
        let ptr = core::ptr::NonNull::<AlignedZst>::dangling().as_ptr();
        let max_len = u64::try_from(usize::MAX).expect("supported targets fit usize in u64");
        let values = unsafe { slice(ptr, max_len) }.expect("zero-sized span has zero bytes");
        assert_eq!(values.len(), usize::MAX);

        let misaligned = core::ptr::with_exposed_provenance::<AlignedZst>(1);
        assert!(unsafe { slice(misaligned, 1) }.is_none());
    }
}
