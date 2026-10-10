use crate::{
    Admission, CAPABILITY_SCHEMA, Capability, ClosureMode, EventJournal, Receipt, ValidatedPolicy,
};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::{BackendFailure, SupervisorState};
use std::collections::BTreeMap;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::time::Instant;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(all(test, target_os = "linux"))]
pub(crate) fn linux_test_custody() -> std::sync::MutexGuard<'static, ()> {
    linux::TEST_WAIT_CUSTODY.lock().unwrap()
}

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
    return ineligible(
        mode,
        std::env::consts::OS,
        "unsupported",
        "no kernel process-closure backend exists for this platform",
    );
}

pub fn capability_contract_is_valid(recorded: &Capability, mode: ClosureMode) -> bool {
    if recorded.schema != CAPABILITY_SCHEMA
        || recorded.mode != mode
        || !recorded.admission.is_well_formed()
    {
        return false;
    }
    #[cfg(target_os = "windows")]
    return planned_contract_is_valid(recorded, &capability(mode));
    #[cfg(target_os = "linux")]
    return recorded_linux_capability_contract_is_valid(recorded, mode);
    #[cfg(target_os = "macos")]
    return macos::capability_contract_is_valid(recorded, mode);
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    return planned_contract_is_valid(recorded, &capability(mode));
}

/// The recorded Linux contract is independent of the verifier host. This
/// authenticates its shape, not the container provider or historical kernel.
/// Native Linux and rooted offline verification use the same predicate.
pub fn recorded_linux_capability_contract_is_valid(
    recorded: &Capability,
    mode: ClosureMode,
) -> bool {
    recorded.schema == CAPABILITY_SCHEMA
        && recorded.mode == mode
        && recorded.platform == "linux"
        && recorded.backend == "ptrace-exitkill"
        && recorded.pre_entry_exec_authority
        && recorded.pre_entry_process_create_authority
        && recorded.recursive_descendant_authority
        && recorded.required_environment.is_empty()
        && recorded.admission.is_well_formed()
}

/// Compare immutable backend facts without treating today's plan as evidence of
/// a historical launch. Fixed refused plans cannot produce admitted receipts.
#[allow(dead_code)]
pub(super) fn planned_contract_is_valid(recorded: &Capability, planned: &Capability) -> bool {
    let mut contract = recorded.clone();
    contract.admission = planned.admission.clone();
    contract == *planned
        && recorded.admission.is_well_formed()
        && match &planned.admission {
            Admission::Ineligible { .. } => recorded.admission == planned.admission,
            Admission::Eligible {} => !matches!(recorded.admission, Admission::Ineligible { .. }),
            Admission::Admitted { .. } => false,
        }
}

pub fn run(
    policy: &ValidatedPolicy,
    _events: &mut EventJournal,
    capability: Capability,
) -> Receipt {
    if matches!(
        policy.path_namespace,
        crate::PolicyPathNamespace::LinuxGuest
    ) {
        return Receipt::rejected(
            policy,
            &capability,
            "retained Linux policy is an offline verification input, not a launch authority",
        );
    }
    #[cfg(target_os = "windows")]
    return windows::run(policy, _events, capability);
    #[cfg(target_os = "linux")]
    return linux::run(policy, _events, capability);
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let Admission::Ineligible { reason } = &capability.admission else {
            unreachable!("platform without an admitted executor must refuse its plan");
        };
        Receipt::rejected(policy, &capability, reason.clone())
    }
}

#[allow(dead_code)]
fn ineligible(mode: ClosureMode, platform: &str, backend: &str, reason: &str) -> Capability {
    Capability {
        schema: CAPABILITY_SCHEMA.to_owned(),
        platform: platform.to_owned(),
        mode,
        backend: backend.to_owned(),
        admission: Admission::Ineligible {
            reason: reason.to_owned(),
        },
        pre_entry_exec_authority: false,
        pre_entry_process_create_authority: false,
        recursive_descendant_authority: false,
        required_environment: required_environment(),
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn run_backend(
    policy: &ValidatedPolicy,
    events: &mut EventJournal,
    capability: Capability,
    supervise: impl FnOnce(
        &ValidatedPolicy,
        &mut EventJournal,
    ) -> Result<crate::NativeCustody, BackendFailure>,
) -> Receipt {
    if let Admission::Ineligible { reason } = &capability.admission {
        return Receipt::rejected(policy, &capability, reason.clone());
    }
    assert_eq!(
        capability.admission,
        Admission::Eligible {},
        "backend requires a prelaunch plan"
    );
    let started = Instant::now();
    let mut receipt = Receipt::running(policy, &capability);
    match supervise(policy, events) {
        Ok(native_custody) => {
            receipt.native_custody = native_custody;
            receipt
                .transition(SupervisorState::Draining)
                .expect("valid drain transition");
        }
        Err(failure) => {
            receipt.record_error(failure.cause);
            for diagnostic in failure.cleanup {
                receipt.record_error(diagnostic);
            }
            receipt.native_custody = failure.native_custody;
        }
    }
    receipt.journal_coverage = events.coverage().clone();
    match events.verified() {
        Ok(verified) => receipt.apply_verified_event_log(verified),
        Err(error) => receipt.record_error(error),
    }
    if receipt.accounting.root_execs == 0 {
        receipt.record_error("root executable never reached an admitted image event");
    }
    receipt.elapsed_ns = started.elapsed().as_nanos();
    let complete = receipt.error_count == 0
        && matches!(receipt.capability.admission, Admission::Admitted { .. })
        && receipt.violation_count == 0
        && receipt.accounting.active_processes == 0
        && receipt.accounting.root_execs >= 1
        && receipt.root_exit_code.is_some()
        && receipt.accounting.process_creates == receipt.accounting.process_exits
        && receipt.native_custody_supports_complete();
    receipt.finish(complete);
    receipt
}
