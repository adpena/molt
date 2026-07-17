//! Compile-time socket capabilities shared by native and WASM-host lanes.
//!
//! Cargo features express requested functionality; `molt_has_net_io` expresses
//! the build-script-proven native implementation capability. Keep OS socket
//! type flags behind this authority so consumers never infer support from a
//! feature or target family independently.

#[cfg(all(molt_has_net_io, unix))]
const SOCKET_TYPE_CREATION_FLAGS: i32 = libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;

#[cfg(all(molt_has_net_io, unix))]
const SOCKET_NONBLOCK_FLAG: i32 = libc::SOCK_NONBLOCK;

#[inline(always)]
pub(super) const fn base_socket_type(socket_type: i32) -> i32 {
    #[cfg(all(molt_has_net_io, unix))]
    {
        return socket_type & !SOCKET_TYPE_CREATION_FLAGS;
    }
    #[cfg(not(all(molt_has_net_io, unix)))]
    {
        socket_type
    }
}

#[inline(always)]
pub(super) const fn socket_type_requests_nonblocking(socket_type: i32) -> bool {
    #[cfg(all(molt_has_net_io, unix))]
    {
        return socket_type & SOCKET_NONBLOCK_FLAG != 0;
    }
    #[cfg(not(all(molt_has_net_io, unix)))]
    {
        let _ = socket_type;
        false
    }
}

#[cfg(all(test, molt_has_net_io, unix))]
mod tests {
    use super::*;

    #[test]
    fn unix_socket_creation_flags_are_one_canonical_capability() {
        let requested = libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
        assert_eq!(base_socket_type(requested), libc::SOCK_STREAM);
        assert!(socket_type_requests_nonblocking(requested));
        assert!(!socket_type_requests_nonblocking(libc::SOCK_STREAM));
    }
}
