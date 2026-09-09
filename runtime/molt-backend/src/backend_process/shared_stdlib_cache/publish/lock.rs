use std::fs::File;
use std::io;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

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
    let unlock = if unlock_rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    };
    finish_publication_lock(&lock_path, result, unlock)
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
    let unlock = if unlock_rc != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    };
    finish_publication_lock(&lock_path, result, unlock)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn with_shared_stdlib_cache_publish_lock<T>(
    _stdlib_path: &Path,
    body: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    body()
}

/// Releasing custody is a second operation, never a replacement for the
/// publication result. Keep the primary typed error reachable through source().
fn finish_publication_lock<T>(
    lock_path: &Path,
    body: io::Result<T>,
    unlock: io::Result<()>,
) -> io::Result<T> {
    let Err(unlock) = unlock else {
        return body;
    };
    let body = body.err();
    let kind = body.as_ref().map_or(unlock.kind(), io::Error::kind);
    Err(io::Error::new(
        kind,
        PublicationLockReleaseError {
            lock_path: lock_path.to_path_buf(),
            body,
            unlock,
        },
    ))
}

#[derive(Debug)]
struct PublicationLockReleaseError {
    lock_path: PathBuf,
    body: Option<io::Error>,
    unlock: io::Error,
}

impl std::fmt::Display for PublicationLockReleaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(body) = &self.body {
            write!(formatter, "{body}; ")?;
        } else {
            write!(formatter, "publication body completed successfully; ")?;
        }
        write!(
            formatter,
            "shared stdlib publication lock release failed for '{}': {}",
            self.lock_path.display(),
            self.unlock
        )
    }
}

impl std::error::Error for PublicationLockReleaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.body.as_ref().unwrap_or(&self.unlock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend_process::atomic_publish::{AtomicPublicationError, PublicationState};

    #[test]
    fn successful_unlock_preserves_body_value_or_typed_failure() {
        let path = Path::new("stdlib.a.publish.lock");
        assert_eq!(
            finish_publication_lock(path, Ok(17), Ok(())).expect("success"),
            17
        );
        let primary = AtomicPublicationError::new(
            Path::new("stdlib.a"),
            PublicationState::Replaced,
            io::Error::other("primary publication failure"),
        );
        let result: io::Result<()> = finish_publication_lock(path, Err(primary.into()), Ok(()));
        let error = result.expect_err("body failure survives successful unlock");
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<AtomicPublicationError>())
                .expect("original typed publication error")
                .state(),
            PublicationState::Replaced
        );
    }

    #[test]
    fn failed_unlock_keeps_primary_publication_state_and_both_diagnostics() {
        let primary = AtomicPublicationError::new(
            Path::new("stdlib.a"),
            PublicationState::Replaced,
            io::Error::new(io::ErrorKind::InvalidData, "primary durability failure"),
        );
        let result: io::Result<()> = finish_publication_lock(
            Path::new("stdlib.a.publish.lock"),
            Err(primary.into()),
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected unlock failure",
            )),
        );
        let error = result.expect_err("both failures propagate");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let detail = error.to_string();
        for expected in [
            "destination replaced",
            "primary durability failure",
            "injected unlock failure",
            "stdlib.a.publish.lock",
        ] {
            assert!(detail.contains(expected), "missing {expected}: {detail}");
        }
        let release = error.get_ref().expect("release wrapper");
        let primary = release
            .source()
            .expect("primary source")
            .downcast_ref::<io::Error>()
            .expect("original io error");
        assert_eq!(
            primary
                .get_ref()
                .and_then(|error| error.downcast_ref::<AtomicPublicationError>())
                .expect("typed publication state remains reachable")
                .state(),
            PublicationState::Replaced
        );
    }

    #[test]
    fn failed_unlock_after_success_is_not_reported_as_publication_rollback() {
        let result = finish_publication_lock(
            Path::new("stdlib.a.publish.lock"),
            Ok(17),
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected unlock failure",
            )),
        );
        let error = result.expect_err("unlock failure is loud");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            error
                .to_string()
                .contains("publication body completed successfully")
        );
        assert!(error.to_string().contains("injected unlock failure"));
        assert_eq!(
            error
                .get_ref()
                .expect("release wrapper")
                .source()
                .expect("unlock source")
                .downcast_ref::<io::Error>()
                .expect("unlock io error")
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
