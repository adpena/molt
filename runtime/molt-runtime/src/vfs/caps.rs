//! Mount-to-capability mapping for VFS access control.

use crate::vfs::VfsError;
#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicU8, Ordering};

// ---------------------------------------------------------------------------
// IO mode selection
// ---------------------------------------------------------------------------

/// Controls how the runtime performs IO operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoMode {
    /// Real filesystem access (default).
    Real,
    /// All IO routed through the in-memory VFS.
    Virtual,
    /// IO operations invoke host-provided callbacks.
    Callback,
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static IO_MODE: RefCell<IoMode> = const { RefCell::new(IoMode::Real) };
}

#[cfg(target_arch = "wasm32")]
static IO_MODE: AtomicU8 = AtomicU8::new(0);

#[cfg(target_arch = "wasm32")]
#[inline]
const fn io_mode_encode(mode: IoMode) -> u8 {
    match mode {
        IoMode::Real => 0,
        IoMode::Virtual => 1,
        IoMode::Callback => 2,
    }
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn io_mode_decode(bits: u8) -> IoMode {
    match bits {
        1 => IoMode::Virtual,
        2 => IoMode::Callback,
        _ => IoMode::Real,
    }
}

/// Set the IO mode for the current thread.
pub fn set_io_mode(mode: IoMode) {
    #[cfg(target_arch = "wasm32")]
    {
        IO_MODE.store(io_mode_encode(mode), Ordering::Relaxed);
    }
    #[cfg(not(target_arch = "wasm32"))]
    IO_MODE.with(|cell| {
        *cell.borrow_mut() = mode;
    });
}

/// Get the current IO mode.
pub fn io_mode() -> IoMode {
    #[cfg(target_arch = "wasm32")]
    {
        return io_mode_decode(IO_MODE.load(Ordering::Relaxed));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        IO_MODE.with(|cell| *cell.borrow())
    }
}

/// Sentinel for "never writable" (distinct from "" which means "always allowed").
const NEVER: &str = "!";

/// (mount_prefix, read_capability, write_capability)
/// Empty string "" = no capability required (always allowed).
/// "!" = never allowed (read-only mount).
/// Any other string = capability name that must be granted.
const MOUNT_CAPABILITIES: &[(&str, &str, &str)] = &[
    ("/bundle", "fs.bundle.read", NEVER),
    ("/tmp", "fs.tmp.read", "fs.tmp.write"),
    ("/state", "fs.state.read", "fs.state.write"),
    ("/dev", "", ""), // always readable and writable
];

/// Check whether the given operation is allowed on the mount.
/// Returns Ok(()) if allowed, Err with diagnostic if denied.
pub fn check_mount_capability(
    mount_prefix: &str,
    is_write: bool,
    has_cap: &dyn Fn(&str) -> bool,
) -> Result<(), VfsError> {
    let entry = MOUNT_CAPABILITIES.iter().find(|(prefix, _, _)| {
        mount_prefix == *prefix
            || (mount_prefix.len() > prefix.len()
                && mount_prefix.starts_with(prefix)
                && mount_prefix.as_bytes()[prefix.len()] == b'/')
    });

    let Some((_, read_cap, write_cap)) = entry else {
        return Err(VfsError::NotFound);
    };

    if is_write {
        if *write_cap == NEVER {
            return Err(VfsError::ReadOnly);
        }
        if write_cap.is_empty() {
            return Ok(()); // always allowed
        }
        if !has_cap(write_cap) {
            return Err(VfsError::CapabilityDenied(format!(
                "operation requires '{write_cap}' capability\n  \
                 mount: {mount_prefix}\n  \
                 hint: set MOLT_CAPABILITIES={write_cap} or add to host profile"
            )));
        }
    } else {
        if !read_cap.is_empty() && !has_cap(read_cap) {
            return Err(VfsError::CapabilityDenied(format!(
                "operation requires '{read_cap}' capability\n  \
                 mount: {mount_prefix}\n  \
                 hint: set MOLT_CAPABILITIES={read_cap} or add to host profile"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: always-grant capability checker.
    fn allow_all(_cap: &str) -> bool {
        true
    }

    /// Helper: never-grant capability checker.
    fn deny_all(_cap: &str) -> bool {
        false
    }

    #[test]
    fn dev_exact_match_allowed() {
        let result = check_mount_capability("/dev", false, &allow_all);
        assert!(
            result.is_ok(),
            "/dev exact read should be allowed without caps"
        );
    }

    #[test]
    fn dev_null_with_separator_allowed() {
        let result = check_mount_capability("/dev/null", false, &allow_all);
        assert!(
            result.is_ok(),
            "/dev/null should match /dev mount via separator"
        );
    }

    #[test]
    fn devious_must_not_match_dev() {
        let result = check_mount_capability("/devious", false, &allow_all);
        assert!(
            matches!(result, Err(VfsError::NotFound)),
            "/devious must NOT match /dev — got {result:?}"
        );
    }

    #[test]
    fn tmp_private_must_not_match_tmp() {
        let result = check_mount_capability("/tmp_private", false, &allow_all);
        assert!(
            matches!(result, Err(VfsError::NotFound)),
            "/tmp_private must NOT match /tmp — got {result:?}"
        );
    }

    #[test]
    fn bundle_extra_must_not_match_bundle() {
        let result = check_mount_capability("/bundle_extra", false, &allow_all);
        assert!(
            matches!(result, Err(VfsError::NotFound)),
            "/bundle_extra must NOT match /bundle — got {result:?}"
        );
    }

    #[test]
    fn tmp_exact_checks_read_cap_granted() {
        let result = check_mount_capability("/tmp", false, &allow_all);
        assert!(result.is_ok(), "/tmp read with cap granted should succeed");
    }

    #[test]
    fn tmp_exact_checks_read_cap_denied() {
        let result = check_mount_capability("/tmp", false, &deny_all);
        assert!(
            matches!(result, Err(VfsError::CapabilityDenied(_))),
            "/tmp read without cap should be CapabilityDenied — got {result:?}"
        );
    }

    #[test]
    fn tmp_subpath_checks_read_cap() {
        let result = check_mount_capability("/tmp/foo", false, &allow_all);
        assert!(
            result.is_ok(),
            "/tmp/foo read with cap granted should succeed"
        );

        let result = check_mount_capability("/tmp/foo", false, &deny_all);
        assert!(
            matches!(result, Err(VfsError::CapabilityDenied(_))),
            "/tmp/foo read without cap should be CapabilityDenied — got {result:?}"
        );
    }

    #[test]
    fn bundle_write_is_readonly() {
        let result = check_mount_capability("/bundle", true, &allow_all);
        assert!(
            matches!(result, Err(VfsError::ReadOnly)),
            "/bundle write should be ReadOnly — got {result:?}"
        );
    }

    #[test]
    fn unknown_mount_is_not_found() {
        let result = check_mount_capability("/unknown", false, &allow_all);
        assert!(
            matches!(result, Err(VfsError::NotFound)),
            "/unknown should be NotFound — got {result:?}"
        );
    }

    #[test]
    fn io_mode_roundtrip_updates_and_reads_current_mode() {
        set_io_mode(IoMode::Real);
        assert_eq!(io_mode(), IoMode::Real);
        set_io_mode(IoMode::Virtual);
        assert_eq!(io_mode(), IoMode::Virtual);
        set_io_mode(IoMode::Callback);
        assert_eq!(io_mode(), IoMode::Callback);
        set_io_mode(IoMode::Real);
    }
}
