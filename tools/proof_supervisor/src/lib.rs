//! Kernel-backed process-image custody for Molt proof execution.
//!
//! This crate is intentionally workspace-neutral and protocol-first.  The
//! caller seals one policy before launch; the platform backend owns every
//! process event until the tree is quiescent and returns one terminal receipt.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/protocol.rs"));

// The transport bound is declared in protocol.json for Rust and Python.
const _: () = assert!(EXPORT_RECEIPT_MAX_BYTES == evidence::MAX_RECEIPT_BYTES);

pub mod budget;
pub mod evidence;
pub mod image_cache;
pub mod platform;
mod process_ledger;

pub use evidence::{
    ArtifactSummary, EventJournal, IdentitySummary, PublishedEvidence, VerifiedEventLog,
};
pub use image_cache::{ImageCacheKey, ImageHashCache};
pub use process_ledger::RecordOutcome;

const MAX_DIAGNOSTICS_PER_CLASS: usize = BUDGET_DIAGNOSTICS_PER_CLASS;
// Keep both full diagnostic classes inside 48 KiB, leaving room for the sealed
// capability, lifecycle, accounting and JSON framing in the 64 KiB receipt.
const MAX_DIAGNOSTIC_BYTES: usize =
    BUDGET_COMBINED_DIAGNOSTICS_JSON_BYTES / (2 * MAX_DIAGNOSTICS_PER_CLASS);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClosureMode {
    Leaf,
    DeclaredTree,
    InventoryTree,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RootExitDisposition {
    #[default]
    RequireExit,
    Terminate,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixedImage {
    pub role: String,
    pub path: PathBuf,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "RootExitDisposition::is_require_exit")]
    pub root_exit_disposition: RootExitDisposition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixedAuthority {
    pub path: PathBuf,
    pub sha256: String,
    pub roles: BTreeSet<String>,
    pub root_exit_disposition: RootExitDisposition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedRoot {
    pub role: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema: String,
    pub nonce: String,
    pub mode: ClosureMode,
    pub cwd: PathBuf,
    pub command: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub root_role: String,
    pub fixed_images: Vec<FixedImage>,
    #[serde(default)]
    pub derived_roots: Vec<DerivedRoot>,
}

#[derive(Clone, Debug)]
pub struct ValidatedPolicy {
    pub policy: Policy,
    pub policy_sha256: String,
    pub root_path: PathBuf,
    pub fixed: BTreeMap<PathBuf, FixedAuthority>,
    pub derived: Vec<DerivedRoot>,
    path_namespace: PolicyPathNamespace,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageClass {
    Fixed,
    Derived,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SupervisorState {
    Created,
    PolicySealed,
    Running,
    Draining,
    Complete,
    Rejected,
    Incomplete,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub path: PathBuf,
    pub file_id: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub class: ImageClass,
    pub roles: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessEvent {
    pub sequence: u64,
    pub process_id: u32,
    pub stable_process_id: String,
    pub event: ProcessEventKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProcessEventKind {
    ProcessCreate {
        parent_process_id: Option<u32>,
    },
    ProcessExit {
        exit_code: i64,
    },
    Fork {
        parent_process_id: u32,
        image: Option<FileIdentity>,
    },
    Exec {
        image: FileIdentity,
    },
    InitialImage {
        image: FileIdentity,
    },
    CloneUnclassified {
        parent_process_id: u32,
        reason: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Accounting {
    pub active_processes: u64,
    pub process_creates: u64,
    pub process_exits: u64,
    pub execs: u64,
    pub root_execs: u64,
    pub root_exit_terminated_processes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
pub enum KernelAccounting {
    WindowsJob {
        total_processes: u64,
        active_processes: u64,
        completion_port_new_processes: u64,
        completion_port_exits: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureStage {
    NativeObservation,
    JournalPreparation,
    JournalAppend,
}

/// Accepted event coverage and actual kernel custody are independent facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum JournalCoverage {
    Full {},
    Prefix {
        stage: CaptureStage,
        next_sequence: u64,
        accepted_records: u64,
        accepted_bytes: u64,
        accepted_sha256: String,
        cause: String,
    },
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NativeCustody {
    NotCreated {},
    Linux {
        remaining_tasks: u64,
        remaining_processes: u64,
        wait_exhausted: bool,
        root_exit_code: Option<i64>,
    },
    Windows {
        root_in_job: bool,
        root_exit_code: Option<i64>,
        debug_root_exit_code: Option<i64>,
        remaining_processes: u64,
        observed_creates: u64,
        observed_exits: u64,
        job_totals_reconciled: bool,
        pending_debug_stop: bool,
        job: Option<KernelAccounting>,
    },
}
impl NativeCustody {
    pub fn is_closed(&self) -> bool {
        match self {
            Self::NotCreated {} => false,
            Self::Linux {
                remaining_tasks,
                remaining_processes,
                wait_exhausted,
                root_exit_code,
            } => {
                *remaining_tasks == 0
                    && *remaining_processes == 0
                    && *wait_exhausted
                    && root_exit_code.is_some()
            }
            Self::Windows {
                root_exit_code,
                debug_root_exit_code,
                remaining_processes,
                pending_debug_stop,
                job,
                ..
            } => {
                root_exit_code.is_some()
                    && root_exit_code == debug_root_exit_code
                    && *remaining_processes == 0
                    && !pending_debug_stop
                    && matches!(
                        job,
                        Some(KernelAccounting::WindowsJob {
                            active_processes: 0,
                            ..
                        })
                    )
            }
        }
    }
}

impl RootExitDisposition {
    fn is_require_exit(&self) -> bool {
        *self == Self::RequireExit
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub schema: String,
    pub platform: String,
    pub mode: ClosureMode,
    pub backend: String,
    pub admission: Admission,
    pub pre_entry_exec_authority: bool,
    pub pre_entry_process_create_authority: bool,
    pub recursive_descendant_authority: bool,
    pub required_environment: BTreeMap<String, String>,
}

/// A plan permits only an attempt. The event ledger alone derives the admitted
/// state from the accepted root creation and its initial policy-validated image.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Admission {
    Ineligible {
        reason: String,
    },
    Eligible {},
    Admitted {
        root_stable_process_id: String,
        root_create_sequence: u64,
        initial_image_sequence: u64,
    },
}

impl Admission {
    pub fn is_well_formed(&self) -> bool {
        match self {
            Self::Ineligible { reason } => {
                !reason.trim().is_empty() && reason.len() <= MAX_DIAGNOSTIC_BYTES
            }
            Self::Eligible {} => true,
            Self::Admitted {
                root_stable_process_id,
                root_create_sequence,
                initial_image_sequence,
            } => {
                !root_stable_process_id.is_empty()
                    && root_stable_process_id.len() <= BUDGET_STABLE_PROCESS_ID_UTF8_BYTES
                    && *root_create_sequence > 0
                    && root_create_sequence < initial_image_sequence
                    && *initial_image_sequence <= evidence::MAX_EVENT_RECORDS
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: String,
    pub capability: Capability,
    pub policy_sha256: String,
    pub nonce_sha256: String,
    pub state: SupervisorState,
    pub lifecycle: Vec<SupervisorState>,
    pub event_log: Option<ArtifactSummary>,
    pub derived_image_summary: IdentitySummary,
    pub journal_coverage: JournalCoverage,
    pub accounting: Accounting,
    pub native_custody: NativeCustody,
    pub violation_count: u64,
    pub violations: Vec<String>,
    pub error_count: u64,
    pub errors: Vec<String>,
    pub root_exit_code: Option<i64>,
    pub elapsed_ns: u128,
    pub complete: bool,
    pub identity_sha256: String,
}

/// A backend failure and its independent cleanup observations. None of these
/// observations can substitute for an event that the journal did not accept.
#[derive(Debug)]
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) struct BackendFailure {
    pub cause: String,
    pub cleanup: Vec<String>,
    pub native_custody: NativeCustody,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl From<String> for BackendFailure {
    fn from(cause: String) -> Self {
        Self {
            cause,
            cleanup: Vec::new(),
            native_custody: NativeCustody::NotCreated {},
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl BackendFailure {
    pub(crate) fn retain_cleanup(&mut self, diagnostic: String) {
        // Reserve one of the receipt's bounded slots for the original cause.
        if self.cleanup.len() < MAX_DIAGNOSTICS_PER_CLASS - 1 {
            push_bounded_diagnostic(&mut self.cleanup, diagnostic);
        }
    }
}

/// Bounded diagnostic custody for actual terminal observations even when the
/// journal is unusable. The count and digest cover every observation; a small
/// sample remains readable. This is never a replacement event-log witness.
#[derive(Default)]
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(crate) struct TerminalObservations {
    count: u64,
    digest: Sha256,
    sample: [Option<(u32, i64)>; 8],
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl TerminalObservations {
    pub(crate) fn observe(&mut self, process_id: u32, status: i64) {
        self.count += 1;
        self.digest.update(process_id.to_be_bytes());
        self.digest.update(status.to_be_bytes());
        if let Some(slot) = self.sample.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some((process_id, status));
        }
    }

    pub(crate) fn summary(&self, kind: &str) -> String {
        format!(
            "cleanup terminal {kind} [{}]; count={}; sha256={}",
            self.sample
                .iter()
                .flatten()
                .map(|(pid, status)| format!("{pid}:{status:#x}"))
                .collect::<Vec<_>>()
                .join(", "),
            self.count,
            hex_lower(&self.digest.clone().finalize())
        )
    }
}

impl Receipt {
    pub fn running(policy: &ValidatedPolicy, capability: &Capability) -> Self {
        let mut receipt = Self {
            schema: RECEIPT_SCHEMA.to_owned(),
            capability: capability.clone(),
            policy_sha256: policy.policy_sha256.clone(),
            nonce_sha256: sha256_bytes(policy.policy.nonce.as_bytes()),
            state: SupervisorState::Created,
            lifecycle: vec![SupervisorState::Created],
            event_log: None,
            derived_image_summary: IdentitySummary::empty(),
            journal_coverage: JournalCoverage::Full {},
            accounting: Accounting::default(),
            native_custody: NativeCustody::NotCreated {},
            violation_count: 0,
            violations: Vec::new(),
            error_count: 0,
            errors: Vec::new(),
            root_exit_code: None,
            elapsed_ns: 0,
            complete: false,
            identity_sha256: String::new(),
        };
        receipt
            .transition(SupervisorState::PolicySealed)
            .expect("valid initial transition");
        receipt
            .transition(SupervisorState::Running)
            .expect("valid initial transition");
        receipt
    }

    pub fn rejected(
        policy: &ValidatedPolicy,
        capability: &Capability,
        reason: impl AsRef<str>,
    ) -> Self {
        let mut receipt = Self {
            schema: RECEIPT_SCHEMA.to_owned(),
            capability: capability.clone(),
            policy_sha256: policy.policy_sha256.clone(),
            nonce_sha256: sha256_bytes(policy.policy.nonce.as_bytes()),
            state: SupervisorState::Created,
            lifecycle: vec![SupervisorState::Created],
            event_log: None,
            derived_image_summary: IdentitySummary::empty(),
            journal_coverage: JournalCoverage::Full {},
            accounting: Accounting::default(),
            native_custody: NativeCustody::NotCreated {},
            violation_count: 0,
            violations: Vec::new(),
            error_count: 0,
            errors: Vec::new(),
            root_exit_code: None,
            elapsed_ns: 0,
            complete: false,
            identity_sha256: String::new(),
        };
        receipt
            .transition(SupervisorState::PolicySealed)
            .expect("valid initial transition");
        receipt
            .transition(SupervisorState::Rejected)
            .expect("valid rejection transition");
        receipt.record_error(reason);
        receipt.seal();
        receipt
    }

    pub fn transition(&mut self, next: SupervisorState) -> Result<(), String> {
        if !valid_transition(self.state, next) {
            return Err(format!(
                "invalid supervisor transition {:?} -> {next:?}",
                self.state
            ));
        }
        self.state = next;
        self.lifecycle.push(next);
        Ok(())
    }

    pub fn finish(&mut self, complete: bool) {
        let terminal = if complete {
            SupervisorState::Complete
        } else {
            SupervisorState::Incomplete
        };
        self.transition(terminal)
            .expect("platform must finish from running or draining");
        self.complete = complete;
        self.seal();
    }

    pub fn apply_verified_event_log(&mut self, verified: &VerifiedEventLog) {
        self.capability.admission = verified.admission.clone();
        self.derived_image_summary = verified.derived_images.clone();
        self.accounting = verified.accounting.clone();
        self.root_exit_code = verified.root_exit_code;
        self.violation_count = verified.violation_count;
        self.violations.clone_from(&verified.violations);
    }

    pub fn attach_evidence(&mut self, evidence: PublishedEvidence) -> Result<(), String> {
        if self.capability.admission != evidence.verified.admission
            || self.derived_image_summary != evidence.verified.derived_images
            || self.accounting != evidence.verified.accounting
            || self.root_exit_code != evidence.verified.root_exit_code
            || self.violation_count != evidence.verified.violation_count
            || self.violations != evidence.verified.violations
        {
            return Err("published event replay disagrees with terminal receipt".to_owned());
        }
        self.event_log = Some(evidence.event_log);
        self.seal();
        Ok(())
    }

    pub fn record_violation(&mut self, value: impl AsRef<str>) {
        self.violation_count = self.violation_count.saturating_add(1);
        push_bounded_diagnostic(&mut self.violations, value);
    }

    pub fn record_error(&mut self, value: impl AsRef<str>) {
        self.error_count = self.error_count.saturating_add(1);
        push_bounded_diagnostic(&mut self.errors, value);
    }

    /// Preserve the already bounded execution and actual cleanup diagnostics
    /// when publication cannot produce a receipt. This is a stderr diagnostic,
    /// never a replacement receipt or a publication acknowledgement. Keeping
    /// the publication cause outside `errors` also preserves it when that
    /// existing bounded list is full.
    pub fn publication_failure_diagnostic(&self, cause: &str) -> String {
        let snapshot = budget::encode(self, BUDGET_RECEIPT_BYTES)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| "receipt exceeds publication byte budget".to_owned());
        format!(
            "terminal publication not acknowledged: {cause}; terminal receipt snapshot (diagnostic only): {snapshot}"
        )
    }

    pub fn seal(&mut self) {
        self.identity_sha256.clear();
        self.identity_sha256 =
            budget::digest(self, usize::MAX).expect("receipt hash serialization is infallible");
    }

    pub fn identity_is_valid(&self) -> bool {
        let expected = self.identity_sha256.as_bytes();
        let mut material = self.clone();
        material.identity_sha256.clear();
        budget::digest(&material, BUDGET_RECEIPT_BYTES)
            .is_ok_and(|digest| constant_time_eq(expected, digest.as_bytes()))
    }

    pub fn terminal_is_consistent(&self) -> bool {
        let terminal = matches!(
            (self.state, self.complete),
            (SupervisorState::Complete, true)
                | (SupervisorState::Incomplete, false)
                | (SupervisorState::Rejected, false)
        );
        let diagnostics_consistent = self.violation_count >= self.violations.len() as u64
            && self.error_count >= self.errors.len() as u64;
        let admission_consistent = self.capability.admission.is_well_formed()
            && match &self.capability.admission {
                Admission::Ineligible { .. } => {
                    self.state == SupervisorState::Rejected
                        && self.accounting == Accounting::default()
                        && self.root_exit_code.is_none()
                }
                Admission::Eligible {} => {
                    self.state == SupervisorState::Incomplete && self.accounting.root_execs == 0
                }
                Admission::Admitted { .. } => {
                    self.state != SupervisorState::Rejected && self.accounting.root_execs > 0
                }
            };
        let complete_consistent = !self.complete
            || (self.violation_count == 0
                && self.error_count == 0
                && matches!(self.capability.admission, Admission::Admitted { .. })
                && self.capability.pre_entry_exec_authority
                && self.capability.recursive_descendant_authority
                && (self.capability.mode != ClosureMode::Leaf
                    || self.capability.pre_entry_process_create_authority)
                && self.accounting.active_processes == 0
                && self.accounting.root_execs >= 1
                && self.root_exit_code.is_some()
                && self.accounting.process_creates == self.accounting.process_exits
                && self.native_custody_supports_complete());
        terminal
            && diagnostics_consistent
            && admission_consistent
            && complete_consistent
            && self.coverage_is_valid()
            && self.native_custody_is_valid()
            && self.lifecycle_is_valid()
            && self.event_log.is_some()
    }

    pub fn lifecycle_is_valid(&self) -> bool {
        if self.lifecycle.first() != Some(&SupervisorState::Created) {
            return false;
        }
        let mut current = SupervisorState::Created;
        for next in self.lifecycle.iter().copied().skip(1) {
            if !valid_transition(current, next) {
                return false;
            }
            current = next;
        }
        current == self.state
    }

    pub fn coverage_is_valid(&self) -> bool {
        match (&self.journal_coverage, &self.event_log) {
            (JournalCoverage::Full {}, _) => true,
            (
                JournalCoverage::Prefix {
                    stage: _,
                    next_sequence,
                    accepted_records,
                    accepted_bytes,
                    accepted_sha256,
                    cause,
                },
                Some(log),
            ) => {
                !self.complete
                    && self.state == SupervisorState::Incomplete
                    && self.error_count > 0
                    && !cause.is_empty()
                    && cause.len() <= MAX_DIAGNOSTIC_BYTES
                    && accepted_records.checked_add(1) == Some(*next_sequence)
                    && *accepted_records == log.count
                    && *accepted_bytes == log.bytes
                    && accepted_sha256 == &log.sha256
            }
            _ => false,
        }
    }
    pub fn native_custody_is_valid(&self) -> bool {
        let full = matches!(self.journal_coverage, JournalCoverage::Full {});
        match &self.native_custody {
            NativeCustody::NotCreated {} => self.accounting.process_creates == 0 && !self.complete,
            NativeCustody::Linux {
                remaining_tasks,
                remaining_processes,
                root_exit_code,
                ..
            } => {
                self.capability.platform == "linux"
                    && remaining_processes <= remaining_tasks
                    && (!full
                        || (*remaining_processes == self.accounting.active_processes
                            && self.root_exit_code == *root_exit_code))
            }
            NativeCustody::Windows {
                root_in_job,
                root_exit_code,
                debug_root_exit_code,
                job,
                observed_creates,
                observed_exits,
                remaining_processes,
                job_totals_reconciled,
                ..
            } => {
                if self.capability.platform != "windows"
                    || observed_exits.checked_add(*remaining_processes) != Some(*observed_creates)
                {
                    return false;
                }
                if root_exit_code.is_some()
                    && debug_root_exit_code.is_some()
                    && root_exit_code != debug_root_exit_code
                {
                    return false;
                }
                if full
                    && (*observed_creates != self.accounting.process_creates
                        || *observed_exits != self.accounting.process_exits
                        || *remaining_processes != self.accounting.active_processes
                        || *debug_root_exit_code != self.root_exit_code)
                {
                    return false;
                }
                // Raw Job counters can include failed associations. Preserve
                // them and the explicit mismatch; do not invent omitted events.
                let expected_job_creates = observed_creates.checked_sub(u64::from(!root_in_job));
                let reconciled = matches!(job, Some(KernelAccounting::WindowsJob { total_processes, active_processes, .. })
                    if Some(*total_processes) == expected_job_creates && *active_processes == *remaining_processes);
                if reconciled != *job_totals_reconciled {
                    return false;
                }
                if !reconciled && self.error_count == 0 {
                    return false;
                }
                !self.complete || (*root_in_job && reconciled && full)
            }
        }
    }
    pub(crate) fn native_custody_supports_complete(&self) -> bool {
        matches!(self.journal_coverage, JournalCoverage::Full {})
            && self.native_custody.is_closed()
            && self.native_custody_is_valid()
    }
}

/// Format only the admitted escaped-JSON prefix. The formatter stops at the
/// bound instead of allocating a full caller-controlled message then truncating.
/// This constructor is fallible so journal preparation can refuse before append.
pub(crate) fn bounded_diagnostic(arguments: std::fmt::Arguments<'_>) -> Result<String, String> {
    struct Buffer {
        value: String,
        wire_bytes: usize,
        ellipsis_boundary: usize,
        truncated: bool,
    }
    impl std::fmt::Write for Buffer {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            if self.truncated {
                return Err(std::fmt::Error);
            }
            for character in text.chars() {
                let cost = match character {
                    '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
                    '\u{0}'..='\u{1f}' => 6,
                    _ => character.len_utf8(),
                };
                if self.wire_bytes + cost > MAX_DIAGNOSTIC_BYTES {
                    self.value.truncate(self.ellipsis_boundary);
                    self.value.push_str("...");
                    self.truncated = true;
                    return Err(std::fmt::Error);
                }
                self.value.push(character);
                self.wire_bytes += cost;
                if self.wire_bytes <= MAX_DIAGNOSTIC_BYTES - 3 {
                    self.ellipsis_boundary = self.value.len();
                }
            }
            Ok(())
        }
    }
    let mut value = String::new();
    value
        .try_reserve_exact(MAX_DIAGNOSTIC_BYTES)
        .map_err(|_| "diagnostic storage reservation refused".to_owned())?;
    let mut buffer = Buffer {
        value,
        wire_bytes: 2,
        ellipsis_boundary: 0,
        truncated: false,
    };
    if std::fmt::write(&mut buffer, arguments).is_err() && !buffer.truncated {
        return Err("diagnostic formatting refused".to_owned());
    }
    Ok(buffer.value)
}

pub(crate) fn push_bounded_diagnostic(values: &mut Vec<String>, value: impl AsRef<str>) {
    if values.len() >= MAX_DIAGNOSTICS_PER_CLASS {
        return;
    }
    // Receipt/cleanup counters already retain the failed observation. If the
    // diagnostic allocation itself refuses, retain that bounded refusal rather
    // than an unbounded original allocation or an invented successful message.
    let bounded =
        bounded_diagnostic(format_args!("{}", value.as_ref())).unwrap_or_else(|refusal| refusal);
    values.push(bounded);
}

fn valid_transition(current: SupervisorState, next: SupervisorState) -> bool {
    matches!(
        (current, next),
        (SupervisorState::Created, SupervisorState::PolicySealed)
            | (SupervisorState::PolicySealed, SupervisorState::Running)
            | (SupervisorState::PolicySealed, SupervisorState::Rejected)
            | (SupervisorState::Running, SupervisorState::Draining)
            | (SupervisorState::Running, SupervisorState::Incomplete)
            | (SupervisorState::Draining, SupervisorState::Complete)
            | (SupervisorState::Draining, SupervisorState::Incomplete)
    )
}

#[derive(Clone, Copy, Debug)]
enum PolicyPathNamespace {
    Host,
    LinuxGuest,
}

/// A retained Linux root is an input to offline verification, never a launch
/// authority. The caller owns immutable custody and admission of the complete
/// root; this resolver checks only the policy's directories and fixed images.
enum PolicyPaths {
    Host,
    RetainedLinuxRoot(PathBuf),
}

impl PolicyPaths {
    fn namespace(&self) -> PolicyPathNamespace {
        match self {
            Self::Host => PolicyPathNamespace::Host,
            Self::RetainedLinuxRoot(_) => PolicyPathNamespace::LinuxGuest,
        }
    }

    fn required_environment(&self) -> BTreeMap<String, String> {
        match self {
            Self::Host => platform::required_environment(),
            Self::RetainedLinuxRoot(_) => BTreeMap::new(),
        }
    }

    fn is_absolute(&self, path: &Path) -> bool {
        match self {
            Self::Host => path.is_absolute(),
            Self::RetainedLinuxRoot(_) => linux_guest_components(path).is_ok(),
        }
    }

    fn resolve(
        &self,
        path: &Path,
        label: &str,
        directory: bool,
    ) -> Result<(PathBuf, PathBuf), String> {
        match self {
            Self::Host => {
                let canonical = if directory {
                    canonical_directory(path, label)?
                } else {
                    canonical_file(path, label)?
                };
                Ok((canonical.clone(), canonical))
            }
            Self::RetainedLinuxRoot(root) => {
                let components = linux_guest_components(path)?;
                let mut retained = root.clone();
                for component in components {
                    // Match the actual entry spelling even on case-insensitive
                    // verifier filesystems. Guest Linux names remain exact.
                    let mut found = false;
                    for entry in std::fs::read_dir(&retained).map_err(|error| {
                        format!(
                            "cannot read retained directory {}: {error}",
                            retained.display()
                        )
                    })? {
                        let entry = entry
                            .map_err(|error| format!("cannot read retained entry: {error}"))?;
                        if entry.file_name().as_os_str() == std::ffi::OsStr::new(component) {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Err(format!(
                            "retained {label} has no exact entry: {}",
                            path.display()
                        ));
                    }
                    retained.push(component);
                    let metadata = std::fs::symlink_metadata(&retained).map_err(|error| {
                        format!(
                            "cannot inspect retained {label} {}: {error}",
                            retained.display()
                        )
                    })?;
                    if redirecting_metadata(&metadata) {
                        return Err(format!(
                            "retained {label} contains a symlink or reparse point: {}",
                            path.display()
                        ));
                    }
                }
                let metadata = std::fs::symlink_metadata(&retained)
                    .map_err(|error| format!("cannot inspect retained {label}: {error}"))?;
                if redirecting_metadata(&metadata)
                    || (directory && !metadata.is_dir())
                    || (!directory && !metadata.is_file())
                {
                    return Err(format!(
                        "retained {label} has an invalid file type: {}",
                        path.display()
                    ));
                }
                let canonical = dunce::canonicalize(&retained)
                    .map_err(|error| format!("cannot resolve retained {label}: {error}"))?;
                if !canonical.starts_with(root) {
                    return Err(format!("retained {label} escaped root: {}", path.display()));
                }
                Ok((path.to_path_buf(), canonical))
            }
        }
    }
}

fn redirecting_metadata(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Canonical, portable spelling of an absolute Linux guest path. Do not use
/// the verifier host's Path::is_absolute or normalize away guest path escapes.
fn linux_guest_components(path: &Path) -> Result<Vec<&str>, String> {
    let text = path
        .to_str()
        .ok_or_else(|| "Linux guest path must be UTF-8".to_owned())?;
    if text == "/" {
        return Ok(Vec::new());
    }
    if !text.starts_with('/') || text.contains(['\\', ':', '\0']) {
        return Err("Linux guest path must be an absolute portable path".to_owned());
    }
    let components: Vec<_> = text[1..].split('/').collect();
    if components
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == ".." || part.ends_with(['.', ' ']))
    {
        return Err("Linux guest path must not contain aliases or traversal components".to_owned());
    }
    Ok(components)
}

impl Policy {
    pub fn validate(self) -> Result<ValidatedPolicy, String> {
        self.validate_paths(PolicyPaths::Host)
    }

    /// Verify fixed-image policy inputs in a retained Linux execution root.
    /// Logical guest paths and the policy digest are preserved. This does not
    /// authenticate root provenance, prove isolation, or authorize execution.
    pub fn validate_rooted_linux(self, rootfs: &Path) -> Result<ValidatedPolicy, String> {
        if self.mode == ClosureMode::InventoryTree || !self.derived_roots.is_empty() {
            return Err(
                "rooted Linux verification requires fixed-only leaf or declared-tree policy"
                    .to_owned(),
            );
        }
        let metadata = std::fs::symlink_metadata(rootfs)
            .map_err(|error| format!("cannot inspect retained root: {error}"))?;
        if redirecting_metadata(&metadata) || !metadata.is_dir() {
            return Err(
                "retained root must be a directory, not a symlink or reparse point".to_owned(),
            );
        }
        let root = canonical_directory(rootfs, "retained root")?;
        self.validate_paths(PolicyPaths::RetainedLinuxRoot(root))
    }

    fn validate_paths(mut self, paths: PolicyPaths) -> Result<ValidatedPolicy, String> {
        budget::policy_shape(&self)?;
        if self.schema != POLICY_SCHEMA {
            return Err(format!("policy schema must be {POLICY_SCHEMA}"));
        }
        if self.nonce.len() < 32 || !self.nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(
                "policy nonce must contain at least 128 bits of hexadecimal entropy".to_owned(),
            );
        }
        if self.command.is_empty() || self.command[0].is_empty() {
            return Err("policy command must name an executable".to_owned());
        }
        if self.command.iter().any(|argument| argument.contains('\0')) {
            return Err("policy command arguments cannot contain NUL".to_owned());
        }
        if !paths.is_absolute(&self.cwd) || !paths.is_absolute(Path::new(&self.command[0])) {
            return Err("policy cwd and root command must be absolute".to_owned());
        }
        let mut environment_keys = BTreeSet::new();
        for (key, value) in &self.environment {
            if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                return Err("policy environment contains an invalid name or NUL".to_owned());
            }
            if !environment_keys.insert(key.to_ascii_lowercase()) {
                return Err("policy environment keys must be unique ignoring ASCII case".to_owned());
            }
        }
        for (key, value) in paths.required_environment() {
            match self.environment.get(&key) {
                None => {
                    return Err(format!(
                        "policy environment requires canonical key {key}={value}"
                    ));
                }
                Some(actual) if actual != &value => {
                    return Err(format!("policy environment requires {key}={value}"));
                }
                Some(_) => {}
            }
        }
        if self.root_role.is_empty() {
            return Err("policy root_role must be non-empty".to_owned());
        }
        let (cwd, _) = paths.resolve(&self.cwd, "policy cwd", true)?;
        budget::path_bound(&cwd)?;
        let mut canonical_bytes = budget::encoded_size(&self, BUDGET_CANONICAL_POLICY_BYTES)?
            - budget::encoded_size(&self.fixed_images, BUDGET_CANONICAL_POLICY_BYTES)?
            - budget::encoded_size(&self.derived_roots, BUDGET_CANONICAL_POLICY_BYTES)?
            + 4;
        let mut normalized_images = Vec::new();
        normalized_images
            .try_reserve_exact(self.fixed_images.len())
            .map_err(|_| "fixed image row reservation refused")?;
        for image in std::mem::take(&mut self.fixed_images) {
            if image.role.is_empty() {
                return Err("fixed image role must be non-empty".to_owned());
            }
            validate_digest(&image.sha256, "fixed image")?;
            if !paths.is_absolute(&image.path) {
                return Err("fixed image paths must be absolute".to_owned());
            }
            let (path, _) = paths.resolve(&image.path, "fixed image", false)?;
            budget::path_bound(&path)?;
            let normalized = FixedImage {
                role: image.role,
                path,
                sha256: image.sha256.to_ascii_lowercase(),
                root_exit_disposition: image.root_exit_disposition,
            };
            canonical_bytes = canonical_bytes
                .checked_add(budget::encoded_size(&normalized, BUDGET_CANONICAL_POLICY_BYTES)? + 1)
                .ok_or("canonical policy size overflow")?;
            budget::bound(
                canonical_bytes,
                BUDGET_CANONICAL_POLICY_BYTES,
                "canonical policy",
            )?;
            normalized_images.push(normalized);
        }
        normalized_images.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.role.cmp(&b.role)));
        normalized_images.dedup();
        let mut fixed = BTreeMap::new();
        let mut group_roles_bytes = 2_usize;
        // Reconcile aliases before executable I/O; hash each canonical group once.
        for image in &normalized_images {
            if !fixed.contains_key(&image.path) {
                group_roles_bytes = 2;
                budget::bound(
                    fixed.len() + 1,
                    BUDGET_DISTINCT_FIXED_PATHS,
                    "distinct fixed paths",
                )?;
                fixed.insert(
                    image.path.clone(),
                    FixedAuthority {
                        path: image.path.clone(),
                        sha256: image.sha256.clone(),
                        roles: BTreeSet::new(),
                        root_exit_disposition: image.root_exit_disposition,
                    },
                );
            }
            let authority = fixed.get_mut(&image.path).expect("fixed group inserted");
            if authority.sha256 != image.sha256 {
                return Err("one executable identity has conflicting fixed digests".to_owned());
            }
            if authority.root_exit_disposition != image.root_exit_disposition {
                return Err(
                    "one executable identity has conflicting root-exit dispositions".to_owned(),
                );
            }
            if !authority.roles.contains(&image.role) {
                group_roles_bytes = group_roles_bytes
                    .checked_add(
                        budget::encoded_size(&image.role, BUDGET_ONE_IMAGE_ROLES_JSON_BYTES)?
                            + usize::from(!authority.roles.is_empty()),
                    )
                    .ok_or("fixed role encoding overflow")?;
                budget::bound(
                    group_roles_bytes,
                    BUDGET_ONE_IMAGE_ROLES_JSON_BYTES,
                    "fixed image roles",
                )?;
                authority.roles.insert(image.role.clone());
            }
        }
        if fixed.is_empty() {
            return Err("policy must contain at least the root fixed image".to_owned());
        }
        let (root_path, _) = paths.resolve(Path::new(&self.command[0]), "root command", false)?;
        let root_key = root_path.clone();
        let root = fixed
            .get(&root_key)
            .ok_or_else(|| "root command is outside fixed image authority".to_owned())?;
        if !root.roles.contains(&self.root_role) {
            return Err("root command role does not match root_role".to_owned());
        }
        if root.root_exit_disposition != RootExitDisposition::RequireExit {
            return Err("root command must require its own exit".to_owned());
        }
        let mut derived = Vec::new();
        derived
            .try_reserve_exact(self.derived_roots.len())
            .map_err(|_| "derived root reservation refused")?;
        for root in std::mem::take(&mut self.derived_roots) {
            if root.role.is_empty() {
                return Err("derived root role must be non-empty".to_owned());
            }
            if !root.path.is_absolute() {
                return Err("derived root paths must be absolute".to_owned());
            }
            let path = canonical_directory(&root.path, "derived root")?;
            budget::path_bound(&path)?;
            let root = DerivedRoot {
                role: root.role,
                path,
            };
            canonical_bytes = canonical_bytes
                .checked_add(budget::encoded_size(&root, BUDGET_CANONICAL_POLICY_BYTES)? + 1)
                .ok_or("canonical policy size overflow")?;
            budget::bound(
                canonical_bytes,
                BUDGET_CANONICAL_POLICY_BYTES,
                "canonical policy",
            )?;
            derived.push(root);
        }
        derived.sort_by(|a, b| a.path.cmp(&b.path));
        // Component-aware prefix relation; adjacent sorted paths reveal overlaps.
        for pair in derived.windows(2) {
            if path_is_within(&pair[1].path, &pair[0].path) || pair[0].path == pair[1].path {
                return Err("derived roots cannot overlap or repeat".to_owned());
            }
        }
        if self.mode == ClosureMode::Leaf && !derived.is_empty() {
            return Err("leaf closure cannot admit derived executable roots".to_owned());
        }

        let mut canonical = self;
        canonical.cwd = cwd;
        normalized_images.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.role.cmp(&right.role))
        });
        normalized_images.dedup();
        derived.sort_by(|left, right| left.path.cmp(&right.path));
        canonical.fixed_images = normalized_images;
        canonical.derived_roots = derived.clone();
        let policy_sha256 = budget::digest(&canonical, BUDGET_CANONICAL_POLICY_BYTES)?;
        let mut cache = ImageHashCache::default();
        for authority in fixed.values() {
            let (_, retained) = paths.resolve(&authority.path, "fixed image", false)?;
            let opened = evidence::OpenedRegularFile::open(&retained).map_err(|e| e.to_string())?;
            let key = image_cache::opened_file_key(opened.file()).map_err(|e| e.to_string())?;
            let mut file = opened.file();
            let actual = cache
                .digest(&key, &mut file, |file| image_cache::opened_file_key(file))
                .map_err(|e| e.to_string())?;
            opened.verify().map_err(|e| e.to_string())?;
            if !constant_time_eq(actual.as_bytes(), authority.sha256.as_bytes()) {
                return Err(format!(
                    "fixed image digest mismatch for {}",
                    authority.path.display()
                ));
            }
        }
        Ok(ValidatedPolicy {
            policy: canonical,
            policy_sha256,
            root_path,
            fixed,
            derived,
            path_namespace: paths.namespace(),
        })
    }
}

/// Borrowed classification is shared by native image construction and ledger
/// validation. Validation never builds a duplicate path/role/file-id payload.
pub(crate) enum ObservedClassification<'a> {
    Fixed(&'a BTreeSet<String>),
    Derived(&'a str),
    Unknown,
}
impl ObservedClassification<'_> {
    fn class(&self) -> ImageClass {
        match self {
            Self::Fixed(_) => ImageClass::Fixed,
            Self::Derived(_) => ImageClass::Derived,
            Self::Unknown => ImageClass::Unknown,
        }
    }
    fn roles(&self) -> Vec<String> {
        match self {
            Self::Fixed(roles) => roles.iter().cloned().collect(),
            Self::Derived(role) => vec![(*role).to_owned()],
            Self::Unknown => Vec::new(),
        }
    }
    pub(crate) fn matches(&self, image: &FileIdentity) -> bool {
        image.class == self.class()
            && match self {
                Self::Fixed(roles) => {
                    roles.len() == image.roles.len() && roles.iter().eq(image.roles.iter())
                }
                Self::Derived(role) => image.roles.len() == 1 && image.roles[0] == *role,
                Self::Unknown => image.roles.is_empty(),
            }
    }
}

impl ValidatedPolicy {
    pub(crate) fn validate_observed_image_path(&self, path: &Path) -> Result<(), String> {
        match self.path_namespace {
            PolicyPathNamespace::Host if path.is_absolute() => Ok(()),
            PolicyPathNamespace::Host => {
                Err("observed executable image path is not absolute".to_owned())
            }
            PolicyPathNamespace::LinuxGuest => linux_guest_components(path).map(|_| ()),
        }
    }

    pub fn root_exit_disposition(&self, canonical_path: &Path) -> RootExitDisposition {
        self.fixed
            .get(canonical_path)
            .map_or(RootExitDisposition::RequireExit, |authority| {
                authority.root_exit_disposition
            })
    }

    pub fn classify_path(
        &self,
        path: &Path,
        file_id: String,
        size_bytes: u64,
        sha256: String,
    ) -> FileIdentity {
        let canonical =
            dunce::canonicalize(path).unwrap_or_else(|_| dunce::simplified(path).to_path_buf());
        self.classify_observed_image(&canonical, file_id, size_bytes, sha256)
    }

    pub fn classify_observed_image(
        &self,
        canonical_path: &Path,
        file_id: String,
        size_bytes: u64,
        sha256: String,
    ) -> FileIdentity {
        let classification = self.observed_classification(canonical_path, &sha256);
        FileIdentity {
            path: canonical_path.to_path_buf(),
            file_id,
            size_bytes,
            sha256,
            class: classification.class(),
            roles: classification.roles(),
        }
    }

    pub(crate) fn observed_classification(
        &self,
        canonical_path: &Path,
        sha256: &str,
    ) -> ObservedClassification<'_> {
        if let Some(authority) = self.fixed.get(canonical_path) {
            return if constant_time_eq(authority.sha256.as_bytes(), sha256.as_bytes()) {
                ObservedClassification::Fixed(&authority.roles)
            } else {
                ObservedClassification::Unknown
            };
        }
        for root in &self.derived {
            if path_is_within(canonical_path, &root.path) {
                return ObservedClassification::Derived(&root.role);
            }
        }
        ObservedClassification::Unknown
    }
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let opened = evidence::OpenedRegularFile::open(path)?;
    let mut reader = opened.bounded_reader();
    let digest = sha256_reader(&mut reader)?;
    if reader.limit() == 0 {
        return Err(io::Error::other("regular input grew while hashing"));
    }
    opened.verify()?;
    Ok(digest)
}

pub fn sha256_reader(reader: &mut impl Read) -> io::Result<String> {
    let mut digest = Sha256::new();
    // Image hashing is load-bearing and may run on the 1 MiB Windows main
    // stack. Keep the throughput-sized buffer on the heap.
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex_lower(&digest.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

/// Both paths must already come from the live canonical/handle boundary.
/// Native components remain exact: case-sensitive directories and distinct
/// Unicode sequences must never acquire another root's authority.
pub fn path_is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn canonical_file(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = dunce::canonicalize(path)
        .map_err(|error| format!("cannot resolve {label} {}: {error}", path.display()))?;
    if !canonical.is_file() {
        return Err(format!("{label} is not a file: {}", canonical.display()));
    }
    Ok(canonical)
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = dunce::canonicalize(path)
        .map_err(|error| format!("cannot resolve {label} {}: {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "{label} is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

pub(crate) fn validate_digest(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{label} sha256 must be 64 hexadecimal characters"));
    }
    Ok(())
}

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_diagnostics_never_allocate_the_full_input() {
        for material in ["x", "\u{0000}", "é🦀\n\"\\"] {
            let input = material.repeat(crate::BUDGET_EVENT_RECORD_BYTES);
            crate::allocation_observer::arm_at_least(8192);
            let result = bounded_diagnostic(format_args!("prefix: {input}"));
            let observed = crate::allocation_observer::finish_observations();
            let diagnostic = result.unwrap();
            assert_eq!(observed.at_least_threshold, 0, "{observed:?}");
            assert!(diagnostic.ends_with("..."));
            assert!(serde_json::to_vec(&diagnostic).unwrap().len() <= MAX_DIAGNOSTIC_BYTES);
        }
        // Exact-fit text is preserved; ellipsis appears only on overflow.
        let fit = "x".repeat(MAX_DIAGNOSTIC_BYTES - 2);
        assert_eq!(bounded_diagnostic(format_args!("{fit}")).unwrap(), fit);
        let overflow = fit + "x";
        let result = bounded_diagnostic(format_args!("{overflow}")).unwrap();
        assert!(result.ends_with("..."));
        assert_eq!(
            serde_json::to_vec(&result).unwrap().len(),
            MAX_DIAGNOSTIC_BYTES
        );
    }

    #[test]
    fn canonical_paths_use_the_shared_filesystem_authority() {
        let cwd = std::env::current_dir().unwrap();
        let canonical = canonical_directory(&cwd, "test cwd").unwrap();
        assert_eq!(canonical, dunce::canonicalize(&cwd).unwrap());
        assert_eq!(
            canonical_directory(&cwd.join("."), "aliased cwd").unwrap(),
            canonical
        );
    }

    #[test]
    fn path_containment_has_a_component_boundary() {
        let root = Path::new("/tmp/target");
        assert!(path_is_within(Path::new("/tmp/target/a"), root));
        assert!(!path_is_within(Path::new("/tmp/target-escape/a"), root));
    }

    #[cfg(windows)]
    #[test]
    fn windows_simplification_preserves_namespace_semantics() {
        assert_eq!(
            dunce::simplified(Path::new(r"\\?\C:\Molt\safe\image.exe")),
            Path::new(r"C:\Molt\safe\image.exe")
        );
        for raw in [
            r"\\?\C:\Molt\output.\image.exe",
            r"\\?\C:\Molt\output \image.exe",
            r"\\?\C:\Molt\image.exe.",
            r"\\?\C:\Molt\image.exe ",
            r"\\?\C:\Molt\CON.exe",
            r"\\?\C:\Molt\AUX\image.exe",
            r"\\?\C:\Molt\LPT1.txt",
            r"\\?\C:\Molt\COM1 .txt",
            r"\\?\C:\Molt\..\image.exe",
            r"\\?\Volume{test}\image.exe",
            r"\\?\UNC\host\share\image.exe",
            r"\\.\PhysicalDrive0",
        ] {
            let path = Path::new(raw);
            assert_eq!(dunce::simplified(path).as_os_str(), path.as_os_str());
        }
        let root = Path::new(r"C:\Molt\output");
        assert!(!path_is_within(
            dunce::simplified(Path::new(r"\\?\C:\Molt\output.\image.exe")),
            root,
        ));
        assert!(!path_is_within(
            dunce::simplified(Path::new(r"\\?\C:\Molt\output \image.exe")),
            root,
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_identity_preserves_unpaired_native_units() {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        for prefix in [r"\\?\C:\Molt\", r"\\?\UNC\host\share\"] {
            let mut units: Vec<u16> = prefix.encode_utf16().collect();
            units.push(0xd800);
            let path = PathBuf::from(OsString::from_wide(&units));
            assert_eq!(
                dunce::simplified(&path)
                    .as_os_str()
                    .encode_wide()
                    .collect::<Vec<_>>(),
                units,
            );
            let replacement = PathBuf::from(format!("{prefix}\u{fffd}"));
            assert_ne!(path, replacement);
            assert!(!path_is_within(&path, &replacement));
        }
    }

    #[test]
    fn native_keys_do_not_infer_filesystem_case_or_unicode_equivalence() {
        let upper = PathBuf::from("/custody/Output");
        let lower = PathBuf::from("/custody/output");
        let composed = PathBuf::from("/custody/\u{130}");
        let expanded = PathBuf::from("/custody/i\u{307}");
        let keys = BTreeSet::from([
            upper.clone(),
            lower.clone(),
            composed.clone(),
            expanded.clone(),
        ]);
        assert_eq!(keys.len(), 4);
        assert!(!path_is_within(&lower.join("image.exe"), &upper));
        assert!(!path_is_within(&expanded.join("image.exe"), &composed));
    }

    #[cfg(unix)]
    #[test]
    fn unix_path_identity_preserves_bytes_and_backslashes() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let bytes = PathBuf::from(OsString::from_vec(b"/tmp/\xff".to_vec()));
        let replacement = PathBuf::from("/tmp/\u{fffd}");
        assert_ne!(bytes, replacement);
        assert!(!path_is_within(&bytes, &replacement));
        assert_ne!(Path::new(r"/tmp/a\b"), Path::new("/tmp/a/b"));
        assert!(!path_is_within(
            Path::new(r"/tmp/a\b/image"),
            Path::new("/tmp/a/b"),
        ));
    }

    #[test]
    fn canonical_image_authority_preserves_lexical_proxy_and_all_roles() {
        let executable = std::env::current_exe().unwrap();
        let lexical_proxy = executable
            .parent()
            .unwrap()
            .join(".")
            .join(executable.file_name().unwrap());
        let lexical_command = lexical_proxy.to_string_lossy().into_owned();
        let digest = sha256_file(&executable).unwrap();
        let policy = Policy {
            schema: POLICY_SCHEMA.to_owned(),
            nonce: "a".repeat(32),
            mode: ClosureMode::DeclaredTree,
            cwd: std::env::current_dir().unwrap(),
            command: vec![lexical_command.clone(), "build".to_owned()],
            environment: platform::required_environment(),
            root_role: "cargo".to_owned(),
            fixed_images: vec![
                FixedImage {
                    role: "cargo".to_owned(),
                    path: lexical_proxy.clone(),
                    sha256: digest.clone(),
                    root_exit_disposition: RootExitDisposition::RequireExit,
                },
                FixedImage {
                    role: "rustc".to_owned(),
                    path: executable.clone(),
                    sha256: digest,
                    root_exit_disposition: RootExitDisposition::RequireExit,
                },
            ],
            derived_roots: vec![],
        };
        let validated = policy.validate().unwrap();
        assert_eq!(validated.policy.command[0], lexical_command);
        assert_eq!(validated.fixed.len(), 1);
        let authority = validated.fixed.values().next().unwrap();
        assert_eq!(
            authority.roles,
            BTreeSet::from(["cargo".to_owned(), "rustc".to_owned()])
        );
        let identity = validated.classify_path(
            &executable,
            "stable-file-id".to_owned(),
            executable.metadata().unwrap().len(),
            authority.sha256.clone(),
        );
        assert_eq!(identity.class, ImageClass::Fixed);
        assert_eq!(identity.roles, vec!["cargo".to_owned(), "rustc".to_owned()]);
    }

    #[test]
    fn empty_tagged_states_preserve_wire_and_reject_foreign_fields() {
        let eligible = serde_json::json!({"state":"eligible"});
        let full = serde_json::json!({"state":"full"});
        let not_created = serde_json::json!({"source":"not-created"});
        assert_eq!(
            serde_json::to_value(Admission::Eligible {}).unwrap(),
            eligible
        );
        assert_eq!(
            serde_json::to_value(JournalCoverage::Full {}).unwrap(),
            full
        );
        assert_eq!(
            serde_json::to_value(NativeCustody::NotCreated {}).unwrap(),
            not_created
        );
        assert_eq!(
            serde_json::from_value::<Admission>(eligible.clone()).unwrap(),
            Admission::Eligible {}
        );
        assert_eq!(
            serde_json::from_value::<JournalCoverage>(full.clone()).unwrap(),
            JournalCoverage::Full {}
        );
        assert_eq!(
            serde_json::from_value::<NativeCustody>(not_created.clone()).unwrap(),
            NativeCustody::NotCreated {}
        );
        for (mut value, kind) in [(eligible, 0), (full, 1), (not_created, 2)] {
            for (field, extra) in [
                ("cause", serde_json::Value::Null),
                ("remaining_tasks", serde_json::json!(0)),
                ("available", serde_json::json!(true)),
            ] {
                value
                    .as_object_mut()
                    .unwrap()
                    .insert(field.to_owned(), extra);
                let rejected = match kind {
                    0 => serde_json::from_value::<Admission>(value.clone()).is_err(),
                    1 => serde_json::from_value::<JournalCoverage>(value.clone()).is_err(),
                    _ => serde_json::from_value::<NativeCustody>(value.clone()).is_err(),
                };
                assert!(rejected, "accepted foreign field {field}: {value}");
                value.as_object_mut().unwrap().remove(field);
            }
        }
    }

    #[test]
    fn admission_schema_has_no_boolean_or_mixed_state_lane() {
        for value in [
            serde_json::json!({"state":"eligible", "reason":null}),
            serde_json::json!({"state":"eligible", "available":true}),
            serde_json::json!({"state":"ineligible"}),
            serde_json::json!({"state":"unknown"}),
            serde_json::json!({"state":"admitted", "root_stable_process_id":"root", "root_create_sequence":true, "initial_image_sequence":2}),
        ] {
            assert!(serde_json::from_value::<Admission>(value).is_err());
        }
        for admission in [
            Admission::Ineligible {
                reason: " ".to_owned(),
            },
            Admission::Admitted {
                root_stable_process_id: "root".to_owned(),
                root_create_sequence: 0,
                initial_image_sequence: 2,
            },
            Admission::Admitted {
                root_stable_process_id: "root".to_owned(),
                root_create_sequence: 3,
                initial_image_sequence: 2,
            },
        ] {
            assert!(!admission.is_well_formed());
        }
    }

    #[test]
    fn receipt_identity_binds_terminal_material() {
        let policy = ValidatedPolicy {
            policy: Policy {
                schema: POLICY_SCHEMA.to_owned(),
                nonce: "a".repeat(32),
                mode: ClosureMode::Leaf,
                cwd: PathBuf::from("."),
                command: vec!["proof".to_owned()],
                environment: platform::required_environment(),
                root_role: "root".to_owned(),
                fixed_images: Vec::new(),
                derived_roots: Vec::new(),
            },
            policy_sha256: "b".repeat(64),
            root_path: PathBuf::from("proof"),
            fixed: BTreeMap::new(),
            derived: Vec::new(),
            path_namespace: PolicyPathNamespace::Host,
        };
        let capability = Capability {
            schema: CAPABILITY_SCHEMA.to_owned(),
            platform: "test".to_owned(),
            mode: ClosureMode::Leaf,
            backend: "test".to_owned(),
            admission: Admission::Ineligible {
                reason: "test".to_owned(),
            },
            pre_entry_exec_authority: false,
            pre_entry_process_create_authority: false,
            recursive_descendant_authority: false,
            required_environment: platform::required_environment(),
        };
        let mut receipt = Receipt::rejected(&policy, &capability, "unavailable");
        receipt
            .attach_evidence(PublishedEvidence {
                event_log: ArtifactSummary {
                    schema: evidence::EVENT_LOG_SCHEMA.to_owned(),
                    file: "receipt.events.jsonl".to_owned(),
                    count: 0,
                    bytes: 0,
                    sha256: sha256_bytes(b""),
                },
                verified: VerifiedEventLog {
                    admission: capability.admission.clone(),
                    derived_images: IdentitySummary::empty(),
                    accounting: receipt.accounting.clone(),
                    root_exit_code: receipt.root_exit_code,
                    violation_count: receipt.violation_count,
                    violations: receipt.violations.clone(),
                    active_processes: BTreeSet::new(),
                },
            })
            .unwrap();
        assert_eq!(receipt.identity_sha256.len(), 64);
        assert!(receipt.identity_is_valid());
        assert!(receipt.terminal_is_consistent());
        assert!(!receipt.complete);
    }

    #[test]
    fn adversarial_diagnostics_cannot_expand_compact_receipt_past_limit() {
        let policy = ValidatedPolicy {
            policy: Policy {
                schema: POLICY_SCHEMA.to_owned(),
                nonce: "a".repeat(32),
                mode: ClosureMode::Leaf,
                cwd: PathBuf::from("."),
                command: vec!["proof".to_owned()],
                environment: platform::required_environment(),
                root_role: "root".to_owned(),
                fixed_images: Vec::new(),
                derived_roots: Vec::new(),
            },
            policy_sha256: "b".repeat(64),
            root_path: PathBuf::from("proof"),
            fixed: BTreeMap::new(),
            derived: Vec::new(),
            path_namespace: PolicyPathNamespace::Host,
        };
        let capability = Capability {
            schema: CAPABILITY_SCHEMA.to_owned(),
            platform: "test".to_owned(),
            mode: ClosureMode::Leaf,
            backend: "test".to_owned(),
            admission: Admission::Ineligible {
                reason: "test".to_owned(),
            },
            pre_entry_exec_authority: false,
            pre_entry_process_create_authority: false,
            recursive_descendant_authority: false,
            required_environment: platform::required_environment(),
        };
        let mut receipt = Receipt::rejected(&policy, &capability, "unavailable");
        let payload = (0_u32..=31).filter_map(char::from_u32).collect::<String>() + "\"\\café🦀";
        for index in 0..1_000 {
            receipt.record_error(format!("error-{index}-{}", payload.repeat(256)));
            receipt.record_violation(format!("violation-{index}-{}", payload.repeat(256)));
        }
        receipt
            .attach_evidence(PublishedEvidence {
                event_log: ArtifactSummary {
                    schema: evidence::EVENT_LOG_SCHEMA.to_owned(),
                    file: "receipt.events.jsonl".to_owned(),
                    count: 0,
                    bytes: 0,
                    sha256: sha256_bytes(b""),
                },
                verified: VerifiedEventLog {
                    admission: capability.admission.clone(),
                    derived_images: IdentitySummary::empty(),
                    accounting: receipt.accounting.clone(),
                    root_exit_code: receipt.root_exit_code,
                    violation_count: receipt.violation_count,
                    violations: receipt.violations.clone(),
                    active_processes: BTreeSet::new(),
                },
            })
            .unwrap();
        assert_eq!(receipt.errors.len(), MAX_DIAGNOSTICS_PER_CLASS);
        assert_eq!(receipt.violations.len(), MAX_DIAGNOSTICS_PER_CLASS);
        assert_eq!(receipt.error_count, 1_001);
        assert_eq!(receipt.violation_count, 1_000);
        for value in receipt.errors.iter().chain(&receipt.violations) {
            assert!(serde_json::to_vec(value).unwrap().len() <= MAX_DIAGNOSTIC_BYTES);
        }
        assert!(serde_json::to_vec_pretty(&receipt).unwrap().len() < evidence::MAX_RECEIPT_BYTES);
        assert!(receipt.identity_is_valid());

        // Publication can fail after both bounded diagnostic lists are full.
        // Its independent cause must not evict or disappear behind the first
        // execution failure, and the diagnostic snapshot cannot mutate facts.
        let before = serde_json::to_value(&receipt).unwrap();
        let diagnostic = receipt.publication_failure_diagnostic("directory sync denied");
        assert!(
            diagnostic.contains("terminal publication not acknowledged: directory sync denied")
        );
        let (_, snapshot) = diagnostic
            .split_once("terminal receipt snapshot (diagnostic only): ")
            .unwrap();
        let retained: Receipt = serde_json::from_str(snapshot).unwrap();
        assert_eq!(serde_json::to_value(retained).unwrap(), before);
        assert_eq!(serde_json::to_value(&receipt).unwrap(), before);
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "windows")))]
mod journal_failure_tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const SUBJECT: &str = "journal_failure_tests::subject";

    /// Invoked only as a real supervised child of the control below. Both
    /// workers rendezvous before exit so the first failed terminal append
    /// leaves other actual generations for the cleanup owner to drain.
    #[test]
    #[ignore]
    fn subject() {
        let directory = PathBuf::from(std::env::var_os("MOLT_JOURNAL_TEST_DIRECTORY").unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        if let Ok(worker) = std::env::var("MOLT_JOURNAL_TEST_WORKER") {
            fs::write(directory.join(format!("ready-{worker}")), b"entered").unwrap();
            while !directory.join("release-workers").exists() {
                assert!(Instant::now() < deadline, "worker rendezvous expired");
                std::thread::sleep(Duration::from_millis(1));
            }
            return;
        }
        fs::write(directory.join("root-entered"), b"entered").unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for worker in ["0", "1"] {
            children.push(
                Command::new(&executable)
                    .args(["--exact", SUBJECT, "--ignored"])
                    .env("MOLT_JOURNAL_TEST_WORKER", worker)
                    .spawn()
                    .unwrap(),
            );
        }
        while !(directory.join("ready-0").exists() && directory.join("ready-1").exists()) {
            assert!(Instant::now() < deadline, "root rendezvous expired");
            std::thread::sleep(Duration::from_millis(1));
        }
        fs::write(directory.join("release-workers"), b"released").unwrap();
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        fs::write(directory.join("root-completed"), b"completed").unwrap();
    }

    #[test]
    fn permanent_journal_failure_does_not_abandon_actual_generation_cleanup() {
        // Linux library tests also own raw ptrace fixtures. Serialize their
        // wait domain: every wait(-1, __WALL) must see only this live closure.
        #[cfg(target_os = "linux")]
        let _serial = platform::linux_test_custody();
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let executable = std::env::current_exe().unwrap();
        let image_digest = sha256_file(&executable).unwrap();
        // Root CREATE, descendant CREATE, first EXIT with other generations
        // still retained, and final root EXIT. None is a mock exit.
        let limits = [None, Some(0), Some(1), Some(2), Some(6), Some(8)];
        for limit in limits {
            let directory = std::env::temp_dir().join(format!(
                "molt-journal-kernel-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let receipt_path = directory.join("receipt.json");
            let mut environment = platform::required_environment();
            environment.insert(
                "MOLT_JOURNAL_TEST_DIRECTORY".to_owned(),
                directory.display().to_string(),
            );
            let policy = Policy {
                schema: POLICY_SCHEMA.to_owned(),
                nonce: "a".repeat(32),
                mode: ClosureMode::DeclaredTree,
                cwd: std::env::current_dir().unwrap(),
                command: vec![
                    executable.display().to_string(),
                    "--exact".to_owned(),
                    SUBJECT.to_owned(),
                    "--ignored".to_owned(),
                ],
                environment,
                root_role: "test".to_owned(),
                fixed_images: vec![FixedImage {
                    role: "test".to_owned(),
                    path: executable.clone(),
                    sha256: image_digest.clone(),
                    root_exit_disposition: RootExitDisposition::RequireExit,
                }],
                derived_roots: Vec::new(),
            }
            .validate()
            .unwrap();
            let capability = platform::capability(policy.policy.mode);
            assert_eq!(capability.admission, Admission::Eligible {});
            let mut journal = EventJournal::create(&receipt_path, &policy, &capability).unwrap();
            if let Some(limit) = limit {
                journal.refuse_writes_after(limit).unwrap();
            }
            let mut receipt = platform::run(&policy, &mut journal, capability);
            if limit.is_none() {
                receipt.attach_evidence(journal.publish().unwrap()).unwrap();
                assert!(receipt.complete, "{receipt:#?}");
                assert_eq!(receipt.accounting.process_creates, 3);
                assert_eq!(receipt.accounting.process_exits, 3);
                assert!(directory.join("ready-0").exists() && directory.join("ready-1").exists());
                assert_eq!(
                    fs::read(directory.join("root-completed")).unwrap(),
                    b"completed"
                );
            } else {
                assert!(!receipt.complete, "{receipt:#?}");
                assert!(
                    receipt.errors[0].contains("cannot append process event"),
                    "{receipt:#?}"
                );
                assert!(receipt.native_custody.is_closed(), "{receipt:#?}");
                assert!(matches!(
                    receipt.journal_coverage,
                    JournalCoverage::Prefix { .. }
                ));
                assert!(journal.publish().unwrap_err().contains("poisoned"));
                let summary = receipt
                    .errors
                    .iter()
                    .find(|error| error.starts_with("cleanup terminal "))
                    .unwrap();
                #[cfg(target_os = "linux")]
                assert!(
                    summary.contains("unresolved process capabilities=0; wait exhaustion=true"),
                    "{receipt:#?}"
                );
                #[cfg(target_os = "windows")]
                {
                    assert!(summary.contains("root handle wait=0x0"), "{receipt:#?}");
                    assert!(summary.contains("active debug processes=0"), "{receipt:#?}");
                    assert!(
                        matches!(
                            receipt.native_custody,
                            NativeCustody::Windows {
                                job: Some(KernelAccounting::WindowsJob {
                                    active_processes: 0,
                                    ..
                                }),
                                ..
                            }
                        ),
                        "{receipt:#?}"
                    );
                }
                if matches!(limit, Some(0 | 1)) {
                    assert!(!directory.join("root-entered").exists());
                }
                if limit == Some(2) {
                    assert!(
                        !directory.join("ready-0").exists() && !directory.join("ready-1").exists()
                    );
                }
                if limit == Some(6) {
                    assert!(
                        directory.join("ready-0").exists() && directory.join("ready-1").exists()
                    );
                    assert!(summary.contains("count="));
                }
                assert_eq!(directory.join("root-completed").exists(), limit == Some(8));
            }
            // Every artifact belongs to this finite fixture. Only after the
            // real closure checks above is its directory eligible for removal.
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

/// Test-only observation of the real allocator boundary on the executing
/// thread. Other test threads and all production builds are unaffected.
#[cfg(test)]
pub(crate) mod allocation_observer {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static GROWTHS: Cell<usize> = const { Cell::new(0) };
        static LARGEST: Cell<usize> = const { Cell::new(0) };
        static THRESHOLD: Cell<usize> = const { Cell::new(usize::MAX) };
        static LARGE: Cell<usize> = const { Cell::new(0) };
    }
    struct ObservedSystem;
    fn observe(size: usize) {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            let _ = GROWTHS.try_with(|n| n.set(n.get() + 1));
            let _ = LARGEST.try_with(|n| n.set(n.get().max(size)));
            if THRESHOLD.try_with(|n| size >= n.get()).unwrap_or(false) {
                let _ = LARGE.try_with(|n| n.set(n.get() + 1));
            }
        }
    }
    unsafe impl GlobalAlloc for ObservedSystem {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            observe(l.size());
            unsafe { System.alloc(l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            observe(l.size());
            unsafe { System.alloc_zeroed(l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            observe(n);
            unsafe { System.realloc(p, l, n) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: ObservedSystem = ObservedSystem;
    #[derive(Debug)]
    pub struct Observations {
        pub growths: usize,
        pub largest: usize,
        pub at_least_threshold: usize,
    }
    pub fn arm() {
        arm_at_least(usize::MAX);
    }
    pub fn arm_at_least(minimum: usize) {
        GROWTHS.with(|n| n.set(0));
        LARGEST.with(|n| n.set(0));
        LARGE.with(|n| n.set(0));
        THRESHOLD.with(|n| n.set(minimum));
        ARMED.with(|n| n.set(true));
    }
    pub fn finish_observations() -> Observations {
        ARMED.with(|n| n.set(false));
        Observations {
            growths: GROWTHS.with(Cell::get),
            largest: LARGEST.with(Cell::get),
            at_least_threshold: LARGE.with(Cell::get),
        }
    }
    pub fn finish() -> usize {
        finish_observations().growths
    }
}
