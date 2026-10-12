//! Mount-to-capability mapping for VFS access control.

use crate::VfsError;
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
        io_mode_decode(IO_MODE.load(Ordering::Relaxed))
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

/// Selected mount requirements. Read-only is a constraint, not a capability:
/// callers report all missing grants before applying this constraint.
#[derive(Debug)]
pub struct MountRequirements {
    capabilities: [&'static str; 2],
    count: usize,
    pub read_only: bool,
}

impl MountRequirements {
    pub fn capabilities(&self) -> &[&'static str] {
        &self.capabilities[..self.count]
    }
}

/// Resolve only policy metadata; authorization belongs to the runtime gate.
pub fn mount_requirements(
    mount_prefix: &str,
    readable: bool,
    writable: bool,
) -> Result<MountRequirements, VfsError> {
    let entry = MOUNT_CAPABILITIES.iter().find(|(prefix, _, _)| {
        mount_prefix == *prefix
            || (mount_prefix.len() > prefix.len()
                && mount_prefix.starts_with(prefix)
                && mount_prefix.as_bytes()[prefix.len()] == b'/')
    });
    let Some((_, read_cap, write_cap)) = entry else {
        return Err(VfsError::NotFound);
    };
    let mut requirements = MountRequirements {
        capabilities: [""; 2],
        count: 0,
        read_only: writable && *write_cap == NEVER,
    };
    for (selected, capability) in [(readable, *read_cap), (writable, *write_cap)] {
        if selected && !capability.is_empty() && capability != NEVER {
            requirements.capabilities[requirements.count] = capability;
            requirements.count += 1;
        }
    }
    Ok(requirements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_access_requires_only_its_mount_grants() {
        for (read, write, expected) in [
            (true, false, vec!["fs.tmp.read"]),
            (false, true, vec!["fs.tmp.write"]),
            (true, true, vec!["fs.tmp.read", "fs.tmp.write"]),
        ] {
            for prefix in ["/tmp", "/tmp/child"] {
                let selected = mount_requirements(prefix, read, write).unwrap();
                assert_eq!(selected.capabilities(), expected);
                assert!(!selected.read_only);
            }
        }
        let bundle = mount_requirements("/bundle", true, true).unwrap();
        assert_eq!(bundle.capabilities(), ["fs.bundle.read"]);
        assert!(bundle.read_only);
        let dev = mount_requirements("/dev/null", true, true).unwrap();
        assert!(dev.capabilities().is_empty());
        assert!(!dev.read_only);
    }

    #[test]
    fn policy_requires_mount_path_boundaries() {
        for prefix in ["/devious", "/tmp_private", "/bundle_extra", "/unknown"] {
            assert!(matches!(
                mount_requirements(prefix, true, true),
                Err(VfsError::NotFound)
            ));
        }
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
