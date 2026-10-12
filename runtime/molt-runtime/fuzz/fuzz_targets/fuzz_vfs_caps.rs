#![no_main]

//! Fuzz target for VFS capability checking (`mount_requirements`).
//!
//! Generates arbitrary mount prefixes and selected access modes, then verifies
//! security invariants:
//!   - Prefix-confusion attacks (e.g. "/devious") must never succeed.
//!   - Only known mount prefixes at path boundaries may return `Ok`.
//!   - Read-only mounts must retain their write constraint.

use libfuzzer_sys::fuzz_target;
use molt_runtime::vfs::VfsError;
use molt_runtime::vfs::caps::mount_requirements;

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime fuzz_vfs_caps"
));

/// Known mount prefixes from the capability table.
const KNOWN_PREFIXES: &[&str] = &["/bundle", "/tmp", "/state", "/dev"];

/// Returns true when `path` matches a known prefix at a path boundary:
/// either exact match or the character immediately after the prefix is '/'.
fn matches_known_prefix(path: &str) -> bool {
    KNOWN_PREFIXES.iter().any(|prefix| {
        path == *prefix
            || (path.len() > prefix.len()
                && path.starts_with(prefix)
                && path.as_bytes()[prefix.len()] == b'/')
    })
}

fuzz_target!(|data: &[u8]| {
    // Need at least 2 bytes: 1 for flags, rest for the mount prefix string.
    if data.len() < 2 {
        return;
    }

    let flags = data[0];
    let is_write = flags & 0x01 != 0;
    let is_read = flags & 0x02 != 0;

    // Build the mount prefix string. Use bit 3 to decide whether to use a
    // "near-miss" prefix (more likely to exercise boundary logic) or raw bytes.
    let mount_prefix: String = if flags & 0x08 != 0 {
        // Near-miss mode: pick a base prefix and append fuzzer-controlled suffix.
        let base_idx = (flags >> 4) as usize % KNOWN_PREFIXES.len();
        let base = KNOWN_PREFIXES[base_idx];
        let suffix = String::from_utf8_lossy(&data[1..]);
        format!("{base}{suffix}")
    } else {
        // Raw mode: interpret remaining bytes as a UTF-8 string.
        // Prepend '/' 50% of the time to bias toward plausible paths.
        let raw = String::from_utf8_lossy(&data[1..]);
        if flags & 0x10 != 0 {
            format!("/{raw}")
        } else {
            raw.into_owned()
        }
    };

    let result = mount_requirements(&mount_prefix, is_read, is_write);

    // ── Invariant checks ──────────────────────────────────────────────

    // (a) If Ok, the mount_prefix must match a known prefix at a path boundary.
    if result.is_ok() {
        assert!(
            matches_known_prefix(&mount_prefix),
            "SECURITY: Ok(()) returned for unrecognized mount prefix: {mount_prefix:?}"
        );
    }

    // (b) Regression: "/devious" must NEVER succeed (prefix confusion bug).
    if mount_prefix == "/devious" {
        assert!(
            result.is_err(),
            "SECURITY: /devious must not match /dev — got Ok(())"
        );
    }

    // (c) Any path not matching a known prefix at boundary must be NotFound.
    if !matches_known_prefix(&mount_prefix) {
        assert!(
            matches!(result, Err(VfsError::NotFound)),
            "Expected NotFound for unknown prefix {mount_prefix:?}, got {result:?}"
        );
    }

    // (d) /bundle writes must always be ReadOnly, regardless of capabilities.
    if is_write
        && (mount_prefix == "/bundle"
            || (mount_prefix.starts_with("/bundle/") && mount_prefix.len() > "/bundle".len()))
    {
        assert!(
            result
                .as_ref()
                .is_ok_and(|requirements| requirements.read_only),
            "/bundle is read-only but write returned {result:?} for {mount_prefix:?}"
        );
    }

    // Every selected grant belongs to its access direction; readonly is never
    // represented as an invented grant.
    if let Ok(requirements) = result {
        for capability in requirements.capabilities() {
            assert!(
                (is_read && capability.ends_with(".read"))
                    || (is_write && capability.ends_with(".write"))
            );
            assert_ne!(*capability, "!");
        }
        assert!(requirements.capabilities().len() <= 2);
    }
});
