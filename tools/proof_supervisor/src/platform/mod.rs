use crate::{CAPABILITY_SCHEMA, Capability, ClosureMode, EventJournal, Receipt, ValidatedPolicy};
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use crate::{KernelAccounting, SupervisorState};
use std::collections::BTreeMap;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use std::time::Instant;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
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
    return macos::capability(mode);
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    return unavailable(
        mode,
        std::env::consts::OS,
        "unsupported",
        "no kernel process-closure backend exists for this platform",
    );
}

pub fn capability_contract_is_valid(recorded: &Capability, mode: ClosureMode) -> bool {
    if recorded.schema != CAPABILITY_SCHEMA || recorded.mode != mode {
        return false;
    }
    #[cfg(target_os = "windows")]
    return recorded == &capability(mode);
    #[cfg(target_os = "linux")]
    return recorded.platform == "linux"
        && recorded.backend == "ptrace-exitkill"
        && recorded.pre_entry_exec_authority
        && recorded.pre_entry_process_create_authority
        && recorded.recursive_descendant_authority
        && recorded.required_environment == required_environment()
        && if recorded.available {
            recorded.reason.is_none()
        } else {
            recorded
                .reason
                .as_ref()
                .is_some_and(|reason| !reason.is_empty())
        };
    #[cfg(target_os = "macos")]
    return macos::capability_contract_is_valid(recorded, mode);
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    return recorded == &capability(mode);
}

pub fn run(
    policy: &ValidatedPolicy,
    _events: &mut EventJournal,
    capability: Capability,
) -> Receipt {
    #[cfg(target_os = "windows")]
    return windows::run(policy, _events, capability);
    #[cfg(target_os = "linux")]
    return linux::run(policy, _events, capability);
    #[cfg(target_os = "macos")]
    return macos::run(policy, _events, capability);
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
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
        schema: CAPABILITY_SCHEMA.to_owned(),
        platform: platform.to_owned(),
        mode,
        backend: backend.to_owned(),
        available: false,
        pre_entry_exec_authority: false,
        pre_entry_process_create_authority: false,
        recursive_descendant_authority: false,
        required_environment: required_environment(),
        reason: Some(reason.to_owned()),
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn run_backend(
    policy: &ValidatedPolicy,
    events: &mut EventJournal,
    capability: Capability,
    supervise: impl FnOnce(
        &ValidatedPolicy,
        &mut EventJournal,
    ) -> Result<Option<KernelAccounting>, String>,
) -> Receipt {
    if !capability.available {
        return Receipt::rejected(
            policy,
            &capability,
            capability
                .reason
                .clone()
                .unwrap_or_else(|| "kernel backend unavailable".to_owned()),
        );
    }
    let started = Instant::now();
    let mut receipt = Receipt::running(policy, &capability);
    match supervise(policy, events) {
        Ok(kernel_accounting) => {
            receipt.kernel_accounting = kernel_accounting;
            receipt
                .transition(SupervisorState::Draining)
                .expect("valid drain transition");
        }
        Err(error) => receipt.record_error(error),
    }
    match events.verified() {
        Ok(verified) => receipt.apply_verified_event_log(verified),
        Err(error) => receipt.record_error(error),
    }
    if receipt.accounting.root_execs == 0 {
        receipt.record_error("root executable never reached an admitted image event");
    }
    receipt.elapsed_ns = started.elapsed().as_nanos();
    let complete = receipt.error_count == 0
        && receipt.violation_count == 0
        && receipt.accounting.active_processes == 0
        && receipt.accounting.root_execs >= 1
        && receipt.root_exit_code.is_some()
        && receipt.accounting.process_creates == receipt.accounting.process_exits
        && receipt.kernel_accounting_supports_complete();
    receipt.finish(complete);
    receipt
}
