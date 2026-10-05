use crate::process_ledger::{ProcessLedger, ProcessLedgerSnapshot};
use crate::{
    Accounting, Capability, FileIdentity, ProcessEvent, ProcessEventKind, RecordOutcome,
    ValidatedPolicy, sha256_bytes,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub use crate::EVENT_LOG_SCHEMA;
pub const MAX_RECEIPT_BYTES: usize = 64 * 1024;
const MAX_EVENT_RECORD_BYTES: usize = 1024 * 1024;
const MAX_EVENT_LOG_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EVENT_RECORDS: u64 = 10_000_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSummary {
    pub schema: String,
    pub file: String,
    pub count: u64,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitySummary {
    pub count: u64,
    pub sha256: String,
}

impl IdentitySummary {
    pub fn empty() -> Self {
        Self {
            count: 0,
            sha256: sha256_bytes(b"[]"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedEvidence {
    pub event_log: ArtifactSummary,
    pub verified: VerifiedEventLog,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedEventLog {
    pub derived_images: IdentitySummary,
    pub accounting: Accounting,
    pub root_exit_code: Option<i64>,
    pub violation_count: u64,
    pub violations: Vec<String>,
    pub active_processes: BTreeSet<String>,
}

pub struct EventJournal {
    temporary_path: PathBuf,
    receipt_path: PathBuf,
    file: Option<BufWriter<File>>,
    buffer: Vec<u8>,
    digest: Sha256,
    count: u64,
    bytes: u64,
    ledger: ProcessLedger,
    terminal: Option<VerifiedEventLog>,
    published: bool,
}

impl EventJournal {
    pub fn create(
        receipt_path: &Path,
        policy: &ValidatedPolicy,
        capability: &Capability,
    ) -> Result<Self, String> {
        let ledger = ProcessLedger::new(policy, capability)?;
        let mut staging_name = receipt_path
            .file_name()
            .ok_or_else(|| "receipt path must name a file".to_owned())?
            .to_os_string();
        staging_name.push(".events");
        let staging_path = receipt_path.with_file_name(staging_name);
        let (temporary_path, file) = create_temporary_file(&staging_path)?;
        Ok(Self {
            temporary_path,
            receipt_path: receipt_path.to_path_buf(),
            file: Some(BufWriter::with_capacity(256 * 1024, file)),
            buffer: Vec::with_capacity(1024),
            digest: Sha256::new(),
            count: 0,
            bytes: 0,
            ledger,
            terminal: None,
            published: false,
        })
    }

    pub fn record(
        &mut self,
        process_id: u32,
        stable_process_id: String,
        event: ProcessEventKind,
    ) -> Result<RecordOutcome, String> {
        if self.terminal.is_some() {
            return Err("process event journal is already terminal".to_owned());
        }
        let sequence = self
            .count
            .checked_add(1)
            .ok_or_else(|| "process event journal count overflow".to_owned())?;
        let event = ProcessEvent {
            sequence,
            process_id,
            stable_process_id,
            event,
        };
        self.buffer.clear();
        serde_json::to_writer(&mut self.buffer, &event)
            .map_err(|error| format!("cannot serialize process event: {error}"))?;
        self.buffer.push(b'\n');
        if self.buffer.len() > MAX_EVENT_RECORD_BYTES {
            return Err(format!(
                "process event record exceeds {MAX_EVENT_RECORD_BYTES} bytes"
            ));
        }
        if self.count >= MAX_EVENT_RECORDS
            || self.bytes.saturating_add(self.buffer.len() as u64) > MAX_EVENT_LOG_BYTES
        {
            return Err("process event journal exceeds its bounded evidence budget".to_owned());
        }
        let outcome = self.ledger.apply(&event)?;
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| "process event journal is already finalized".to_owned())?;
        file.write_all(&self.buffer)
            .map_err(|error| format!("cannot append process event journal: {error}"))?;
        self.digest.update(&self.buffer);
        self.bytes = self
            .bytes
            .checked_add(self.buffer.len() as u64)
            .ok_or_else(|| "process event journal byte count overflow".to_owned())?;
        self.count = sequence;
        Ok(outcome)
    }

    pub fn verified(&mut self) -> Result<&VerifiedEventLog, String> {
        if self.terminal.is_none() {
            self.terminal = Some(verified_from_ledger(self.ledger.snapshot())?);
        }
        Ok(self
            .terminal
            .as_ref()
            .expect("terminal ledger was populated"))
    }

    pub fn publish(mut self) -> Result<PublishedEvidence, String> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| "process event journal is already finalized".to_owned())?;
        file.flush()
            .map_err(|error| format!("cannot flush process event journal: {error}"))?;
        file.get_ref()
            .sync_all()
            .map_err(|error| format!("cannot sync process event journal: {error}"))?;
        drop(file);
        let sha256 = crate::hex_lower(&self.digest.clone().finalize());
        let final_path = event_artifact_path(&self.receipt_path, &sha256)?;
        durable_replace(&self.temporary_path, &final_path)?;
        let file_name = final_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "receipt event artifact name is not UTF-8".to_owned())?
            .to_owned();
        let verified = self.verified()?.clone();
        let event_log = ArtifactSummary {
            schema: EVENT_LOG_SCHEMA.to_owned(),
            file: file_name,
            count: self.count,
            bytes: self.bytes,
            sha256,
        };
        self.published = true;
        Ok(PublishedEvidence {
            event_log,
            verified,
        })
    }
}

impl Drop for EventJournal {
    fn drop(&mut self) {
        if !self.published {
            self.file.take();
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}

pub fn event_artifact_path(receipt_path: &Path, sha256: &str) -> Result<PathBuf, String> {
    crate::validate_digest(sha256, "event log")?;
    let file_name = receipt_path
        .file_name()
        .ok_or_else(|| "receipt path must name a file".to_owned())?;
    let mut artifact_name = OsString::from(file_name);
    artifact_name.push(format!(".events.{sha256}.jsonl"));
    Ok(receipt_path.with_file_name(artifact_name))
}

pub fn verify_event_artifact(
    receipt_path: &Path,
    expected: &ArtifactSummary,
    policy: &ValidatedPolicy,
    capability: &Capability,
) -> Result<VerifiedEventLog, String> {
    if expected.schema != EVENT_LOG_SCHEMA {
        return Err(format!("event log schema must be {EVENT_LOG_SCHEMA}"));
    }
    crate::validate_digest(&expected.sha256, "event log")?;
    if expected.bytes > MAX_EVENT_LOG_BYTES || expected.count > MAX_EVENT_RECORDS {
        return Err("event log exceeds its bounded verification budget".to_owned());
    }
    let path = event_artifact_path(receipt_path, &expected.sha256)?;
    let expected_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "receipt event artifact name is not UTF-8".to_owned())?;
    if expected.file != expected_name {
        return Err("event log is not the deterministic adjacent artifact".to_owned());
    }
    let file = File::open(&path)
        .map_err(|error| format!("cannot open event log {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot stat event log {}: {error}", path.display()))?;
    if metadata.len() != expected.bytes {
        return Err(format!(
            "event log byte count mismatch: receipt={} actual={}",
            expected.bytes,
            metadata.len()
        ));
    }
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut line = Vec::new();
    let mut count = 0_u64;
    let mut actual_bytes = 0_u64;
    let mut previous_sequence = 0_u64;
    let mut ledger = ProcessLedger::new(policy, capability)?;
    loop {
        line.clear();
        let bytes = read_bounded_record(&mut reader, &mut line)
            .map_err(|error| format!("cannot read event log: {error}"))?;
        if bytes == 0 {
            break;
        }
        actual_bytes = actual_bytes
            .checked_add(bytes as u64)
            .ok_or_else(|| "event log byte count overflow".to_owned())?;
        if actual_bytes > expected.bytes || count >= expected.count {
            return Err("event log grew beyond its sealed evidence bounds".to_owned());
        }
        digest.update(&line);
        if line.last() != Some(&b'\n') {
            return Err("event log has a non-terminated final record".to_owned());
        }
        let event: ProcessEvent = serde_json::from_slice(&line[..line.len() - 1])
            .map_err(|error| format!("invalid process event record: {error}"))?;
        let expected_sequence = previous_sequence
            .checked_add(1)
            .ok_or_else(|| "event log sequence overflow".to_owned())?;
        if event.sequence != expected_sequence {
            return Err(format!(
                "event log sequence is not contiguous: expected {expected_sequence}, observed {}",
                event.sequence
            ));
        }
        previous_sequence = event.sequence;
        count = count
            .checked_add(1)
            .ok_or_else(|| "event log count overflow".to_owned())?;
        ledger.apply(&event)?;
    }
    if count != expected.count || actual_bytes != expected.bytes {
        return Err(format!(
            "event log record count mismatch: receipt={} actual={count}",
            expected.count
        ));
    }
    let actual_sha256 = crate::hex_lower(&digest.finalize());
    if !crate::constant_time_eq(expected.sha256.as_bytes(), actual_sha256.as_bytes()) {
        return Err("event log digest mismatch".to_owned());
    }
    verified_from_ledger(ledger.snapshot())
}

fn verified_from_ledger(snapshot: ProcessLedgerSnapshot) -> Result<VerifiedEventLog, String> {
    Ok(VerifiedEventLog {
        derived_images: summarize_identities(snapshot.derived_images)?,
        accounting: snapshot.accounting,
        root_exit_code: snapshot.root_exit_code,
        violation_count: snapshot.violation_count,
        violations: snapshot.violations,
        active_processes: snapshot.active_processes,
    })
}

fn read_bounded_record(reader: &mut impl BufRead, output: &mut Vec<u8>) -> io::Result<usize> {
    output.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(output.len());
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if output.len().saturating_add(consumed) > MAX_EVENT_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("event record exceeds {MAX_EVENT_RECORD_BYTES} bytes"),
            ));
        }
        let terminated = available.get(consumed - 1) == Some(&b'\n');
        output.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if terminated {
            return Ok(output.len());
        }
    }
}

pub fn durable_atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let (temporary_path, mut file) = create_temporary_file(path)?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        file.sync_all()
            .map_err(|error| format!("cannot sync {}: {error}", path.display()))?;
        drop(file);
        durable_replace(&temporary_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn summarize_identities(mut identities: Vec<FileIdentity>) -> Result<IdentitySummary, String> {
    identities.sort_by(|left, right| left.path.cmp(&right.path));
    let bytes = serde_json::to_vec(&identities)
        .map_err(|error| format!("cannot serialize derived image summary: {error}"))?;
    Ok(IdentitySummary {
        count: identities.len() as u64,
        sha256: sha256_bytes(&bytes),
    })
}

fn create_temporary_file(path: &Path) -> Result<(PathBuf, File), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create evidence directory {}: {error}",
            parent.display()
        )
    })?;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    for _ in 0..1024 {
        let suffix = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut name = path
            .file_name()
            .ok_or_else(|| "evidence path must name a file".to_owned())?
            .to_os_string();
        name.push(format!(".{}.{}.tmp", std::process::id(), suffix));
        let temporary = path.with_file_name(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create temporary evidence {}: {error}",
                    temporary.display()
                ));
            }
        }
    }
    Err("cannot allocate a unique temporary evidence path".to_owned())
}

#[cfg(unix)]
fn durable_replace(temporary: &Path, final_path: &Path) -> Result<(), String> {
    fs::rename(temporary, final_path).map_err(|error| {
        format!(
            "cannot publish evidence {} -> {}: {error}",
            temporary.display(),
            final_path.display()
        )
    })?;
    sync_parent_directory(final_path)
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "cannot sync evidence directory {}: {error}",
                parent.display()
            )
        })
}

#[cfg(windows)]
fn durable_replace(temporary: &Path, final_path: &Path) -> Result<(), String> {
    use molt_artifact_publish::windows_namespace_path_wide;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let old = windows_namespace_path_wide(temporary).map_err(|error| {
        format!(
            "cannot encode evidence path {}: {error}",
            temporary.display()
        )
    })?;
    let new = windows_namespace_path_wide(final_path).map_err(|error| {
        format!(
            "cannot encode evidence path {}: {error}",
            final_path.display()
        )
    })?;
    if unsafe {
        MoveFileExW(
            old.as_ptr(),
            new.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(format!(
            "cannot publish evidence {} -> {}: {}",
            temporary.display(),
            final_path.display(),
            io::Error::last_os_error()
        ));
    }
    sync_parent_directory(final_path)
}

#[cfg(windows)]
fn sync_parent_directory(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FlushFileBuffers, OPEN_EXISTING,
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = fs::canonicalize(parent).map_err(|error| {
        format!(
            "cannot canonicalize evidence directory {}: {error}",
            parent.display()
        )
    })?;
    let wide: Vec<u16> = canonical_parent
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "cannot open evidence directory {} for sync: {}",
            parent.display(),
            io::Error::last_os_error()
        ));
    }
    let flushed = unsafe { FlushFileBuffers(handle) };
    let flush_error = (flushed == 0).then(io::Error::last_os_error);
    unsafe {
        CloseHandle(handle);
    }
    if let Some(error) = flush_error {
        return Err(format!(
            "cannot sync evidence directory {}: {error}",
            parent.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CAPABILITY_SCHEMA, Capability, ClosureMode, DerivedRoot, FixedImage, ImageClass,
        POLICY_SCHEMA, Policy, ProcessEvent, ProcessEventKind, RootExitDisposition,
        ValidatedPolicy, sha256_file,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "molt-proof-supervisor-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn test_policy(mode: ClosureMode, derived_roots: Vec<DerivedRoot>) -> ValidatedPolicy {
        let executable = std::env::current_exe().unwrap();
        Policy {
            schema: POLICY_SCHEMA.to_owned(),
            nonce: "a".repeat(32),
            mode,
            cwd: std::env::current_dir().unwrap(),
            command: vec![executable.display().to_string()],
            environment: crate::platform::required_environment(),
            root_role: "test-root".to_owned(),
            fixed_images: vec![FixedImage {
                role: "test-root".to_owned(),
                path: executable.clone(),
                sha256: sha256_file(&executable).unwrap(),
                root_exit_disposition: RootExitDisposition::RequireExit,
            }],
            derived_roots,
        }
        .validate()
        .unwrap()
    }

    fn test_capability(mode: ClosureMode) -> Capability {
        Capability {
            schema: CAPABILITY_SCHEMA.to_owned(),
            platform: "linux".to_owned(),
            mode,
            backend: "ptrace-exitkill".to_owned(),
            available: true,
            pre_entry_exec_authority: true,
            pre_entry_process_create_authority: true,
            recursive_descendant_authority: true,
            required_environment: crate::platform::required_environment(),
            reason: None,
        }
    }

    fn test_root_image(policy: &ValidatedPolicy) -> FileIdentity {
        policy.classify_observed_image(
            &policy.root_path,
            "test-root".to_owned(),
            1,
            policy.fixed[&policy.root_path].sha256.clone(),
        )
    }

    fn assert_published_replay_matches(
        journal: EventJournal,
        receipt: &Path,
        policy: &ValidatedPolicy,
        capability: &Capability,
    ) -> PublishedEvidence {
        let published = journal.publish().unwrap();
        assert_eq!(
            verify_event_artifact(receipt, &published.event_log, policy, capability).unwrap(),
            published.verified
        );
        fs::remove_file(event_artifact_path(receipt, &published.event_log.sha256).unwrap())
            .unwrap();
        published
    }

    fn write_event_fixture(receipt: &Path, events: &[ProcessEvent]) -> ArtifactSummary {
        let mut bytes = Vec::new();
        for event in events {
            serde_json::to_writer(&mut bytes, event).unwrap();
            bytes.push(b'\n');
        }
        let sha256 = sha256_bytes(&bytes);
        let path = event_artifact_path(receipt, &sha256).unwrap();
        durable_atomic_write(&path, &bytes).unwrap();
        ArtifactSummary {
            schema: EVENT_LOG_SCHEMA.to_owned(),
            file: path.file_name().unwrap().to_string_lossy().into_owned(),
            count: events.len() as u64,
            bytes: bytes.len() as u64,
            sha256,
        }
    }

    #[test]
    fn event_journal_is_adjacent_durable_and_stream_verified() {
        let receipt = unique_path("receipt.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        journal
            .record(
                1,
                "test:1".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                    image: None,
                },
            )
            .unwrap();
        journal
            .record(
                1,
                "test:1".to_owned(),
                ProcessEventKind::ProcessExit { exit_code: 0 },
            )
            .unwrap();
        let published = journal.publish().unwrap();
        assert_eq!(published.event_log.count, 2);
        assert_eq!(published.verified.derived_images, IdentitySummary::empty());
        assert_eq!(
            verify_event_artifact(&receipt, &published.event_log, &policy, &capability)
                .unwrap()
                .derived_images,
            published.verified.derived_images
        );
        let _ =
            fs::remove_file(event_artifact_path(&receipt, &published.event_log.sha256).unwrap());
    }

    #[test]
    fn durable_atomic_write_replaces_complete_files() {
        let path = unique_path("atomic.json");
        durable_atomic_write(&path, b"first").unwrap();
        durable_atomic_write(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        let _ = fs::remove_file(path);
    }

    #[cfg(windows)]
    #[test]
    fn durable_publication_supports_verbatim_long_paths() {
        let root = unique_path("long-path");
        let mut directory = root.clone();
        while directory.as_os_str().len() < 300 {
            directory.push("proof-custody-segment");
        }
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("receipt.json");
        durable_atomic_write(&path, b"sealed").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"sealed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejected_root_creation_and_live_pid_reuse_preserve_the_accepted_prefix() {
        let receipt = unique_path("rejected-create.json");
        let policy = test_policy(ClosureMode::DeclaredTree, Vec::new());
        let mut capability = test_capability(policy.policy.mode);
        capability.platform = "windows".to_owned();
        capability.backend = "debug-process+nested-job".to_owned();
        let root_image = test_root_image(&policy);
        let wrong_root = policy.classify_observed_image(
            &policy.root_path.with_file_name("wrong-root"),
            "wrong-root".to_owned(),
            2,
            "b".repeat(64),
        );
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record(
                    1,
                    "test:1".to_owned(),
                    ProcessEventKind::ProcessCreate {
                        parent_process_id: None,
                        image: Some(wrong_root),
                    }
                )
                .unwrap_err()
                .contains("initial root image")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        assert_eq!(journal.count, 0);
        // Retrying the same stable identity also checks state omitted from the snapshot.
        journal
            .record(
                1,
                "test:1".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                    image: Some(root_image.clone()),
                },
            )
            .unwrap();

        let child = ProcessEventKind::ProcessCreate {
            parent_process_id: Some(1),
            image: Some(root_image.clone()),
        };
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record(1, "test:2".to_owned(), child.clone())
                .unwrap_err()
                .contains("reuses live process id")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        let mut misclassified = root_image;
        misclassified.roles.push("forged-role".to_owned());
        assert!(
            journal
                .record(
                    2,
                    "test:2".to_owned(),
                    ProcessEventKind::ProcessCreate {
                        parent_process_id: Some(1),
                        image: Some(misclassified),
                    }
                )
                .unwrap_err()
                .contains("classification disagrees")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal.record(2, "test:2".to_owned(), child).unwrap();
        for id in [2, 1] {
            journal
                .record(
                    id,
                    format!("test:{id}"),
                    ProcessEventKind::ProcessExit { exit_code: 0 },
                )
                .unwrap();
        }
        let published = assert_published_replay_matches(journal, &receipt, &policy, &capability);
        assert_eq!(published.event_log.count, 4);
        assert_eq!(published.verified.accounting.process_creates, 2);
        assert_eq!(published.verified.violation_count, 0);
    }

    #[test]
    fn fork_requires_the_exact_optional_live_parent_image_during_record_and_replay() {
        let receipt = unique_path("fork-inheritance.json");
        let auxiliary_path = unique_path("auxiliary-image");
        fs::write(&auxiliary_path, b"distinct auxiliary image").unwrap();
        let mut policy = test_policy(ClosureMode::DeclaredTree, Vec::new()).policy;
        policy.fixed_images.push(FixedImage {
            role: "auxiliary".to_owned(),
            path: auxiliary_path.clone(),
            sha256: sha256_file(&auxiliary_path).unwrap(),
            root_exit_disposition: RootExitDisposition::Terminate,
        });
        let policy = policy.validate().unwrap();
        let capability = test_capability(policy.policy.mode);
        let root_image = test_root_image(&policy);
        let auxiliary = policy
            .fixed
            .values()
            .find(|image| image.roles.contains("auxiliary"))
            .unwrap();
        let auxiliary_image = policy.classify_observed_image(
            &auxiliary.path,
            "test-auxiliary".to_owned(),
            24,
            auxiliary.sha256.clone(),
        );
        let mut wrong_file_id = root_image.clone();
        wrong_file_id.file_id.push_str("-forged");
        let mut wrong_size = root_image.clone();
        wrong_size.size_bytes += 1;
        let forged_images = [
            Some(auxiliary_image),
            None,
            Some(wrong_file_id),
            Some(wrong_size),
        ];
        let events: Vec<ProcessEvent> = [
            ProcessEventKind::ProcessCreate {
                parent_process_id: None,
                image: None,
            },
            ProcessEventKind::Exec {
                image: root_image.clone(),
            },
            ProcessEventKind::Fork {
                parent_process_id: 1,
                image: Some(root_image.clone()),
            },
            ProcessEventKind::ProcessExit { exit_code: 0 },
            ProcessEventKind::ProcessExit { exit_code: 0 },
        ]
        .into_iter()
        .enumerate()
        .map(|(index, event)| {
            let process_id = if index == 2 || index == 4 { 2 } else { 1 };
            ProcessEvent {
                sequence: index as u64 + 1,
                process_id,
                stable_process_id: format!("test:{process_id}"),
                event,
            }
        })
        .collect();
        for image in &forged_images {
            let mut forged = events.clone();
            forged[2].event = ProcessEventKind::Fork {
                parent_process_id: 1,
                image: image.clone(),
            };
            let summary = write_event_fixture(&receipt, &forged);
            assert!(
                verify_event_artifact(&receipt, &summary, &policy, &capability)
                    .unwrap_err()
                    .contains("inherited live parent image")
            );
            fs::remove_file(event_artifact_path(&receipt, &summary.sha256).unwrap()).unwrap();
        }

        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        journal
            .record(1, "test:1".to_owned(), events[0].event.clone())
            .unwrap();
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record(2, "pre-exec:2".to_owned(), events[2].event.clone())
                .unwrap_err()
                .contains("inherited live parent image")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record(
                2,
                "pre-exec:2".to_owned(),
                ProcessEventKind::Fork {
                    parent_process_id: 1,
                    image: None,
                },
            )
            .unwrap();
        journal
            .record(
                2,
                "pre-exec:2".to_owned(),
                ProcessEventKind::ProcessExit { exit_code: 0 },
            )
            .unwrap();
        journal
            .record(1, "test:1".to_owned(), events[1].event.clone())
            .unwrap();
        let before = journal.ledger.snapshot();
        for image in forged_images {
            assert!(
                journal
                    .record(
                        2,
                        "test:2".to_owned(),
                        ProcessEventKind::Fork {
                            parent_process_id: 1,
                            image,
                        }
                    )
                    .unwrap_err()
                    .contains("inherited live parent image")
            );
            assert_eq!(journal.ledger.snapshot(), before);
        }
        journal
            .record(2, "test:2".to_owned(), events[2].event.clone())
            .unwrap();
        assert!(
            journal
                .record(1, "test:1".to_owned(), events[3].event.clone())
                .unwrap()
                .has_policy_violation()
        );
        journal
            .record(2, "test:2".to_owned(), events[4].event.clone())
            .unwrap();
        let published = assert_published_replay_matches(journal, &receipt, &policy, &capability);
        assert_eq!(published.verified.violation_count, 1);
        assert_eq!(
            published.verified.accounting.root_exit_terminated_processes,
            0
        );
        assert_eq!(published.verified.accounting.active_processes, 0);
        fs::remove_file(auxiliary_path).unwrap();
    }

    #[test]
    fn derived_identity_drift_is_rejected_during_record_and_replay() {
        let receipt = unique_path("derived-drift.json");
        let derived_root = unique_path("derived-root");
        fs::create_dir_all(&derived_root).unwrap();
        let policy = test_policy(
            ClosureMode::DeclaredTree,
            vec![DerivedRoot {
                role: "generated-tool".to_owned(),
                path: derived_root.clone(),
            }],
        );
        let derived_path = policy.derived[0].path.join("derived-tool");
        let image = |sha256: &str| FileIdentity {
            path: derived_path.clone(),
            file_id: "test-derived".to_owned(),
            size_bytes: 1,
            sha256: sha256.to_owned(),
            class: ImageClass::Derived,
            roles: vec!["generated-tool".to_owned()],
        };
        let root_image = test_root_image(&policy);
        let events = vec![
            ProcessEvent {
                sequence: 1,
                process_id: 1,
                stable_process_id: "test:1".to_owned(),
                event: ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                    image: None,
                },
            },
            ProcessEvent {
                sequence: 2,
                process_id: 1,
                stable_process_id: "test:1".to_owned(),
                event: ProcessEventKind::Exec { image: root_image },
            },
            ProcessEvent {
                sequence: 3,
                process_id: 1,
                stable_process_id: "test:1".to_owned(),
                event: ProcessEventKind::Exec {
                    image: image(&"a".repeat(64)),
                },
            },
            ProcessEvent {
                sequence: 4,
                process_id: 1,
                stable_process_id: "test:1".to_owned(),
                event: ProcessEventKind::Exec {
                    image: image(&"b".repeat(64)),
                },
            },
        ];

        let capability = test_capability(policy.policy.mode);
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        journal
            .record(1, "test:1".to_owned(), events[0].event.clone())
            .unwrap();
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record(1, "test:1".to_owned(), events[2].event.clone())
                .unwrap_err()
                .contains("initial root image")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record(1, "test:1".to_owned(), events[1].event.clone())
            .unwrap();
        journal
            .record(1, "test:1".to_owned(), events[2].event.clone())
            .unwrap();
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record(1, "test:1".to_owned(), events[3].event.clone())
                .unwrap_err()
                .contains("identity changed")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record(
                2,
                "test:2".to_owned(),
                ProcessEventKind::Fork {
                    parent_process_id: 1,
                    image: Some(image(&"a".repeat(64))),
                },
            )
            .unwrap();
        for id in [2, 1] {
            journal
                .record(
                    id,
                    format!("test:{id}"),
                    ProcessEventKind::ProcessExit { exit_code: 0 },
                )
                .unwrap();
        }
        let published = assert_published_replay_matches(journal, &receipt, &policy, &capability);
        assert_eq!(published.event_log.count, 6);
        assert_eq!(published.verified.derived_images.count, 1);

        let summary = write_event_fixture(&receipt, &events);
        assert!(
            verify_event_artifact(&receipt, &summary, &policy, &capability)
                .unwrap_err()
                .contains("identity changed")
        );
        let _ = fs::remove_file(event_artifact_path(&receipt, &summary.sha256).unwrap());
        let _ = fs::remove_dir_all(derived_root);
    }

    #[test]
    fn verifier_rejects_an_oversized_event_record_without_unbounded_read() {
        let receipt = unique_path("oversized-event.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let mut bytes = vec![b'x'; MAX_EVENT_RECORD_BYTES + 1];
        bytes.push(b'\n');
        let sha256 = sha256_bytes(&bytes);
        let path = event_artifact_path(&receipt, &sha256).unwrap();
        durable_atomic_write(&path, &bytes).unwrap();
        let summary = ArtifactSummary {
            schema: EVENT_LOG_SCHEMA.to_owned(),
            file: path.file_name().unwrap().to_string_lossy().into_owned(),
            count: 1,
            bytes: bytes.len() as u64,
            sha256,
        };
        assert!(
            verify_event_artifact(&receipt, &summary, &policy, &capability)
                .unwrap_err()
                .contains("exceeds")
        );
        let _ = fs::remove_file(path);
    }
}
