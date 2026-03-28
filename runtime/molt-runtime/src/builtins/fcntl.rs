#![allow(dead_code, unused_imports)]
//! fcntl — file descriptor control intrinsics for non-blocking I/O.
//!
//! Provides `fcntl(fd, cmd[, arg])` and the associated constants needed by
//! trio and other async I/O libraries to set sockets to non-blocking mode.
//!
//! On WASM all fds are already non-blocking (I/O goes through host imports),
//! so fcntl is a sensible no-op: F_GETFL returns 0, F_SETFL succeeds silently.

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::builtins::numbers::int_bits_from_i64;
use crate::*;

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

// ── Constants ─────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_fcntl_f_getfl() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            int_bits_from_i64(_py, libc::F_GETFL as i64)
        }
        #[cfg(target_arch = "wasm32")]
        {
            int_bits_from_i64(_py, 3_i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fcntl_f_setfl() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            int_bits_from_i64(_py, libc::F_SETFL as i64)
        }
        #[cfg(target_arch = "wasm32")]
        {
            int_bits_from_i64(_py, 4_i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fcntl_f_getfd() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            int_bits_from_i64(_py, libc::F_GETFD as i64)
        }
        #[cfg(target_arch = "wasm32")]
        {
            int_bits_from_i64(_py, 1_i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fcntl_f_setfd() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            int_bits_from_i64(_py, libc::F_SETFD as i64)
        }
        #[cfg(target_arch = "wasm32")]
        {
            int_bits_from_i64(_py, 2_i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fcntl_fd_cloexec() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            int_bits_from_i64(_py, libc::FD_CLOEXEC as i64)
        }
        #[cfg(target_arch = "wasm32")]
        {
            int_bits_from_i64(_py, 1_i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fcntl_o_nonblock() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, libc::O_NONBLOCK as i64) })
}

// ── fcntl(fd, cmd[, arg]) ─────────────────────────────────────────────────

/// `fcntl(fd, cmd, arg) -> int`
///
/// The `arg` parameter is optional at the Python level; the stdlib wrapper
/// passes 0 when omitted.
///
/// On native: delegates to libc::fcntl.
/// On WASM: F_GETFL returns 0, F_SETFL/F_SETFD return 0 (success),
///           F_GETFD returns FD_CLOEXEC (pretend all fds have CLOEXEC).
#[unsafe(no_mangle)]
pub extern "C" fn molt_fcntl(fd_bits: u64, cmd_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "process");
        audit_capability_decision("fcntl.fcntl", "process", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(
                _py,
                "PermissionError",
                "missing process capability for fcntl operations",
            );
        }

        let fd = to_i64(obj_from_bits(fd_bits)).unwrap_or(-1) as i32;
        let cmd = to_i64(obj_from_bits(cmd_bits)).unwrap_or(-1) as i32;
        let arg = to_i64(obj_from_bits(arg_bits)).unwrap_or(0) as i32;

        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let rc = unsafe { libc::fcntl(fd, cmd, arg) };
            if rc < 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            int_bits_from_i64(_py, rc as i64)
        }

        #[cfg(target_arch = "wasm32")]
        {
            // On WASM, all I/O is inherently non-blocking through host imports.
            // Return sensible defaults so trio's non-blocking setup succeeds.
            let _ = (fd, arg);
            if cmd == 3 {
                // F_GETFL: return 0 (no special flags)
                int_bits_from_i64(_py, 0_i64)
            } else if cmd == 1 {
                // F_GETFD: pretend CLOEXEC is set
                int_bits_from_i64(_py, 1_i64)
            } else {
                // F_SETFL / F_SETFD / anything else: success
                int_bits_from_i64(_py, 0_i64)
            }
        }

        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            let _ = (fd, cmd, arg);
            raise_exception::<u64>(_py, "OSError", "fcntl is not supported on this platform")
        }
    })
}
