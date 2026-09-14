use crate::{Capability, ClosureMode, EventJournal, Receipt, ValidatedPolicy};
use std::collections::BTreeMap;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

/// Environment that callers must capture and seal before any supervised launch.
/// Backends must not inject these values after policy identity is established.
pub fn required_environment() -> BTreeMap<String, String> {
    #[cfg(target_os = "windows")]
    return windows::required_environment();
    #[cfg(not(target_os = "windows"))]
    BTreeMap::new()
}

pub fn capability(mode: ClosureMode) -> Capability {
    #[cfg(target_os = "windows")]
    return windows::capability(mode);
    #[cfg(target_os = "linux")]
    return linux::capability(mode);
    #[cfg(target_os = "macos")]
    return unavailable(
        mode,
        "macos",
        "macos-endpoint-security",
        "Endpoint Security entitlement and privileged helper are not available in this binary",
    );
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    return unavailable(
        mode,
        std::env::consts::OS,
        "unsupported",
        "no kernel process-closure backend exists for this platform",
    );
}

pub fn run(policy: &ValidatedPolicy, _events: &mut EventJournal) -> Receipt {
    #[cfg(target_os = "windows")]
    return windows::run(policy, _events);
    #[cfg(target_os = "linux")]
    return linux::run(policy, _events);
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let capability = capability(policy.policy.mode);
        Receipt::rejected(
            policy,
            &capability,
            capability
                .reason
                .clone()
                .unwrap_or_else(|| "kernel backend unavailable".to_owned()),
        )
    }
}

#[allow(dead_code)]
fn unavailable(mode: ClosureMode, platform: &str, backend: &str, reason: &str) -> Capability {
    Capability {
        schema: crate::CAPABILITY_SCHEMA.to_owned(),
        platform: platform.to_owned(),
        mode,
        backend: backend.to_owned(),
        available: false,
        pre_entry_exec_authority: false,
        recursive_descendant_authority: false,
        required_environment: required_environment(),
        reason: Some(reason.to_owned()),
    }
}
