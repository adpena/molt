//! Molt env-host error protocol, shared by guest and native hosts.
include!("synthetic_errno_generated.rs");

/// Adopt a genuine target SDK error into Molt's guest errno domain.
/// Synthetic host statuses must remain GuestErrno and never enter std::io.
pub fn from_io_error(error: &std::io::Error) -> GuestErrno {
    if let Some(code) = error.raw_os_error() {
        #[cfg(target_os = "wasi")]
        return from_wasi_errno(code);
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(windows)]
            let code = crate::windows_abi::winerror_to_errno(code);
            return from_native_errno(code);
        }
        // Freestanding targets have no SDK errno domain. Only Rust's typed
        // ErrorKind is meaningful there.
        #[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
        let _ = code;
    }
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::WouldBlock => GuestErrno::EAGAIN,
        ErrorKind::Interrupted => GuestErrno::EINTR,
        ErrorKind::ConnectionRefused => GuestErrno::ECONNREFUSED,
        ErrorKind::ConnectionReset => GuestErrno::ECONNRESET,
        ErrorKind::ConnectionAborted => GuestErrno::ECONNABORTED,
        ErrorKind::NotConnected => GuestErrno::ENOTCONN,
        ErrorKind::AddrInUse => GuestErrno::EADDRINUSE,
        ErrorKind::AddrNotAvailable => GuestErrno::EADDRNOTAVAIL,
        ErrorKind::BrokenPipe => GuestErrno::EPIPE,
        ErrorKind::TimedOut => GuestErrno::ETIMEDOUT,
        ErrorKind::PermissionDenied => GuestErrno::EACCES,
        ErrorKind::NotFound => GuestErrno::ENOENT,
        ErrorKind::AlreadyExists => GuestErrno::EEXIST,
        ErrorKind::InvalidInput => GuestErrno::EINVAL,
        ErrorKind::NotADirectory => GuestErrno::ENOTDIR,
        ErrorKind::IsADirectory => GuestErrno::EISDIR,
        ErrorKind::ReadOnlyFilesystem => GuestErrno::EROFS,
        ErrorKind::OutOfMemory => GuestErrno::ENOMEM,
        ErrorKind::Unsupported => GuestErrno::ENOSYS,
        _ => GuestErrno::EIO,
    }
}

#[cfg(test)]
mod io_error_tests;
#[cfg(test)]
mod tests;
