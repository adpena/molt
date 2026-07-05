use std::fs::File;
use std::io;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
use std::path::Path;

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, UnlockFileEx};
#[cfg(windows)]
use windows_sys::Win32::System::IO::OVERLAPPED;

use super::super::super::io_limits::ensure_output_parent_dir;
use super::paths::stdlib_cache_publish_lock_path;

#[cfg(unix)]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or_default())?;
    let lock_path = stdlib_cache_publish_lock_path(stdlib_path);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let lock_rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if lock_rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let result = body();
    let unlock_rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if unlock_rc != 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(windows)]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    ensure_output_parent_dir(stdlib_path.to_str().unwrap_or_default())?;
    let lock_path = stdlib_cache_publish_lock_path(stdlib_path);
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    let mut overlapped = OVERLAPPED::default();
    let lock_rc = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if lock_rc == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = body();
    let unlock_rc = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
    if unlock_rc == 0 {
        return Err(io::Error::last_os_error());
    }
    result
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    _stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    body()
}
