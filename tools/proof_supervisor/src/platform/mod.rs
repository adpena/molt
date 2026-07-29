use crate::{Capability, ClosureMode, EventJournal, Receipt, ValidatedPolicy};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

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

pub fn run(policy: &ValidatedPolicy, events: &mut EventJournal) -> Receipt {
    #[cfg(target_os = "windows")]
    return windows::run(policy, events);
    #[cfg(target_os = "linux")]
    return linux::run(policy, events);
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
        schema: "molt.proof-supervisor-capability.v1".to_owned(),
        platform: platform.to_owned(),
        mode,
        backend: backend.to_owned(),
        available: false,
        pre_entry_exec_authority: false,
        recursive_descendant_authority: false,
        reason: Some(reason.to_owned()),
    }
}
