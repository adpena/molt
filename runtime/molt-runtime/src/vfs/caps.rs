//! Mount-to-capability mapping for VFS access control.

use crate::vfs::VfsError;

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
    ("/dev", "", ""),  // always readable and writable
];

/// Check whether the given operation is allowed on the mount.
/// Returns Ok(()) if allowed, Err with diagnostic if denied.
pub fn check_mount_capability(
    mount_prefix: &str,
    is_write: bool,
    has_cap: &dyn Fn(&str) -> bool,
) -> Result<(), VfsError> {
    let entry = MOUNT_CAPABILITIES
        .iter()
        .find(|(prefix, _, _)| mount_prefix.starts_with(prefix));

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
