use crate::budget;
use crate::{
    Accounting, Admission, CAPABILITY_SCHEMA, Capability, ClosureMode, FileIdentity, ImageClass,
    ProcessEvent, ProcessEventKind, RootExitDisposition, ValidatedPolicy, validate_digest,
};
use std::borrow::Borrow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;

/// The immutable path inside FileIdentity is the sole registry key owner.
/// Set identity is path-only; drift validation separately compares the complete
/// stored FileIdentity, never this key equality relation.
struct DerivedEntry(FileIdentity);
impl Borrow<Path> for DerivedEntry {
    fn borrow(&self) -> &Path {
        &self.0.path
    }
}
impl PartialEq for DerivedEntry {
    fn eq(&self, other: &Self) -> bool {
        self.0.path == other.0.path
    }
}
impl Eq for DerivedEntry {}
impl Hash for DerivedEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.path.hash(state);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessLedgerSnapshot {
    pub admission: Admission,
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
    Ineligible,
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
        if !capability.admission.is_well_formed() {
            return Err("event ledger admission state is malformed".to_owned());
        }
        if matches!(capability.admission, Admission::Ineligible { .. }) {
            return Ok(Self::Ineligible);
        }
        // A claimed witness never authorizes events. Eligible and admitted
        // inputs both replay the same backend and derive admission from scratch.
        match (capability.platform.as_str(), capability.backend.as_str()) {
            ("windows", "debug-process+nested-job") => Ok(Self::WindowsDebugProcess),
            ("linux", "ptrace-exitkill") => Ok(Self::LinuxPtrace),
            _ => Err(crate::bounded_diagnostic(format_args!(
                "event ledger has no dialect for planned backend {}/{}",
                capability.platform, capability.backend
            ))?),
        }
    }

    fn validate(self, event: &ProcessEvent, root_missing: bool) -> Result<(), String> {
        let valid = match self {
            Self::WindowsDebugProcess => match &event.event {
                ProcessEventKind::ProcessCreate { .. } => true,
                ProcessEventKind::InitialImage { .. } => true,
                ProcessEventKind::ProcessExit { .. } => true,
                ProcessEventKind::Fork { .. }
                | ProcessEventKind::Exec { .. }
                | ProcessEventKind::CloneUnclassified { .. } => false,
            },
            Self::LinuxPtrace => match &event.event {
                ProcessEventKind::ProcessCreate { .. } => root_missing,
                ProcessEventKind::ProcessExit { .. }
                | ProcessEventKind::Fork { .. }
                | ProcessEventKind::Exec { .. }
                | ProcessEventKind::CloneUnclassified { .. } => true,
                ProcessEventKind::InitialImage { .. } => false,
            },
            Self::Ineligible => false,
        };
        if valid {
            Ok(())
        } else {
            let backend = match self {
                Self::WindowsDebugProcess => "Windows debug-process",
                Self::LinuxPtrace => "Linux ptrace",
                Self::Ineligible => "ineligible",
            };
            Err(format!(
                "process event is invalid for the sealed {backend} backend dialect"
            ))
        }
    }
}

pub(crate) struct ProcessLedger<'policy> {
    policy: &'policy ValidatedPolicy,
    dialect: ProcessEventDialect,
    accounting: Accounting,
    root_stable_id: Option<String>,
    root_create_sequence: Option<u64>,
    admission: Admission,
    root_exit_code: Option<i64>,
    active_by_stable_id: HashMap<String, ActiveProcess>,
    active_by_process_id: HashMap<u32, String>,
    seen_stable_ids: HashSet<String>,
    derived: HashSet<DerivedEntry>,
    violation_count: u64,
    violations: Vec<String>,
    retained_bytes: usize,
    derived_bytes: usize,
}

impl<'policy> ProcessLedger<'policy> {
    pub(crate) fn new(
        policy: &'policy ValidatedPolicy,
        capability: &Capability,
    ) -> Result<Self, String> {
        let dialect = ProcessEventDialect::from_capability(capability, policy)?;
        let (admission, retained_bytes) = match &capability.admission {
            Admission::Ineligible { reason } => {
                // The dialect constructor already validated the existing admission bound.
                budget::bound(
                    reason.len(),
                    crate::BUDGET_RETAINED_OBSERVATION_PAYLOAD_BYTES,
                    "retained observation storage",
                )?;
                (
                    Admission::Ineligible {
                        reason: budget::copy_string(reason)?,
                    },
                    reason.len(),
                )
            }
            Admission::Eligible {} | Admission::Admitted { .. } => (Admission::Eligible {}, 0),
        };
        Ok(Self {
            policy,
            dialect,
            accounting: Accounting::default(),
            root_stable_id: None,
            root_create_sequence: None,
            admission,
            root_exit_code: None,
            active_by_stable_id: HashMap::new(),
            active_by_process_id: HashMap::new(),
            seen_stable_ids: HashSet::new(),
            derived: HashSet::new(),
            violation_count: 0,
            violations: Vec::new(),
            retained_bytes,
            derived_bytes: 0,
        })
    }

    pub(crate) fn apply(&mut self, event: &ProcessEvent) -> Result<RecordOutcome, String> {
        Ok(self.prepare(event)?.commit())
    }

    /// Validate without changing the accepted prefix. The exclusive ledger
    /// borrow prevents any intervening event while the journal appends bytes.
    /// A failed append drops this transition without committing its witness.
    pub(crate) fn prepare<'ledger, 'event>(
        &'ledger mut self,
        event: &'event ProcessEvent,
    ) -> Result<PreparedLedgerTransition<'ledger, 'event, 'policy>, String> {
        // Borrowed preflight precedes classification, diagnostics, copies and reservations.
        budget::event_shape(event)?;
        if event.sequence == 0 || event.sequence > crate::evidence::MAX_EVENT_RECORDS {
            return Err("process event sequence is outside the admitted journal bound".to_owned());
        }
        self.dialect
            .validate(event, self.root_stable_id.is_none())?;
        if self.root_exit_code.is_some()
            && !matches!(&event.event, ProcessEventKind::ProcessExit { .. })
        {
            return Err("event log expands process activity after the root exited".to_owned());
        }
        let mut outcome = RecordOutcome::default();
        let mut violation = None;
        let delta = match &event.event {
            ProcessEventKind::ProcessCreate {
                parent_process_id, ..
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
                let process_creates = self.validate_process_creation(event, None)?;
                if !root && self.policy.policy.mode == ClosureMode::Leaf {
                    violation = Some(Violation::LeafDescendant(event.process_id));
                }
                LedgerDelta::Create {
                    image: None,
                    process_creates,
                    execs: self.accounting.execs,
                    root_execs: self.accounting.root_execs,
                    root,
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
                if self.policy.policy.mode == ClosureMode::Leaf {
                    violation = Some(Violation::LeafDescendant(event.process_id));
                } else if let Some(image) = image {
                    violation = self.unknown_image_violation(image, event.process_id);
                }
                LedgerDelta::Create {
                    image: image.as_ref(),
                    process_creates,
                    execs: self.accounting.execs,
                    root_execs: self.accounting.root_execs,
                    root: false,
                }
            }
            ProcessEventKind::Exec { image } | ProcessEventKind::InitialImage { image } => {
                self.require_root_creation()?;
                self.require_active_process(event)?;
                if matches!(event.event, ProcessEventKind::InitialImage { .. })
                    && self
                        .active_by_stable_id
                        .get(&event.stable_process_id)
                        .is_some_and(|p| p.image.is_some())
                {
                    return Err("initial image repeats admission for a live process".to_owned());
                }
                self.validate_image(image)?;
                let root = self.root_stable_id.as_deref() == Some(&event.stable_process_id);
                let first_root_image = root && self.accounting.root_execs == 0;
                if first_root_image {
                    self.validate_initial_root_image(image)?;
                    if self
                        .root_create_sequence
                        .is_none_or(|created| created >= event.sequence)
                    {
                        return Err("initial root image must follow its creation event".to_owned());
                    }
                }
                let execs = checked_increment(self.accounting.execs, "exec")?;
                let root_execs = if root {
                    checked_increment(self.accounting.root_execs, "root exec")?
                } else {
                    self.accounting.root_execs
                };
                violation = self.unknown_image_violation(image, event.process_id);
                LedgerDelta::Exec {
                    image,
                    execs,
                    root_execs,
                    admit_root: first_root_image,
                }
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
                if remaining > 0 {
                    outcome.terminate_closure = true;
                    if non_auxiliary > 0 {
                        violation = Some(Violation::RootExit(non_auxiliary));
                    }
                }
                LedgerDelta::Exit {
                    process_exits,
                    terminated_processes,
                    root,
                    exit_code: *exit_code,
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
                violation = Some(Violation::Unclassified(reason));
                LedgerDelta::Violation
            }
        };
        let mut storage = PreparedStorage::default();
        let mut next_retained_bytes = self.retained_bytes;
        let mut next_derived_bytes = self.derived_bytes;
        let observed_image = match &delta {
            LedgerDelta::Create { image, .. } => *image,
            LedgerDelta::Exec { image, .. } => Some(*image),
            _ => None,
        };
        if let Some(image) = observed_image {
            let bytes = budget::image_size(image)?;
            if image.class == ImageClass::Derived && !self.derived.contains(image.path.as_path()) {
                budget::bound(
                    self.derived.len() + 1,
                    crate::BUDGET_INVENTORY_UNIQUE_IMAGES,
                    "derived images",
                )?;
                next_derived_bytes = next_derived_bytes
                    .checked_add(bytes)
                    .ok_or("derived budget overflow")?;
                budget::bound(
                    next_derived_bytes,
                    crate::BUDGET_RETAINED_DERIVED_IDENTITY_BYTES,
                    "derived identity storage",
                )?;
                next_retained_bytes = next_retained_bytes
                    .checked_add(bytes + 128)
                    .ok_or("retained budget overflow")?;
            }
        }
        match &delta {
            LedgerDelta::Create { .. } => {
                budget::bound(
                    self.seen_stable_ids.len() + 1,
                    crate::BUDGET_LIFETIME_PROCESSES,
                    "lifetime processes",
                )?;
                budget::bound(
                    self.active_by_stable_id.len() + 1,
                    crate::BUDGET_LIVE_PROCESSES,
                    "live processes",
                )?;
                next_retained_bytes = next_retained_bytes
                    .checked_add(
                        event.stable_process_id.len() * 4
                            + 256
                            + observed_image
                                .map(budget::image_size)
                                .transpose()?
                                .unwrap_or(0),
                    )
                    .ok_or("retained budget overflow")?;
            }
            LedgerDelta::Exec {
                image, admit_root, ..
            } => {
                if *admit_root {
                    next_retained_bytes = next_retained_bytes
                        .checked_add(event.stable_process_id.len())
                        .ok_or("retained budget overflow")?;
                }
                let old = self
                    .active_by_stable_id
                    .get(&event.stable_process_id)
                    .and_then(|p| p.image.as_ref())
                    .map(budget::image_size)
                    .transpose()?
                    .unwrap_or(0);
                next_retained_bytes = next_retained_bytes
                    .checked_sub(old)
                    .ok_or("retained budget underflow")?
                    .checked_add(budget::image_size(image)?)
                    .ok_or("retained budget overflow")?;
            }
            LedgerDelta::Exit { .. } => {
                let old = self
                    .active_by_stable_id
                    .get(&event.stable_process_id)
                    .and_then(|p| p.image.as_ref())
                    .map(budget::image_size)
                    .transpose()?
                    .unwrap_or(0);
                next_retained_bytes = next_retained_bytes
                    .checked_sub(old)
                    .ok_or("retained budget underflow")?;
            }
            _ => {}
        }
        let retain_violation =
            violation.is_some() && self.violations.len() < crate::MAX_DIAGNOSTICS_PER_CLASS;
        if retain_violation {
            // One explicit conservative debit covers the diagnostic constructor's
            // full reserved capacity, including escaped/short/truncated samples.
            // Saturated lists retain no new owner and accrue no phantom debit.
            next_retained_bytes = next_retained_bytes
                .checked_add(crate::MAX_DIAGNOSTIC_BYTES)
                .ok_or("retained budget overflow")?;
        }
        budget::bound(
            next_retained_bytes,
            crate::BUDGET_RETAINED_OBSERVATION_PAYLOAD_BYTES,
            "retained observation storage",
        )?;
        // Only after the complete debit fits do we allocate owned delta
        // payloads and reserve indexes. No fallible work remains after append.
        if let LedgerDelta::Exec {
            admit_root: true, ..
        } = &delta
        {
            storage.admission = Some(Admission::Admitted {
                root_stable_process_id: budget::copy_string(&event.stable_process_id)?,
                root_create_sequence: self.root_create_sequence.expect("root creation checked"),
                initial_image_sequence: event.sequence,
            });
        }
        if let Some(image) = observed_image {
            storage.image = Some(budget::copy_image(image)?);
            if image.class == ImageClass::Derived && !self.derived.contains(image.path.as_path()) {
                self.derived
                    .try_reserve(1)
                    .map_err(|_| "derived index reservation refused")?;
                storage.derived = Some(DerivedEntry(budget::copy_image(image)?));
            }
        }
        if let LedgerDelta::Create { root, .. } = &delta {
            self.seen_stable_ids
                .try_reserve(1)
                .map_err(|_| "seen identity reservation refused")?;
            self.active_by_process_id
                .try_reserve(1)
                .map_err(|_| "PID index reservation refused")?;
            self.active_by_stable_id
                .try_reserve(1)
                .map_err(|_| "active identity reservation refused")?;
            storage.seen_key = Some(budget::copy_string(&event.stable_process_id)?);
            storage.pid_value = Some(budget::copy_string(&event.stable_process_id)?);
            storage.active_key = Some(budget::copy_string(&event.stable_process_id)?);
            if *root {
                storage.root_key = Some(budget::copy_string(&event.stable_process_id)?);
            }
        }
        if retain_violation {
            self.violations
                .try_reserve(1)
                .map_err(|_| "violation reservation refused")?;
            storage.violation = Some(violation.as_ref().expect("retained sample").materialize()?);
        }
        Ok(PreparedLedgerTransition {
            ledger: self,
            event,
            delta,
            violation: violation.is_some(),
            outcome,
            storage,
            next_retained_bytes,
            next_derived_bytes,
        })
    }

    #[cfg(test)]
    pub(crate) fn retained_payload_bytes(&self) -> usize {
        self.retained_bytes
    }

    #[cfg(test)]
    pub(crate) fn diagnostic_storage(&self) -> (usize, usize) {
        (self.violations.len(), self.violations.capacity())
    }

    pub(crate) fn snapshot(&self) -> ProcessLedgerSnapshot {
        ProcessLedgerSnapshot {
            admission: self.admission.clone(),
            accounting: self.accounting.clone(),
            root_exit_code: self.root_exit_code,
            derived_images: {
                let mut images: Vec<_> = self.derived.iter().map(|entry| entry.0.clone()).collect();
                images.sort_by(|a, b| a.path.cmp(&b.path));
                images
            },
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
        if event.stable_process_id.is_empty()
            || event.stable_process_id.len() > crate::BUDGET_STABLE_PROCESS_ID_UTF8_BYTES
        {
            return Err("process event has an empty or oversized stable identity".to_owned());
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
        self.policy.validate_observed_image_path(&image.path)?;
        validate_digest(&image.sha256, "observed executable image")?;
        let expected = self
            .policy
            .observed_classification(&image.path, &image.sha256);
        if !expected.matches(image) {
            return Err(crate::bounded_diagnostic(format_args!(
                "event image classification disagrees with sealed policy: {}",
                image.path.display()
            ))?);
        }
        if image.class == ImageClass::Derived
            && let Some(prior) = self.derived.get(image.path.as_path())
            && &prior.0 != image
        {
            return Err(crate::bounded_diagnostic(format_args!(
                "derived image identity changed within event log: {}",
                image.path.display()
            ))?);
        }
        Ok(())
    }

    fn unknown_image_violation<'image>(
        &self,
        image: &'image FileIdentity,
        process_id: u32,
    ) -> Option<Violation<'image>> {
        (image.class == ImageClass::Unknown
            && self.policy.policy.mode != ClosureMode::InventoryTree)
            .then_some(Violation::UnknownImage { image, process_id })
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
}

/// Only ProcessLedger can construct a prepared transition. Holding its mutable
/// borrow until commit makes the read/append/commit boundary one transaction,
/// without copying a process tree or keeping a second ledger.
pub(crate) struct PreparedLedgerTransition<'ledger, 'event, 'policy> {
    ledger: &'ledger mut ProcessLedger<'policy>,
    event: &'event ProcessEvent,
    delta: LedgerDelta<'event>,
    violation: bool,
    outcome: RecordOutcome,
    storage: PreparedStorage,
    next_retained_bytes: usize,
    next_derived_bytes: usize,
}
#[derive(Default)]
struct PreparedStorage {
    seen_key: Option<String>,
    pid_value: Option<String>,
    active_key: Option<String>,
    root_key: Option<String>,
    image: Option<FileIdentity>,
    derived: Option<DerivedEntry>,
    admission: Option<Admission>,
    violation: Option<String>,
}

/// Borrowed semantic fact, not a second retained diagnostic owner. Formatting
/// is deferred until the combined image/identity/sample debit is admitted.
enum Violation<'event> {
    LeafDescendant(u32),
    UnknownImage {
        image: &'event FileIdentity,
        process_id: u32,
    },
    RootExit(usize),
    Unclassified(&'event str),
}
impl Violation<'_> {
    fn materialize(&self) -> Result<String, String> {
        match self {
            Self::LeafDescendant(pid) => crate::bounded_diagnostic(format_args!(
                "leaf closure observed descendant process {pid}"
            )),
            Self::UnknownImage { image, process_id } => crate::bounded_diagnostic(format_args!(
                "unadmitted executable image {} in process {process_id}",
                image.path.display()
            )),
            Self::RootExit(remaining) => crate::bounded_diagnostic(format_args!(
                "root exited before {remaining} non-auxiliary descendant process(es)"
            )),
            Self::Unclassified(reason) => crate::bounded_diagnostic(format_args!("{reason}")),
        }
    }
}

enum LedgerDelta<'event> {
    Create {
        image: Option<&'event FileIdentity>,
        process_creates: u64,
        execs: u64,
        root_execs: u64,
        root: bool,
    },
    Exec {
        image: &'event FileIdentity,
        execs: u64,
        root_execs: u64,
        admit_root: bool,
    },
    Exit {
        process_exits: u64,
        terminated_processes: u64,
        root: bool,
        exit_code: i64,
    },
    Violation,
}

impl PreparedLedgerTransition<'_, '_, '_> {
    /// All fallible semantic validation and counter arithmetic occurred before
    /// the append. Replay uses this same transition directly after decoding.
    pub(crate) fn commit(self) -> RecordOutcome {
        let Self {
            ledger,
            event,
            delta,
            violation,
            mut outcome,
            mut storage,
            next_retained_bytes,
            next_derived_bytes,
        } = self;
        if let Some(entry) = storage.derived.take() {
            ledger.derived.insert(entry);
        }
        match delta {
            LedgerDelta::Create {
                image: _,
                process_creates,
                execs,
                root_execs,
                root,
            } => {
                ledger
                    .seen_stable_ids
                    .insert(storage.seen_key.take().expect("reserved seen key"));
                ledger.active_by_process_id.insert(
                    event.process_id,
                    storage.pid_value.take().expect("reserved PID value"),
                );
                ledger.active_by_stable_id.insert(
                    storage.active_key.take().expect("reserved active key"),
                    ActiveProcess {
                        process_id: event.process_id,
                        image: storage.image.take(),
                    },
                );
                ledger.accounting.process_creates = process_creates;
                ledger.accounting.execs = execs;
                ledger.accounting.root_execs = root_execs;
                if root {
                    ledger.root_stable_id = storage.root_key.take();
                    ledger.root_create_sequence = Some(event.sequence);
                }
            }
            LedgerDelta::Exec {
                image: _,
                execs,
                root_execs,
                admit_root: _,
            } => {
                ledger
                    .active_by_stable_id
                    .get_mut(&event.stable_process_id)
                    .expect("prepared active process remains owned")
                    .image = storage.image.take();
                ledger.accounting.execs = execs;
                ledger.accounting.root_execs = root_execs;
                if let Some(admission) = storage.admission.take() {
                    ledger.admission = admission;
                }
            }
            LedgerDelta::Exit {
                process_exits,
                terminated_processes,
                root,
                exit_code,
            } => {
                ledger.active_by_stable_id.remove(&event.stable_process_id);
                ledger.active_by_process_id.remove(&event.process_id);
                ledger.accounting.process_exits = process_exits;
                ledger.accounting.root_exit_terminated_processes = terminated_processes;
                if root {
                    ledger.root_exit_code = Some(exit_code);
                }
            }
            LedgerDelta::Violation => {}
        }
        if violation {
            ledger.violation_count = ledger.violation_count.saturating_add(1);
            if let Some(sample) = storage.violation.take() {
                ledger.violations.push(sample);
            }
            outcome.terminate_closure = true;
            outcome.policy_violation = true;
        }
        ledger.retained_bytes = next_retained_bytes;
        ledger.derived_bytes = next_derived_bytes;
        ledger.accounting.active_processes = ledger.active_by_stable_id.len() as u64;
        outcome
    }
}

fn checked_increment(value: u64, label: &str) -> Result<u64, String> {
    value
        .checked_add(1)
        .ok_or_else(|| format!("{label} count overflow"))
}

#[cfg(test)]
mod allocation_boundary_tests {
    use super::*;
    use crate::evidence::tests::{test_capability, test_policy, test_root_image};
    use crate::{
        BUDGET_PATH_UTF8_BYTES, BUDGET_RETAINED_DERIVED_IDENTITY_BYTES,
        BUDGET_RETAINED_OBSERVATION_PAYLOAD_BYTES, DerivedRoot,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn event(sequence: u64, pid: u32, stable: &str, event: ProcessEventKind) -> ProcessEvent {
        ProcessEvent {
            sequence,
            process_id: pid,
            stable_process_id: stable.to_owned(),
            event,
        }
    }

    // Count surviving variable payloads independently of serialization and the
    // admission debit. Table capacity, inline scalar fields and allocator
    // overhead are explicitly outside the retained-payload contract.
    fn image_payload(image: &FileIdentity) -> usize {
        image.path.as_os_str().len()
            + image.file_id.len()
            + image.sha256.len()
            + image.roles.iter().map(String::len).sum::<usize>()
    }
    fn owned_payload(ledger: &ProcessLedger<'_>) -> usize {
        ledger.violations.iter().map(String::len).sum::<usize>()
            + ledger
                .derived
                .iter()
                .map(|entry| image_payload(&entry.0))
                .sum::<usize>()
            + ledger
                .active_by_stable_id
                .iter()
                .map(|(key, process)| {
                    key.len() + process.image.as_ref().map(image_payload).unwrap_or(0)
                })
                .sum::<usize>()
            + ledger
                .seen_stable_ids
                .iter()
                .map(String::len)
                .sum::<usize>()
            + ledger
                .active_by_process_id
                .values()
                .map(String::len)
                .sum::<usize>()
            + ledger.root_stable_id.as_ref().map(String::len).unwrap_or(0)
            + match &ledger.admission {
                Admission::Admitted {
                    root_stable_process_id,
                    ..
                } => root_stable_process_id.len(),
                Admission::Ineligible { reason } => reason.len(),
                _ => 0,
            }
    }

    #[test]
    fn retained_diagnostic_samples_charge_capacity_and_capped_observations_do_not() {
        let policy = test_policy(ClosureMode::DeclaredTree, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let mut ledger = ProcessLedger::new(&policy, &capability).unwrap();
        ledger
            .apply(&event(
                1,
                1,
                "root",
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            ))
            .unwrap();
        let initial = 4 * "root".len() + 256;
        assert_eq!(ledger.retained_bytes, initial);
        let sample_charge = crate::BUDGET_COMBINED_DIAGNOSTICS_JSON_BYTES
            / (2 * crate::BUDGET_DIAGNOSTICS_PER_CLASS);
        for index in 0..crate::MAX_DIAGNOSTICS_PER_CLASS + 1 {
            let reason = if index % 2 == 0 {
                "x".repeat(8192)
            } else {
                "\u{0000}é🦀".repeat(2048)
            };
            let row = event(
                index as u64 + 2,
                2,
                "unclassified",
                ProcessEventKind::CloneUnclassified {
                    parent_process_id: 1,
                    reason,
                },
            );
            crate::allocation_observer::arm_at_least(sample_charge);
            let result = ledger.apply(&row);
            let observed = crate::allocation_observer::finish_observations();
            let result = result.unwrap();
            assert_eq!(
                observed.at_least_threshold,
                usize::from(index < crate::MAX_DIAGNOSTICS_PER_CLASS)
            );
            assert!(result.has_policy_violation() && result.must_terminate_closure());
            let samples = (index + 1).min(crate::MAX_DIAGNOSTICS_PER_CLASS);
            assert_eq!(ledger.violations.len(), samples);
            assert_eq!(ledger.violation_count, (index + 1) as u64);
            assert_eq!(ledger.retained_bytes, initial + samples * sample_charge);
            let actual = ledger.violations.iter().map(String::len).sum::<usize>();
            assert!(actual <= samples * sample_charge);
            assert!(owned_payload(&ledger) <= ledger.retained_bytes);
            assert!(
                ledger
                    .violations
                    .iter()
                    .all(|sample| serde_json::to_vec(sample).unwrap().len() <= sample_charge)
            );
        }
    }

    #[test]
    fn ineligible_admission_reason_is_bounded_and_charged_before_copy() {
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let mut capability = test_capability(policy.policy.mode);
        capability.admission = Admission::Ineligible {
            reason: "unavailable".to_owned(),
        };
        let ledger = ProcessLedger::new(&policy, &capability).unwrap();
        assert_eq!(ledger.retained_bytes, "unavailable".len());
        assert_eq!(owned_payload(&ledger), "unavailable".len());
        capability.admission = Admission::Ineligible {
            reason: "x".repeat(1024 * 1024),
        };
        crate::allocation_observer::arm_at_least(8192);
        let result = ProcessLedger::new(&policy, &capability);
        let observed = crate::allocation_observer::finish_observations();
        assert!(result.is_err());
        assert_eq!(observed.at_least_threshold, 0);
    }

    #[test]
    fn long_derived_payloads_have_one_registry_path_owner_and_refuse_before_copy() {
        let directory = std::env::temp_dir().join(format!(
            "molt-derived-budget-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let policy = test_policy(
            ClosureMode::DeclaredTree,
            vec![DerivedRoot {
                role: "derived".to_owned(),
                path: directory.clone(),
            }],
        );
        let capability = test_capability(policy.policy.mode);
        let mut ledger = ProcessLedger::new(&policy, &capability).unwrap();
        ledger
            .apply(&event(
                1,
                1,
                "root",
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            ))
            .unwrap();
        ledger
            .apply(&event(
                2,
                1,
                "root",
                ProcessEventKind::Exec {
                    image: test_root_image(&policy),
                },
            ))
            .unwrap();
        let path_bytes = BUDGET_PATH_UTF8_BYTES - 256;
        let prefix = policy.derived[0].path.join("").to_string_lossy().len();
        let filler = "x".repeat(path_bytes - prefix - 32);
        let mut sequence = 3;
        let mut accepted_wire = 0;
        let mut last_image = None;
        loop {
            // These synthetic observed identities exercise admitted protocol
            // sizes without requiring the host filesystem to create long paths.
            let image = policy.classify_observed_image(
                &policy.derived[0]
                    .path
                    .join(format!("{filler}-{:08}", ledger.derived.len())),
                "derived-id".to_owned(),
                17,
                "b".repeat(64),
            );
            let wire = serde_json::to_vec(&image).unwrap().len();
            let path_payload = image.path.as_os_str().len();
            let row = event(
                sequence,
                1,
                "root",
                ProcessEventKind::Exec {
                    image: image.clone(),
                },
            );
            let exceeds = accepted_wire + wire > BUDGET_RETAINED_DERIVED_IDENTITY_BYTES;
            let before = if exceeds {
                Some(ledger.snapshot())
            } else {
                None
            };
            let prior = (
                ledger.retained_bytes,
                ledger.derived_bytes,
                ledger.derived.len(),
            );
            crate::allocation_observer::arm_at_least(path_payload);
            let result = ledger.apply(&row);
            let observed = crate::allocation_observer::finish_observations();
            if exceeds {
                assert!(result.unwrap_err().contains("derived identity storage"));
                assert_eq!(
                    observed.at_least_threshold, 0,
                    "refusal copied a long path: {observed:?}"
                );
                assert_eq!(ledger.snapshot(), before.unwrap());
                assert_eq!(
                    (
                        ledger.retained_bytes,
                        ledger.derived_bytes,
                        ledger.derived.len()
                    ),
                    prior
                );
                assert!(BUDGET_RETAINED_DERIVED_IDENTITY_BYTES - accepted_wire < wire);
                break;
            }
            result.unwrap();
            // One live image and one persistent witness; a separate registry
            // key or a classification copy is a third proportional allocation.
            assert_eq!(
                observed.at_least_threshold, 2,
                "duplicate owned path: {observed:?}"
            );
            accepted_wire += wire;
            assert_eq!(ledger.derived_bytes, accepted_wire);
            assert!(owned_payload(&ledger) <= ledger.retained_bytes);
            last_image = Some(image);
            sequence += 1;
        }
        let image = last_image.unwrap();
        // Set equality is path-only. Drift must still compare ALL identity
        // fields: separate digest and file-id mutations must both be refused.
        for field in ["sha256", "file-id"] {
            let mut changed = image.clone();
            if field == "sha256" {
                changed.sha256 = "c".repeat(64);
            } else {
                changed.file_id = "changed-file-id".to_owned();
            }
            let before = ledger.snapshot();
            let row = event(
                sequence,
                1,
                "root",
                ProcessEventKind::Exec { image: changed },
            );
            crate::allocation_observer::arm_at_least(8192);
            let result = ledger.apply(&row);
            let observed = crate::allocation_observer::finish_observations();
            assert!(
                result
                    .unwrap_err()
                    .contains("derived image identity changed"),
                "{field}"
            );
            assert_eq!(observed.at_least_threshold, 0);
            assert_eq!(ledger.snapshot(), before);
        }
        // Existing derived identities consume no new registry budget. Fill
        // active inherited images to the actual aggregate payload boundary.
        let mut pid = 2;
        loop {
            let stable = format!("child-{pid}");
            let row = event(
                sequence,
                pid,
                &stable,
                ProcessEventKind::Fork {
                    parent_process_id: 1,
                    image: Some(image.clone()),
                },
            );
            let prior = (
                ledger.retained_bytes,
                ledger.derived_bytes,
                ledger.seen_stable_ids.len(),
                ledger.active_by_stable_id.len(),
                ledger.active_by_process_id.len(),
            );
            crate::allocation_observer::arm_at_least(image.path.as_os_str().len());
            let result = ledger.apply(&row);
            let observed = crate::allocation_observer::finish_observations();
            if let Err(error) = result {
                assert!(error.contains("retained observation storage"));
                assert_eq!(
                    observed.at_least_threshold, 0,
                    "aggregate refusal copied image"
                );
                assert_eq!(
                    (
                        ledger.retained_bytes,
                        ledger.derived_bytes,
                        ledger.seen_stable_ids.len(),
                        ledger.active_by_stable_id.len(),
                        ledger.active_by_process_id.len()
                    ),
                    prior
                );
                assert_eq!(ledger.accounting.process_creates, (pid - 1) as u64);
                assert_eq!(ledger.accounting.active_processes, (pid - 1) as u64);
                assert!(
                    BUDGET_RETAINED_OBSERVATION_PAYLOAD_BYTES - ledger.retained_bytes
                        < serde_json::to_vec(&row).unwrap().len() + 1024
                );
                break;
            }
            assert_eq!(observed.at_least_threshold, 1, "fork owns one live image");
            assert!(owned_payload(&ledger) <= ledger.retained_bytes);
            assert!(ledger.retained_bytes <= BUDGET_RETAINED_OBSERVATION_PAYLOAD_BYTES);
            sequence += 1;
            pid += 1;
        }
        // Exiting all active owners leaves the durable derived witness intact.
        for child in 2..pid {
            ledger
                .apply(&event(
                    sequence,
                    child,
                    &format!("child-{child}"),
                    ProcessEventKind::ProcessExit { exit_code: 0 },
                ))
                .unwrap();
            sequence += 1;
        }
        ledger
            .apply(&event(
                sequence,
                1,
                "root",
                ProcessEventKind::ProcessExit { exit_code: 0 },
            ))
            .unwrap();
        assert!(ledger.active_by_stable_id.is_empty());
        assert_eq!(ledger.derived_bytes, accepted_wire);
        assert!(ledger.snapshot().derived_images.contains(&image));
        assert!(owned_payload(&ledger) <= ledger.retained_bytes);
        drop(ledger);
        std::fs::remove_dir(directory).unwrap();
    }
}
