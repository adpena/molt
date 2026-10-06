//! Compile-time socket capabilities shared by native and WASM-host lanes.
//!
//! Cargo features express requested functionality; `molt_has_net_io` expresses
//! the build-script-proven native implementation capability. Socket type
//! creation flags come from the one platform authority,
//! `molt_runtime_platform::socket_constants`, which is 0 on targets whose C
//! library has no such flag (Apple platforms, for example), exactly as CPython
//! omits `socket.SOCK_NONBLOCK` and `socket.SOCK_CLOEXEC` there.

#[cfg(molt_has_net_io)]
use crate::socket_constants::{SOCK_CLOEXEC_FLAG, SOCK_NONBLOCK_FLAG};

#[cfg(molt_has_net_io)]
const SOCKET_TYPE_CREATION_FLAGS: i32 = SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG;

#[inline(always)]
pub(super) const fn base_socket_type(socket_type: i32) -> i32 {
    #[cfg(molt_has_net_io)]
    {
        socket_type & !SOCKET_TYPE_CREATION_FLAGS
    }
    #[cfg(not(molt_has_net_io))]
    {
        socket_type
    }
}

// The flag is 0 on targets whose C library lacks it; there the mask is
// constant-false by design, because no socket type can request the flag.
#[allow(clippy::bad_bit_mask)]
#[inline(always)]
pub(super) const fn socket_type_requests_nonblocking(socket_type: i32) -> bool {
    #[cfg(molt_has_net_io)]
    {
        socket_type & SOCK_NONBLOCK_FLAG != 0
    }
    #[cfg(not(molt_has_net_io))]
    {
        let _ = socket_type;
        false
    }
}

#[cfg(all(test, molt_has_net_io))]
mod tests {
    use super::*;

    #[test]
    fn socket_creation_flags_come_from_the_platform_authority() {
        let requested = libc::SOCK_STREAM | SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG;
        assert_eq!(base_socket_type(requested), libc::SOCK_STREAM);
        assert_eq!(
            socket_type_requests_nonblocking(requested),
            SOCK_NONBLOCK_FLAG != 0
        );
        assert!(!socket_type_requests_nonblocking(libc::SOCK_STREAM));
    }
}
