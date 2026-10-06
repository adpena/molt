//! `msghdr` length fields as the target's libc declares them.
//!
//! Linux and Android declare `msg_iovlen` and `msg_controllen` as `size_t`;
//! the BSD family (macOS included) declares `msg_iovlen` as `int` and
//! `msg_controllen` as `socklen_t`. Each conversion is written once here, so
//! the lint that rejects a same-type conversion is allowed only on these
//! functions and every caller spells the platform fact the same way.

/// The type of `libc::msghdr::msg_controllen` on this target.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub type MsgControlLen = usize;
/// The type of `libc::msghdr::msg_controllen` on this target.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub type MsgControlLen = libc::socklen_t;

/// The type of `libc::msghdr::msg_iovlen` on this target.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub type MsgIovLen = usize;
/// The type of `libc::msghdr::msg_iovlen` on this target.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub type MsgIovLen = std::ffi::c_int;

// The aliases must be the field types libc declares; a mismatch fails here.
const _: () = {
    fn field_types(msg: libc::msghdr) -> (MsgControlLen, MsgIovLen) {
        (msg.msg_controllen, msg.msg_iovlen)
    }
    let _ = field_types;
};

/// `msg_controllen` for a control buffer of `len` bytes; `None` when the
/// field cannot hold it.
#[allow(
    clippy::useless_conversion,
    reason = "msg_controllen is size_t on Linux and Android, socklen_t elsewhere"
)]
#[inline]
pub fn msg_controllen(len: usize) -> Option<MsgControlLen> {
    MsgControlLen::try_from(len).ok()
}

/// `msg_iovlen` for `len` iovecs; `None` when the field cannot hold it.
#[allow(
    clippy::useless_conversion,
    reason = "msg_iovlen is size_t on Linux and Android, int elsewhere"
)]
#[inline]
pub fn msg_iovlen(len: usize) -> Option<MsgIovLen> {
    MsgIovLen::try_from(len).ok()
}

/// A received `msg_controllen` as a 32-bit byte count; `None` above `u32`.
#[allow(
    clippy::useless_conversion,
    reason = "msg_controllen is size_t on Linux and Android, socklen_t elsewhere"
)]
#[inline]
pub fn control_len_u32(len: MsgControlLen) -> Option<u32> {
    u32::try_from(len).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_round_trip_and_zero_is_always_representable() {
        assert_eq!(msg_controllen(0).and_then(control_len_u32), Some(0));
        assert_eq!(msg_controllen(4096).and_then(control_len_u32), Some(4096));
        assert!(msg_iovlen(0).is_some());
        assert!(msg_iovlen(16).is_some());
    }

    #[test]
    fn a_length_above_the_field_is_refused_where_the_field_is_narrower() {
        // `size_t` fields hold every usize; the BSD family's narrower fields
        // refuse what they cannot hold instead of truncating.
        let widest = size_of::<MsgControlLen>() == size_of::<usize>();
        assert_eq!(msg_controllen(usize::MAX).is_some(), widest);
        let widest = size_of::<MsgIovLen>() == size_of::<usize>();
        assert_eq!(msg_iovlen(usize::MAX).is_some(), widest);
    }
}
