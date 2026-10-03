use crate::{
    Accounting, CAPABILITY_SCHEMA, Capability, ClosureMode, FileIdentity, ImageClass, ProcessEvent,
    ProcessEventKind, RootExitDisposition, ValidatedPolicy, push_bounded_diagnostic,
    validate_digest,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessLedgerSnapshot {
    pub accounting: Accounting,
    pub root_exit_code: Option<i64>,
    pub derived_images: Vec<FileIdentity>,
    pub violation_count: u64,
    pub violations: Vec<String>,
    pub active_processes: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RecordOutcome {
    terminate_closure: bool,
    policy_violation: bool,
}

impl RecordOutcome {
    pub fn must_terminate_closure(self) -> bool {
        self.terminate_closure
    }

    pub fn has_policy_violation(self) -> bool {
        self.policy_violation
    }
}

#[derive(Clone, Debug)]
struct ActiveProcess {
    process_id: u32,
    image: Option<FileIdentity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessEventDialect {
    WindowsDebugProcess,
    LinuxPtrace,
    Unavailable,
}

impl ProcessEventDialect {
    fn from_capability(capability: &Capability, policy: &ValidatedPolicy) -> Result<Self, String> {
        if capability.schema != CAPABILITY_SCHEMA {
            return Err(format!(
                "event ledger capability schema must be {CAPABILITY_SCHEMA}"
            ));
        }
        if capability.mode != policy.policy.mode {
            return Err("event ledger capability mode disagrees with policy".to_owned());
        }
        match (capability.platform.as_str(), capability.backend.as_str()) {
            ("windows", "debug-process+nested-job") if capability.available => {
                Ok(Self::WindowsDebugProcess)
            }
            ("linux", "ptrace-exitkill") if capability.available => Ok(Self::LinuxPtrace),
            _ if !capability.available => Ok(Self::Unavailable),
            _ => Err(format!(
                "event ledger has no dialect for available backend {}/{}",
                capability.platform, capability.backend
            )),
        }
    }

    fn validate(self, event: &ProcessEvent, root_missing: bool) -> Result<(), String> {
        let valid = match self {
            Self::WindowsDebugProcess => match &event.event {
                ProcessEventKind::ProcessCreate { image, .. } => image.is_some(),
                ProcessEventKind::ProcessExit { .. } => true,
                ProcessEventKind::Fork { .. }
                | ProcessEventKind::Exec { .. }
                | ProcessEventKind::CloneUnclassified { .. } => false,
            },
            Self::LinuxPtrace => match &event.event {
                ProcessEventKind::ProcessCreate { image, .. } => root_missing && image.is_none(),
                ProcessEventKind::ProcessExit { .. }
                | ProcessEventKind::Fork { .. }
                | ProcessEventKind::Exec { .. }
                | ProcessEventKind::CloneUnclassified { .. } => true,
            },
            Self::Unavailable => false,
        };
        if valid {
            Ok(())
        } else {
            let backend = match self {
                Self::WindowsDebugProcess => "Windows debug-process",
                Self::LinuxPtrace => "Linux ptrace",
                Self::Unavailable => "unavailable",
            };
            Err(format!(
                "process event is invalid for the sealed {backend} backend dialect"
            ))
        }
    }
}

pub(crate) struct ProcessLedger {
    policy: ValidatedPolicy,
    dialect: ProcessEventDialect,
    accounting: Accounting,
    root_stable_id: Option<String>,
    root_exit_code: Option<i64>,
    active_by_stable_id: BTreeMap<String, ActiveProcess>,
    active_by_process_id: BTreeMap<u32, String>,
    seen_stable_ids: BTreeSet<String>,
    derived: BTreeMap<PathBuf, FileIdentity>,
    violation_count: u64,
    violations: Vec<String>,
}

impl ProcessLedger {
    pub(crate) fn new(policy: &ValidatedPolicy, capability: &Capability) -> Result<Self, String> {
        Ok(Self {
            policy: policy.clone(),
            dialect: ProcessEventDialect::from_capability(capability, policy)?,
            accounting: Accounting::default(),
            root_stable_id: None,
            root_exit_code: None,
            active_by_stable_id: BTreeMap::new(),
            active_by_process_id: BTreeMap::new(),
            seen_stable_ids: BTreeSet::new(),
            derived: BTreeMap::new(),
            violation_count: 0,
            violations: Vec::new(),
        })
    }

    pub(crate) fn apply(&mut self, event: &ProcessEvent) -> Result<RecordOutcome, String> {
        self.dialect
            .validate(event, self.root_stable_id.is_none())?;
        if self.root_exit_code.is_some()
            && !matches!(&event.event, ProcessEventKind::ProcessExit { .. })
        {
            return Err("event log expands process activity after the root exited".to_owned());
        }
        let mut outcome = RecordOutcome::default();
        match &event.event {
            ProcessEventKind::ProcessCreate {
                parent_process_id,
                image,
            } => {
                let root = self.root_stable_id.is_none();
                if root {
                    if parent_process_id.is_some() {
                        return Err(
                            "root process creation names a parent inside the closure".to_owned()
                        );
                    }
                } else {
                    self.validate_live_parent(*parent_process_id)?;
                }
                let process_creates = self.validate_process_creation(event, image.as_ref())?;
                let mut execs = self.accounting.execs;
                let mut root_execs = self.accounting.root_execs;
                if let Some(image) = image {
                    if root {
                        self.validate_initial_root_image(image)?;
                    }
                    execs = checked_increment(execs, "exec")?;
                    if root {
                        root_execs = checked_increment(root_execs, "root exec")?;
                    }
                }
                self.create_process(event, image.as_ref(), process_creates);
                self.accounting.execs = execs;
                self.accounting.root_execs = root_execs;
                if root {
                    self.root_stable_id = Some(event.stable_process_id.clone());
                } else if self.policy.policy.mode == ClosureMode::Leaf {
                    self.record_violation(
                        format!(
                            "leaf closure observed descendant process {}",
                            event.process_id
                        ),
                        &mut outcome,
                    );
                } else if let Some(image) = image {
                    self.reject_unknown_image(image, event.process_id, &mut outcome);
                }
                if root && let Some(image) = image {
                    self.reject_unknown_image(image, event.process_id, &mut outcome);
                }
            }
            ProcessEventKind::Fork {
                parent_process_id,
                image,
            } => {
                self.require_root_creation()?;
                let parent = self.require_live_parent(*parent_process_id)?;
                if parent.image.as_ref() != image.as_ref() {
                    return Err("fork image disagrees with inherited live parent image".to_owned());
                }
                let process_creates = self.validate_process_creation(event, image.as_ref())?;
                self.create_process(event, image.as_ref(), process_creates);
                if self.policy.policy.mode == ClosureMode::Leaf {
                    self.record_violation(
                        format!(
                            "leaf closure observed descendant process {}",
                            event.process_id
                        ),
                        &mut outcome,
                    );
                } else if let Some(image) = image {
                    self.reject_unknown_image(image, event.process_id, &mut outcome);
                }
            }
            ProcessEventKind::Exec { image, .. } => {
                self.require_root_creation()?;
                self.require_active_process(event)?;
                self.validate_image(image)?;
                let root = self.root_stable_id.as_deref() == Some(&event.stable_process_id);
                if root && self.accounting.root_execs == 0 {
                    self.validate_initial_root_image(image)?;
                }
                let execs = checked_increment(self.accounting.execs, "exec")?;
                let root_execs = if root {
                    checked_increment(self.accounting.root_execs, "root exec")?
                } else {
                    self.accounting.root_execs
                };
                self.observe_image(image);
                self.active_by_stable_id
                    .get_mut(&event.stable_process_id)
                    .expect("active process was checked")
                    .image = Some(image.clone());
                self.accounting.execs = execs;
                self.accounting.root_execs = root_execs;
                self.reject_unknown_image(image, event.process_id, &mut outcome);
            }
            ProcessEventKind::ProcessExit { exit_code } => {
                self.require_root_creation()?;
                self.require_active_process(event)?;
                let process_exits =
                    checked_increment(self.accounting.process_exits, "process exit")?;
                let root = self.root_stable_id.as_deref() == Some(&event.stable_process_id);
                let (remaining, non_auxiliary) = if root {
                    if self.root_exit_code.is_some() {
                        return Err("event log exits the root process more than once".to_owned());
                    }
                    self.root_exit_descendants(&event.stable_process_id)
                } else {
                    (0, 0)
                };
                let terminated_processes = if remaining > 0 && non_auxiliary == 0 {
                    self.accounting
                        .root_exit_terminated_processes
                        .checked_add(remaining as u64)
                        .ok_or_else(|| "root-exit termination count overflow".to_owned())?
                } else {
                    self.accounting.root_exit_terminated_processes
                };
                self.active_by_stable_id.remove(&event.stable_process_id);
                self.active_by_process_id.remove(&event.process_id);
                self.accounting.process_exits = process_exits;
                self.accounting.root_exit_terminated_processes = terminated_processes;
                if root {
                    self.root_exit_code = Some(*exit_code);
                }
                if remaining > 0 {
                    outcome.terminate_closure = true;
                    if non_auxiliary > 0 {
                        self.record_violation(
                            format!("root exited before {non_auxiliary} non-auxiliary descendant process(es)"),
                            &mut outcome,
                        );
                    }
                }
            }
            ProcessEventKind::CloneUnclassified {
                parent_process_id,
                reason,
            } => {
                self.require_root_creation()?;
                self.validate_live_parent(Some(*parent_process_id))?;
                if reason.is_empty() {
                    return Err("unclassified clone event has an empty reason".to_owned());
                }
                self.record_violation(reason.clone(), &mut outcome);
            }
        }
        self.accounting.active_processes = self.active_by_stable_id.len() as u64;
        Ok(outcome)
    }

    pub(crate) fn snapshot(&self) -> ProcessLedgerSnapshot {
        ProcessLedgerSnapshot {
            accounting: self.accounting.clone(),
            root_exit_code: self.root_exit_code,
            derived_images: self.derived.values().cloned().collect(),
            violation_count: self.violation_count,
            violations: self.violations.clone(),
            active_processes: self.active_by_stable_id.keys().cloned().collect(),
        }
    }

    fn validate_process_creation(
        &self,
        event: &ProcessEvent,
        image: Option<&FileIdentity>,
    ) -> Result<u64, String> {
        if event.stable_process_id.is_empty() {
            return Err("process event has an empty stable identity".to_owned());
        }
        if self.seen_stable_ids.contains(&event.stable_process_id) {
            return Err("event log reuses one stable process identity".to_owned());
        }
        if self.active_by_process_id.contains_key(&event.process_id) {
            return Err(format!(
                "event log reuses live process id {}",
                event.process_id
            ));
        }
        if let Some(image) = image {
            self.validate_image(image)?;
        }
        checked_increment(self.accounting.process_creates, "process creation")
    }

    // Commit helpers are infallible: every semantic check and next counter is
    // validated before changing the accepted event prefix.
    fn create_process(
        &mut self,
        event: &ProcessEvent,
        image: Option<&FileIdentity>,
        process_creates: u64,
    ) {
        if let Some(image) = image {
            self.observe_image(image);
        }
        self.seen_stable_ids.insert(event.stable_process_id.clone());
        self.active_by_process_id
            .insert(event.process_id, event.stable_process_id.clone());
        self.active_by_stable_id.insert(
            event.stable_process_id.clone(),
            ActiveProcess {
                process_id: event.process_id,
                image: image.cloned(),
            },
        );
        self.accounting.process_creates = process_creates;
    }

    fn require_root_creation(&self) -> Result<(), String> {
        if self.root_stable_id.is_none() {
            Err("event log begins before the root process creation".to_owned())
        } else {
            Ok(())
        }
    }

    fn require_active_process(&self, event: &ProcessEvent) -> Result<(), String> {
        let active = self
            .active_by_stable_id
            .get(&event.stable_process_id)
            .ok_or_else(|| "event log references a process that is not live".to_owned())?;
        if active.process_id != event.process_id {
            return Err("stable process identity changed its numeric process id".to_owned());
        }
        Ok(())
    }

    fn validate_live_parent(&self, parent_process_id: Option<u32>) -> Result<(), String> {
        let Some(parent) = parent_process_id else {
            return Ok(());
        };
        self.require_live_parent(parent).map(|_| ())
    }

    fn require_live_parent(&self, process_id: u32) -> Result<&ActiveProcess, String> {
        self.active_by_process_id
            .get(&process_id)
            .and_then(|stable_id| self.active_by_stable_id.get(stable_id))
            .ok_or_else(|| format!("process event names non-live parent process {process_id}"))
    }

    fn validate_image(&self, image: &FileIdentity) -> Result<(), String> {
        if !image.path.is_absolute() {
            return Err("observed executable image path is not absolute".to_owned());
        }
        validate_digest(&image.sha256, "observed executable image")?;
        let expected = self
            .policy
            .classify_observed_image(&image.path, image.file_id.clone(), image.size_bytes, image.sha256.clone());
        if &expected != image {
            return Err(format!(
                "event image classification disagrees with sealed policy: {}",
                image.path.display()
            ));
        }
        if image.class == ImageClass::Derived
            && let Some(prior) = self.derived.get(&image.path)
            && prior != image
        {
            return Err(format!(
                "derived image identity changed within event log: {}",
                image.path.display()
            ));
        }
        Ok(())
    }

    fn observe_image(&mut self, image: &FileIdentity) {
        if image.class == ImageClass::Derived {
            self.derived
                .entry(image.path.clone())
                .or_insert_with(|| image.clone());
        }
    }

    fn reject_unknown_image(
        &mut self,
        image: &FileIdentity,
        process_id: u32,
        outcome: &mut RecordOutcome,
    ) {
        if image.class == ImageClass::Unknown
            && self.policy.policy.mode != ClosureMode::InventoryTree
        {
            self.record_violation(
                format!(
                    "unadmitted executable image {} in process {process_id}",
                    image.path.display()
                ),
                outcome,
            );
        }
    }

    fn validate_initial_root_image(&self, image: &FileIdentity) -> Result<(), String> {
        let expected_path = &self.policy.root_path;
        if &image.path != expected_path
            || image.class != ImageClass::Fixed
            || !image.roles.contains(&self.policy.policy.root_role)
        {
            return Err(
                "initial root image disagrees with sealed root command authority".to_owned(),
            );
        }
        Ok(())
    }

    fn root_exit_descendants(&self, root_stable_id: &str) -> (usize, usize) {
        let mut remaining = 0;
        let mut non_auxiliary = 0;
        for (_, process) in self
            .active_by_stable_id
            .iter()
            .filter(|(stable_id, _)| stable_id.as_str() != root_stable_id)
        {
            remaining += 1;
            if process.image.as_ref().is_none_or(|image| {
                image.class != ImageClass::Fixed
                    || self.policy.root_exit_disposition(&image.path)
                        != RootExitDisposition::Terminate
            }) {
                non_auxiliary += 1;
            }
        }
        (remaining, non_auxiliary)
    }

    fn record_violation(&mut self, violation: String, outcome: &mut RecordOutcome) {
        self.violation_count = self.violation_count.saturating_add(1);
        push_bounded_diagnostic(&mut self.violations, violation);
        outcome.terminate_closure = true;
        outcome.policy_violation = true;
    }
}

fn checked_increment(value: u64, label: &str) -> Result<u64, String> {
    value
        .checked_add(1)
        .ok_or_else(|| format!("{label} count overflow"))
}
