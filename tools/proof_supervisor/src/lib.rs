//! Kernel-backed process-image custody for Molt proof execution.
//!
//! This crate is intentionally workspace-neutral and protocol-first.  The
//! caller seals one policy before launch; the platform backend owns every
//! process event until the tree is quiescent and returns one terminal receipt.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub mod evidence;
pub mod image_cache;
pub mod platform;

pub use evidence::{ArtifactSummary, EventJournal, IdentitySummary, PublishedEvidence};
pub use image_cache::{ImageCacheKey, ImageHashCache};

pub const POLICY_SCHEMA: &str = "molt.proof-process-closure.v1";
pub const RECEIPT_SCHEMA: &str = "molt.proof-process-closure-receipt.v2";
const MAX_DIAGNOSTICS_PER_CLASS: usize = 16;
const MAX_DIAGNOSTIC_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClosureMode {
    Leaf,
    DeclaredTree,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixedImage {
    pub role: String,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixedAuthority {
    pub path: PathBuf,
    pub sha256: String,
    pub roles: BTreeSet<String>,
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
    pub fixed: BTreeMap<String, FixedAuthority>,
    pub derived: Vec<DerivedRoot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    ProcessCreate,
    ProcessExit,
    Fork,
    Exec,
    ThreadCreate,
    CloneUnclassified,
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
    pub kind: EventKind,
    pub process_id: u32,
    pub parent_process_id: Option<u32>,
    pub stable_process_id: String,
    pub image: Option<FileIdentity>,
    pub exit_code: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Accounting {
    pub total_processes: u64,
    pub active_processes: u64,
    pub observed_process_creates: u64,
    pub observed_process_exits: u64,
    pub observed_execs: u64,
    pub root_execs: u64,
    pub completion_port_new_processes: Option<u64>,
    pub completion_port_exits: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub schema: String,
    pub platform: String,
    pub mode: ClosureMode,
    pub backend: String,
    pub available: bool,
    pub pre_entry_exec_authority: bool,
    pub recursive_descendant_authority: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: String,
    pub platform: String,
    pub backend: String,
    pub policy_sha256: String,
    pub nonce_sha256: String,
    pub state: SupervisorState,
    pub lifecycle: Vec<SupervisorState>,
    pub event_log: Option<ArtifactSummary>,
    pub derived_image_summary: IdentitySummary,
    pub accounting: Accounting,
    pub violation_count: u64,
    pub violations: Vec<String>,
    pub error_count: u64,
    pub errors: Vec<String>,
    pub root_exit_code: Option<i64>,
    pub elapsed_ns: u128,
    pub complete: bool,
    pub identity_sha256: String,
}

impl Receipt {
    pub fn running(policy: &ValidatedPolicy, capability: &Capability) -> Self {
        let mut receipt = Self {
            schema: RECEIPT_SCHEMA.to_owned(),
            platform: capability.platform.clone(),
            backend: capability.backend.clone(),
            policy_sha256: policy.policy_sha256.clone(),
            nonce_sha256: sha256_bytes(policy.policy.nonce.as_bytes()),
            state: SupervisorState::Created,
            lifecycle: vec![SupervisorState::Created],
            event_log: None,
            derived_image_summary: IdentitySummary::empty(),
            accounting: Accounting::default(),
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
        reason: impl Into<String>,
    ) -> Self {
        let mut receipt = Self {
            schema: RECEIPT_SCHEMA.to_owned(),
            platform: capability.platform.clone(),
            backend: capability.backend.clone(),
            policy_sha256: policy.policy_sha256.clone(),
            nonce_sha256: sha256_bytes(policy.policy.nonce.as_bytes()),
            state: SupervisorState::Created,
            lifecycle: vec![SupervisorState::Created],
            event_log: None,
            derived_image_summary: IdentitySummary::empty(),
            accounting: Accounting::default(),
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

    pub fn attach_evidence(&mut self, evidence: PublishedEvidence) {
        self.event_log = Some(evidence.event_log);
        self.derived_image_summary = evidence.derived_images;
        self.seal();
    }

    pub fn record_violation(&mut self, value: impl Into<String>) {
        self.violation_count = self.violation_count.saturating_add(1);
        push_bounded_diagnostic(&mut self.violations, value.into());
    }

    pub fn record_error(&mut self, value: impl Into<String>) {
        self.error_count = self.error_count.saturating_add(1);
        push_bounded_diagnostic(&mut self.errors, value.into());
    }

    pub fn seal(&mut self) {
        self.identity_sha256.clear();
        let material = serde_json::to_vec(self).expect("receipt serialization is infallible");
        self.identity_sha256 = sha256_bytes(&material);
    }

    pub fn identity_is_valid(&self) -> bool {
        let expected = self.identity_sha256.as_bytes();
        let mut material = self.clone();
        material.identity_sha256.clear();
        let bytes = serde_json::to_vec(&material).expect("receipt serialization is infallible");
        constant_time_eq(expected, sha256_bytes(&bytes).as_bytes())
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
        let complete_consistent = !self.complete
            || (self.violation_count == 0
                && self.error_count == 0
                && self.accounting.active_processes == 0
                && self.accounting.root_execs >= 1
                && self.accounting.total_processes == self.accounting.observed_process_creates
                && self.accounting.observed_process_creates
                    == self.accounting.observed_process_exits);
        terminal
            && diagnostics_consistent
            && complete_consistent
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
}

fn push_bounded_diagnostic(values: &mut Vec<String>, mut value: String) {
    if values.len() >= MAX_DIAGNOSTICS_PER_CLASS {
        return;
    }
    if value.len() > MAX_DIAGNOSTIC_BYTES {
        let mut boundary = MAX_DIAGNOSTIC_BYTES.saturating_sub(3);
        while !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value.truncate(boundary);
        value.push_str("...");
    }
    values.push(value);
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

impl Policy {
    pub fn validate(self) -> Result<ValidatedPolicy, String> {
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
        if !self.cwd.is_absolute() || !Path::new(&self.command[0]).is_absolute() {
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
        if self.root_role.is_empty() {
            return Err("policy root_role must be non-empty".to_owned());
        }
        let cwd = canonical_directory(&self.cwd, "policy cwd")?;
        let mut fixed = BTreeMap::new();
        let mut normalized_images = Vec::with_capacity(self.fixed_images.len());
        for image in &self.fixed_images {
            if image.role.is_empty() {
                return Err("fixed image role must be non-empty".to_owned());
            }
            validate_digest(&image.sha256, "fixed image")?;
            if !image.path.is_absolute() {
                return Err("fixed image paths must be absolute".to_owned());
            }
            let path = canonical_file(&image.path, "fixed image")?;
            let actual = sha256_file(&path)
                .map_err(|error| format!("cannot hash fixed image {}: {error}", path.display()))?;
            if !constant_time_eq(
                actual.as_bytes(),
                image.sha256.to_ascii_lowercase().as_bytes(),
            ) {
                return Err(format!(
                    "fixed image digest mismatch for {}: policy={} actual={actual}",
                    path.display(),
                    image.sha256
                ));
            }
            let key = normalized_path_key(&path);
            let normalized = FixedImage {
                role: image.role.clone(),
                path: path.clone(),
                sha256: actual.clone(),
            };
            let authority = fixed.entry(key).or_insert_with(|| FixedAuthority {
                path,
                sha256: actual.clone(),
                roles: BTreeSet::new(),
            });
            if authority.sha256 != actual {
                return Err("one executable identity has conflicting fixed digests".to_owned());
            }
            authority.roles.insert(image.role.clone());
            normalized_images.push(normalized);
        }
        if fixed.is_empty() {
            return Err("policy must contain at least the root fixed image".to_owned());
        }
        let root_path = canonical_file(Path::new(&self.command[0]), "root command")?;
        let root_key = normalized_path_key(&root_path);
        let root = fixed
            .get(&root_key)
            .ok_or_else(|| "root command is outside fixed image authority".to_owned())?;
        if !root.roles.contains(&self.root_role) {
            return Err("root command role does not match root_role".to_owned());
        }
        let mut derived = Vec::with_capacity(self.derived_roots.len());
        let mut seen_roots = BTreeSet::new();
        for root in &self.derived_roots {
            if root.role.is_empty() {
                return Err("derived root role must be non-empty".to_owned());
            }
            if !root.path.is_absolute() {
                return Err("derived root paths must be absolute".to_owned());
            }
            let path = canonical_directory(&root.path, "derived root")?;
            let key = normalized_path_key(&path);
            if !seen_roots.insert(key) {
                return Err("policy has duplicate derived roots".to_owned());
            }
            if derived.iter().any(|prior: &DerivedRoot| {
                path_is_within(&path, &prior.path) || path_is_within(&prior.path, &path)
            }) {
                return Err("derived roots cannot overlap".to_owned());
            }
            derived.push(DerivedRoot {
                role: root.role.clone(),
                path,
            });
        }
        if self.mode == ClosureMode::Leaf && !derived.is_empty() {
            return Err("leaf closure cannot admit derived executable roots".to_owned());
        }

        let mut canonical = self;
        canonical.cwd = cwd;
        normalized_images.sort_by(|left, right| {
            normalized_path_key(&left.path)
                .cmp(&normalized_path_key(&right.path))
                .then_with(|| left.role.cmp(&right.role))
        });
        normalized_images.dedup();
        derived.sort_by(|left, right| {
            normalized_path_key(&left.path).cmp(&normalized_path_key(&right.path))
        });
        canonical.fixed_images = normalized_images;
        canonical.derived_roots = derived.clone();
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|error| format!("cannot serialize canonical policy: {error}"))?;
        Ok(ValidatedPolicy {
            policy: canonical,
            policy_sha256: sha256_bytes(&bytes),
            fixed,
            derived,
        })
    }
}

impl ValidatedPolicy {
    pub fn classify_path(
        &self,
        path: &Path,
        file_id: String,
        size_bytes: u64,
        sha256: String,
    ) -> FileIdentity {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let key = normalized_path_key(&canonical);
        if let Some(authority) = self.fixed.get(&key) {
            let matches = constant_time_eq(authority.sha256.as_bytes(), sha256.as_bytes());
            return FileIdentity {
                path: canonical,
                file_id,
                size_bytes,
                sha256,
                class: if matches {
                    ImageClass::Fixed
                } else {
                    ImageClass::Unknown
                },
                roles: if matches {
                    authority.roles.iter().cloned().collect()
                } else {
                    Vec::new()
                },
            };
        }
        for root in &self.derived {
            if path_is_within(&canonical, &root.path) {
                return FileIdentity {
                    path: canonical,
                    file_id,
                    size_bytes,
                    sha256,
                    class: ImageClass::Derived,
                    roles: vec![root.role.clone()],
                };
            }
        }
        FileIdentity {
            path: canonical,
            file_id,
            size_bytes,
            sha256,
            class: ImageClass::Unknown,
            roles: Vec::new(),
        }
    }
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    sha256_reader(&mut file)
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

pub fn normalized_path_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

pub fn path_is_within(path: &Path, root: &Path) -> bool {
    let path_key = normalized_path_key(path);
    let root_key = normalized_path_key(root);
    path_key == root_key || path_key.starts_with(&(root_key.trim_end_matches('/').to_owned() + "/"))
}

fn canonical_file(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve {label} {}: {error}", path.display()))?;
    if !canonical.is_file() {
        return Err(format!("{label} is not a file: {}", canonical.display()));
    }
    Ok(canonical)
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(path)
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
    fn path_containment_has_a_component_boundary() {
        let root = Path::new("/tmp/target");
        assert!(path_is_within(Path::new("/tmp/target/a"), root));
        assert!(!path_is_within(Path::new("/tmp/target-escape/a"), root));
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
            environment: BTreeMap::new(),
            root_role: "cargo".to_owned(),
            fixed_images: vec![
                FixedImage {
                    role: "cargo".to_owned(),
                    path: lexical_proxy.clone(),
                    sha256: digest.clone(),
                },
                FixedImage {
                    role: "rustc".to_owned(),
                    path: executable.clone(),
                    sha256: digest,
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
    fn receipt_identity_binds_terminal_material() {
        let policy = ValidatedPolicy {
            policy: Policy {
                schema: POLICY_SCHEMA.to_owned(),
                nonce: "a".repeat(32),
                mode: ClosureMode::Leaf,
                cwd: PathBuf::from("."),
                command: vec!["proof".to_owned()],
                environment: BTreeMap::new(),
                root_role: "root".to_owned(),
                fixed_images: Vec::new(),
                derived_roots: Vec::new(),
            },
            policy_sha256: "b".repeat(64),
            fixed: BTreeMap::new(),
            derived: Vec::new(),
        };
        let capability = Capability {
            schema: "molt.proof-supervisor-capability.v1".to_owned(),
            platform: "test".to_owned(),
            mode: ClosureMode::Leaf,
            backend: "test".to_owned(),
            available: false,
            pre_entry_exec_authority: false,
            recursive_descendant_authority: false,
            reason: Some("test".to_owned()),
        };
        let mut receipt = Receipt::rejected(&policy, &capability, "unavailable");
        receipt.attach_evidence(PublishedEvidence {
            event_log: ArtifactSummary {
                schema: evidence::EVENT_LOG_SCHEMA.to_owned(),
                file: "receipt.events.jsonl".to_owned(),
                count: 0,
                bytes: 0,
                sha256: sha256_bytes(b""),
            },
            derived_images: IdentitySummary::empty(),
        });
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
                environment: BTreeMap::new(),
                root_role: "root".to_owned(),
                fixed_images: Vec::new(),
                derived_roots: Vec::new(),
            },
            policy_sha256: "b".repeat(64),
            fixed: BTreeMap::new(),
            derived: Vec::new(),
        };
        let capability = Capability {
            schema: "molt.proof-supervisor-capability.v1".to_owned(),
            platform: "test".to_owned(),
            mode: ClosureMode::Leaf,
            backend: "test".to_owned(),
            available: false,
            pre_entry_exec_authority: false,
            recursive_descendant_authority: false,
            reason: Some("test".to_owned()),
        };
        let mut receipt = Receipt::rejected(&policy, &capability, "unavailable");
        for index in 0..1_000 {
            receipt.record_error(format!("error-{index}-{}", "x".repeat(4096)));
            receipt.record_violation(format!("violation-{index}-{}", "y".repeat(4096)));
        }
        receipt.attach_evidence(PublishedEvidence {
            event_log: ArtifactSummary {
                schema: evidence::EVENT_LOG_SCHEMA.to_owned(),
                file: "receipt.events.jsonl".to_owned(),
                count: 0,
                bytes: 0,
                sha256: sha256_bytes(b""),
            },
            derived_images: IdentitySummary::empty(),
        });
        assert_eq!(receipt.errors.len(), MAX_DIAGNOSTICS_PER_CLASS);
        assert_eq!(receipt.violations.len(), MAX_DIAGNOSTICS_PER_CLASS);
        assert_eq!(receipt.error_count, 1_001);
        assert_eq!(receipt.violation_count, 1_000);
        assert!(serde_json::to_vec_pretty(&receipt).unwrap().len() < evidence::MAX_RECEIPT_BYTES);
        assert!(receipt.identity_is_valid());
    }
}
