//! `msghdr` length fields as the target's libc declares them.
//!
//! The assignment to the actual libc field selects each conversion's type.
//! GNU libc and musl differ even on the same OS, so an OS name cannot own
//! these widths. Checked monomorphized conversions need no runtime dispatch.

/// `msg_controllen` for a control buffer of `len` bytes; `None` when the
/// field cannot hold it.
#[inline]
pub fn msg_controllen<T: TryFrom<usize>>(len: usize) -> Option<T> {
    T::try_from(len).ok()
}

/// `msg_iovlen` for `len` iovecs; `None` when the field cannot hold it.
#[inline]
pub fn msg_iovlen<T: TryFrom<usize>>(len: usize) -> Option<T> {
    T::try_from(len).ok()
}

/// A received `msg_controllen` as a 32-bit byte count; `None` above `u32`.
#[inline]
pub fn control_len_u32<T: TryInto<u32>>(len: T) -> Option<u32> {
    len.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_libc_fields_select_their_own_conversions() {
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        for len in [0, 16, 4096] {
            msg.msg_controllen = msg_controllen(len).unwrap();
            msg.msg_iovlen = msg_iovlen(len).unwrap();
            assert_eq!(control_len_u32(msg.msg_controllen), Some(len as u32));
            assert_eq!(msg.msg_iovlen as u128, len as u128);
        }
    }

    #[test]
    fn signed_and_unsigned_field_boundaries_do_not_truncate() {
        assert_eq!(msg_controllen::<u32>(u32::MAX as usize), Some(u32::MAX));
        assert_eq!(msg_iovlen::<i32>(i32::MAX as usize), Some(i32::MAX));
        assert_eq!(msg_iovlen::<i32>(i32::MAX as usize + 1), None);
        assert_eq!(msg_controllen::<usize>(usize::MAX), Some(usize::MAX));
        assert_eq!(msg_iovlen::<usize>(usize::MAX), Some(usize::MAX));
        assert_eq!(control_len_u32(-1i32), None);
        assert_eq!(control_len_u32(u32::MAX), Some(u32::MAX));
        if let Some(above_u32) = (u32::MAX as usize).checked_add(1) {
            assert_eq!(msg_controllen::<u32>(above_u32), None);
            assert_eq!(control_len_u32(above_u32), None);
        }
    }
}
