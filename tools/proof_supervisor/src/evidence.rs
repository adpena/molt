use crate::process_ledger::{ProcessLedger, ProcessLedgerSnapshot};
use crate::{
    Accounting, Admission, Capability, FileIdentity, ProcessEvent, ProcessEventKind, RecordOutcome,
    ValidatedPolicy, sha256_bytes,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Take, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub use crate::EVENT_LOG_SCHEMA;
pub const MAX_RECEIPT_BYTES: usize = crate::BUDGET_RECEIPT_BYTES;
const MAX_EVENT_RECORD_BYTES: usize = crate::BUDGET_EVENT_RECORD_BYTES;
const MAX_EVENT_LOG_BYTES: u64 = crate::BUDGET_EVENT_LOG_BYTES as u64;
pub(crate) const MAX_EVENT_RECORDS: u64 = crate::BUDGET_EVENT_RECORDS as u64;

/// One direct regular-file generation for supervisor input readers. The open
/// cannot wait for a FIFO writer; byte consumers are bounded by its admitted
/// extent, and verification checks both the retained handle and current name.
pub struct OpenedRegularFile {
    file: File,
    path: PathBuf,
    bytes: u64,
    key: crate::ImageCacheKey,
}

impl OpenedRegularFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        let path = {
            use std::os::windows::ffi::OsStringExt;
            let wide = molt_artifact_publish::windows_namespace_path_wide(path)?;
            PathBuf::from(OsString::from_wide(&wide[..wide.len() - 1]))
        };
        #[cfg(not(windows))]
        let path = path.to_path_buf();
        let direct = fs::symlink_metadata(&path)?;
        if !direct.is_file() || crate::redirecting_metadata(&direct) {
            return Err(io::Error::other("input must be a direct regular file"));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || crate::redirecting_metadata(&metadata) {
            return Err(io::Error::other(
                "opened input must be a direct regular file",
            ));
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
            if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
                return Err(io::Error::other("opened input must be a disk file"));
            }
        }
        let bytes = metadata.len();
        if bytes == u64::MAX {
            return Err(io::Error::other("regular input extent cannot be bounded"));
        }
        let key = crate::image_cache::opened_file_key(&file)?;
        if file.metadata()?.len() != bytes {
            return Err(io::Error::other("regular input changed during admission"));
        }
        Ok(Self {
            file,
            path,
            bytes,
            key,
        })
    }

    pub fn file(&self) -> &File {
        &self.file
    }

    pub fn size_bytes(&self) -> u64 {
        self.bytes
    }

    pub fn bounded_reader(&self) -> Take<&File> {
        (&self.file).take(self.bytes + 1)
    }

    pub fn rewind(&self) -> io::Result<()> {
        use std::io::Seek;
        let mut file = &self.file;
        file.rewind()
    }

    pub fn verify(&self) -> io::Result<()> {
        let after = crate::image_cache::opened_file_key(&self.file)?;
        let named = Self::open(&self.path)?;
        if self.key != after || after != named.key || self.bytes != named.bytes {
            return Err(io::Error::other(
                "regular input generation changed while reading",
            ));
        }
        Ok(())
    }

    pub fn read_all(&self, limit: usize) -> io::Result<Vec<u8>> {
        if self.bytes > limit as u64 {
            return Err(io::Error::other("regular input exceeds its byte budget"));
        }
        let mut stream = self.bounded_reader();
        let extent =
            usize::try_from(self.bytes).map_err(|_| io::Error::other("input extent overflow"))?;
        let mut bytes = crate::budget::BoundedBuffer::new(
            extent
                .checked_add(1)
                .ok_or_else(|| io::Error::other("input extent overflow"))?,
        );
        let mut chunk = [0_u8; 16 * 1024];
        loop {
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            bytes.write_all(&chunk[..count])?;
            if bytes.len() > extent {
                return Err(io::Error::other(
                    "regular input grew or shrank while reading",
                ));
            }
        }
        if bytes.len() != extent {
            return Err(io::Error::other(
                "regular input grew or shrank while reading",
            ));
        }
        self.verify()?;
        Ok(bytes.into_vec())
    }
}

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
    pub admission: Admission,
    pub derived_images: IdentitySummary,
    pub accounting: Accounting,
    pub root_exit_code: Option<i64>,
    pub violation_count: u64,
    pub violations: Vec<String>,
    pub active_processes: BTreeSet<String>,
}

pub struct EventJournal<'policy> {
    temporary_path: PathBuf,
    receipt_path: PathBuf,
    file: Option<File>,
    failure: Option<String>,
    coverage: crate::JournalCoverage,
    buffer: crate::budget::BoundedBuffer,
    digest: Sha256,
    count: u64,
    bytes: u64,
    ledger: ProcessLedger<'policy>,
    terminal: Option<VerifiedEventLog>,
    published: bool,
    #[cfg(test)]
    read_only_after: Option<(u64, File)>,
}

impl<'policy> EventJournal<'policy> {
    pub fn create(
        receipt_path: &Path,
        policy: &'policy ValidatedPolicy,
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
            file: Some(file),
            failure: None,
            coverage: crate::JournalCoverage::Full {},
            buffer: crate::budget::BoundedBuffer::new(MAX_EVENT_RECORD_BYTES),
            digest: Sha256::new(),
            count: 0,
            bytes: 0,
            ledger,
            terminal: None,
            published: false,
            #[cfg(test)]
            read_only_after: None,
        })
    }

    pub fn record(
        &mut self,
        process_id: u32,
        stable_process_id: String,
        event: ProcessEventKind,
    ) -> Result<RecordOutcome, String> {
        if let crate::JournalCoverage::Prefix { cause, .. } = &self.coverage {
            return Err(format!("event capture already cut off: {cause}"));
        }
        let result =
            self.record_with_append(process_id, stable_process_id, event, |file, bytes| {
                file.write_all(bytes)
            });
        if let Err(error) = &result {
            self.cutoff(
                if self.failure.is_some() {
                    crate::CaptureStage::JournalAppend
                } else {
                    crate::CaptureStage::JournalPreparation
                },
                error,
            );
        }
        result
    }

    /// Freeze the exact accepted prefix when the native owner cannot publish
    /// an actual observation. Later cleanup changes custody, never this prefix.
    pub fn cutoff(&mut self, stage: crate::CaptureStage, error: &str) {
        if !matches!(self.coverage, crate::JournalCoverage::Full {}) {
            return;
        }
        let cause =
            crate::bounded_diagnostic(format_args!("{error}")).unwrap_or_else(|refusal| refusal);
        self.coverage = crate::JournalCoverage::Prefix {
            stage,
            next_sequence: self.count + 1,
            accepted_records: self.count,
            accepted_bytes: self.bytes,
            accepted_sha256: crate::hex_lower(&self.digest.clone().finalize()),
            cause,
        };
    }

    pub fn coverage(&self) -> &crate::JournalCoverage {
        &self.coverage
    }

    /// Transaction tests exercise rejection without claiming it was an actual
    /// omitted platform observation. Real producer calls use record/cutoff.
    #[cfg(test)]
    fn record_transition(
        &mut self,
        pid: u32,
        stable: String,
        event: ProcessEventKind,
    ) -> Result<RecordOutcome, String> {
        self.record_with_append(pid, stable, event, |file, bytes| file.write_all(bytes))
    }

    /// Library-test fault boundary: the next write after this accepted prefix
    /// uses a real read-only handle to the same staging file. It exists only
    /// in test builds; production has no fault flag or alternate writer lane.
    #[cfg(test)]
    pub(crate) fn refuse_writes_after(&mut self, accepted_records: u64) -> io::Result<()> {
        assert!(accepted_records >= self.count);
        self.read_only_after = Some((accepted_records, File::open(&self.temporary_path)?));
        Ok(())
    }

    fn record_with_append(
        &mut self,
        process_id: u32,
        stable_process_id: String,
        event: ProcessEventKind,
        append: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    ) -> Result<RecordOutcome, String> {
        if let Some(failure) = &self.failure {
            return Err(format!("process event journal is poisoned: {failure}"));
        }
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
        if self.count >= MAX_EVENT_RECORDS || self.bytes >= MAX_EVENT_LOG_BYTES {
            return Err("process event journal exceeds its bounded evidence budget".to_owned());
        }
        // Prepare every owned delta/index reservation before output storage.
        let prepared = self.ledger.prepare(&event)?;
        let remaining =
            (MAX_EVENT_LOG_BYTES - self.bytes).min(MAX_EVENT_RECORD_BYTES as u64) as usize;
        self.buffer.reset(remaining);
        serde_json::to_writer(&mut self.buffer, &event)
            .map_err(|error| format!("cannot serialize bounded process event: {error}"))?;
        self.buffer
            .write_all(b"\n")
            .map_err(|error| error.to_string())?;
        let next_bytes = self
            .bytes
            .checked_add(self.buffer.len() as u64)
            .ok_or_else(|| "process event journal byte count overflow".to_owned())?;
        #[cfg(test)]
        if self
            .read_only_after
            .as_ref()
            .is_some_and(|(count, _)| self.count == *count)
        {
            let (_, file) = self.read_only_after.take().expect("fault boundary checked");
            self.file = Some(file);
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| "process event journal is already finalized".to_owned())?;
        if let Err(error) = append(file, self.buffer.as_slice()) {
            drop(prepared);
            return Err(self.poison(format!(
                "cannot append process event {sequence}: {error}; accepted prefix {} records/{} bytes; attempted record {} bytes/sha256 {}",
                self.count, self.bytes, self.buffer.len(), sha256_bytes(self.buffer.as_slice()),
            )));
        }
        // File::write_all has accepted every byte into the kernel, not a
        // userspace BufWriter. Durability still requires final publication.
        // No fallible transition remains after append and before commit.
        let outcome = prepared.commit();
        self.digest.update(self.buffer.as_slice());
        self.bytes = next_bytes;
        self.count = sequence;
        Ok(outcome)
    }

    fn poison(&mut self, error: String) -> String {
        if self.failure.is_none() {
            self.failure = Some(format!(
                "{error}; incomplete journal staging path {}",
                self.temporary_path.display(),
            ));
        }
        // File Drop has no buffered write retry. Keep any partial bytes on
        // disk as diagnostic evidence; they can never be published as a log.
        self.file.take();
        self.failure.as_ref().expect("failure recorded").clone()
    }

    pub fn verified(&mut self) -> Result<&VerifiedEventLog, String> {
        // This is the accepted file-write prefix for terminal diagnostics.
        // Publication separately refuses a poisoned journal and requires its
        // sync/rename/directory-sync boundary before issuing an artifact.
        if self.terminal.is_none() {
            self.terminal = Some(verified_from_ledger(self.ledger.snapshot())?);
        }
        Ok(self
            .terminal
            .as_ref()
            .expect("terminal ledger was populated"))
    }

    pub fn publish(self) -> Result<PublishedEvidence, String> {
        self.publish_with(|file| file.sync_all(), durable_replace)
    }

    fn publish_with(
        mut self,
        sync_file: impl FnOnce(&File) -> io::Result<()>,
        replace: impl FnOnce(&Path, &Path) -> Result<(), String>,
    ) -> Result<PublishedEvidence, String> {
        if let Some(failure) = &self.failure {
            return Err(format!("cannot publish poisoned event journal: {failure}"));
        }
        let result = (|| {
            let verified = self.verified()?.clone();
            let sha256 = crate::hex_lower(&self.digest.clone().finalize());
            let final_path = event_artifact_path(&self.receipt_path, &sha256)?;
            let file_name = final_path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| "receipt event artifact name is not UTF-8".to_owned())?
                .to_owned();
            let file = self
                .file
                .as_ref()
                .ok_or_else(|| "process event journal is already finalized".to_owned())?;
            let actual_bytes = file
                .metadata()
                .map_err(|error| format!("cannot stat process event journal: {error}"))?
                .len();
            if actual_bytes != self.bytes {
                return Err(format!(
                    "process event journal length changed: expected {} actual {actual_bytes}",
                    self.bytes
                ));
            }
            sync_file(file)
                .map_err(|error| format!("cannot sync process event journal: {error}"))?;
            // Windows replacement also requires relinquishing the open file.
            self.file.take();
            replace(&self.temporary_path, &final_path).map_err(|error| {
                format!(
                    "{error}; event artifact destination {} (publication not acknowledged)",
                    final_path.display(),
                )
            })?;
            let event_log = ArtifactSummary {
                schema: EVENT_LOG_SCHEMA.to_owned(),
                file: file_name,
                count: self.count,
                bytes: self.bytes,
                sha256,
            };
            Ok(PublishedEvidence {
                event_log,
                verified,
            })
        })();
        match result {
            Ok(evidence) => {
                self.published = true;
                Ok(evidence)
            }
            Err(error) => Err(self.poison(error)),
        }
    }
}

impl Drop for EventJournal<'_> {
    fn drop(&mut self) {
        self.file.take();
        if !self.published && self.failure.is_none() {
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
    let opened = OpenedRegularFile::open(&path)
        .map_err(|error| format!("cannot open event log {}: {error}", path.display()))?;
    if opened.size_bytes() != expected.bytes {
        return Err(format!(
            "event log byte count mismatch: receipt={} actual={}",
            expected.bytes,
            opened.size_bytes()
        ));
    }
    let mut reader = BufReader::new(opened.bounded_reader());
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
    opened
        .verify()
        .map_err(|error| format!("event log changed: {error}"))?;
    verified_from_ledger(ledger.snapshot())
}

fn verified_from_ledger(snapshot: ProcessLedgerSnapshot) -> Result<VerifiedEventLog, String> {
    Ok(VerifiedEventLog {
        admission: snapshot.admission,
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
        let next = output.len() + consumed;
        if next > output.capacity() {
            output
                .try_reserve_exact(
                    next.max(output.capacity().saturating_mul(2))
                        .min(MAX_EVENT_RECORD_BYTES)
                        - output.len(),
                )
                .map_err(|_| io::Error::other("event replay buffer reservation refused"))?;
        }
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
    result.map_err(|error| {
        // A failed write/sync may have left partial evidence; a failed
        // directory sync may already have renamed it. Preserve either state
        // and identify both paths without acknowledging durable publication.
        format!(
            "{error}; publication not acknowledged; staging path {}; destination {}",
            temporary_path.display(),
            path.display(),
        )
    })
}

fn summarize_identities(mut identities: Vec<FileIdentity>) -> Result<IdentitySummary, String> {
    identities.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(IdentitySummary {
        count: identities.len() as u64,
        sha256: crate::budget::digest(
            &identities,
            crate::BUDGET_RETAINED_DERIVED_IDENTITY_BYTES
                + crate::BUDGET_INVENTORY_UNIQUE_IMAGES
                + 2,
        )?,
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
pub(crate) mod tests {
    use super::*;
    use crate::{
        CAPABILITY_SCHEMA, Capability, ClosureMode, DerivedRoot, FixedImage, ImageClass,
        POLICY_SCHEMA, Policy, ProcessEvent, ProcessEventKind, RootExitDisposition,
        ValidatedPolicy, sha256_file,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn bounded_file_owner_refuses_growth_and_named_generation_substitution() {
        let path = unique_path("retained-generation.json");
        fs::write(&path, b"abcd").unwrap();
        assert_eq!(read_bounded_file(&path, 4).unwrap(), b"abcd");
        assert!(read_bounded_file(&path, 3).is_err());
        let opened = OpenedRegularFile::open(&path).unwrap();
        let replacement = unique_path("retained-replacement.json");
        fs::write(&replacement, b"wxyz").unwrap();
        // Same byte count, different real generation; content equality or an
        // aggregate length bound is not a substitute for retained ownership.
        #[cfg(unix)]
        {
            fs::rename(&replacement, &path).unwrap();
            assert!(opened.verify().is_err());
        }
        #[cfg(windows)]
        {
            // The platform can deny replacing an open generation; mutate via
            // an admitted writer instead and require the token fence to fail.
            fs::write(&path, b"longer").unwrap();
            assert!(opened.verify().is_err());
            fs::remove_file(&replacement).unwrap();
        }
        drop(opened);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_event_never_reaches_the_file_or_changes_the_accepted_prefix() {
        let receipt = unique_path("invalid-append.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        let before = journal.ledger.snapshot();
        let error = journal
            .record_with_append(
                41,
                "owned-root".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: Some(42),
                },
                |_, _| panic!("semantic rejection must precede every file write"),
            )
            .unwrap_err();
        assert!(error.contains("root process creation names a parent"));
        assert_eq!(journal.ledger.snapshot(), before);
        assert_eq!(fs::read(&journal.temporary_path).unwrap(), b"");
        assert_eq!((journal.count, journal.bytes), (0, 0));
        assert!(journal.failure.is_none());
        journal
            .record_transition(
                41,
                "owned-root".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            )
            .unwrap();
        journal
            .record_transition(
                41,
                "owned-root".to_owned(),
                ProcessEventKind::ProcessExit { exit_code: 0 },
            )
            .unwrap();
        let published = journal.publish().unwrap();
        let replay =
            verify_event_artifact(&receipt, &published.event_log, &policy, &capability).unwrap();
        assert_eq!(replay.accounting.process_creates, 1);
        assert_eq!(replay.accounting.process_exits, 1);
        assert_eq!(published.event_log.count, 2);
        fs::remove_file(event_artifact_path(&receipt, &published.event_log.sha256).unwrap())
            .unwrap();
    }

    #[test]
    fn accepted_append_commits_without_any_allocator_growth() {
        let receipt = unique_path("commit-allocation.json");
        let derived_root = unique_path("commit-derived");
        fs::create_dir(&derived_root).unwrap();
        let policy = test_policy(
            ClosureMode::DeclaredTree,
            vec![DerivedRoot {
                role: "derived".to_owned(),
                path: derived_root.clone(),
            }],
        );
        let capability = test_capability(policy.policy.mode);
        let fixed = test_root_image(&policy);
        let derived = policy.classify_observed_image(
            &policy.derived[0].path.join("first-tool"),
            "derived-file".to_owned(),
            17,
            "b".repeat(64),
        );
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        let mut rows = vec![
            (
                1,
                "root",
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            ),
            (
                1,
                "root",
                ProcessEventKind::Exec {
                    image: fixed.clone(),
                },
            ),
            (
                2,
                "child",
                ProcessEventKind::Fork {
                    parent_process_id: 1,
                    image: Some(fixed.clone()),
                },
            ),
            // First derived insertion owns both a live image and its durable
            // registry witness; the next fork owns another live image only.
            (
                2,
                "child",
                ProcessEventKind::Exec {
                    image: derived.clone(),
                },
            ),
            (
                3,
                "grandchild",
                ProcessEventKind::Fork {
                    parent_process_id: 2,
                    image: Some(derived.clone()),
                },
            ),
            (2, "child", ProcessEventKind::Exec { image: fixed }),
        ];
        for index in 0..crate::MAX_DIAGNOSTICS_PER_CLASS {
            rows.push((
                2,
                "child",
                ProcessEventKind::CloneUnclassified {
                    parent_process_id: 2,
                    reason: format!("kernel creation classification unavailable {index}"),
                },
            ));
        }
        rows.extend([
            (
                3,
                "grandchild",
                ProcessEventKind::ProcessExit { exit_code: 0 },
            ),
            (2, "child", ProcessEventKind::ProcessExit { exit_code: 0 }),
            (1, "root", ProcessEventKind::ProcessExit { exit_code: 0 }),
        ]);
        let expected_count = rows.len() as u64;
        let mut diagnostic_growth_boundaries = 0;
        for (pid, stable, event) in rows {
            let before_capacity = journal.ledger.diagnostic_storage().1;
            let outcome =
                journal.record_with_append(pid, stable.to_owned(), event, |file, bytes| {
                    file.write_all(bytes)?;
                    crate::allocation_observer::arm();
                    Ok(())
                });
            let growths = crate::allocation_observer::finish();
            assert_eq!(growths, 0, "accepted record allocated while committing");
            outcome.unwrap();
            if journal.ledger.diagnostic_storage().1 > before_capacity {
                diagnostic_growth_boundaries += 1;
            }
        }
        assert!(
            diagnostic_growth_boundaries >= 2,
            "fixture must exercise first and subsequent diagnostic capacity growth"
        );
        let accepted = journal.ledger.snapshot();
        assert_eq!(accepted.derived_images, vec![derived]);
        assert_eq!(
            accepted.violation_count,
            crate::MAX_DIAGNOSTICS_PER_CLASS as u64
        );
        assert_eq!(accepted.violations.len(), crate::MAX_DIAGNOSTICS_PER_CLASS);
        assert_eq!(journal.count, expected_count);
        let published = journal.publish().unwrap();
        fs::remove_file(event_artifact_path(&receipt, &published.event_log.sha256).unwrap())
            .unwrap();
        fs::remove_dir(derived_root).unwrap();
    }

    #[test]
    fn aggregate_diagnostic_refusal_preserves_real_prefix_and_exact_fit_is_admitted() {
        let policy = test_policy(ClosureMode::InventoryTree, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let receipt = unique_path("diagnostic-aggregate.json");
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            )
            .unwrap();
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::Exec {
                    image: test_root_image(&policy),
                },
            )
            .unwrap();
        let image_for = |length: usize| {
            policy.classify_observed_image(
                &policy
                    .root_path
                    .with_file_name(format!("observed-{}", "x".repeat(length))),
                "observed".to_owned(),
                17,
                "b".repeat(64),
            )
        };
        let full_length =
            crate::BUDGET_PATH_UTF8_BYTES - policy.root_path.to_string_lossy().len() - 1024;
        let full = image_for(full_length);
        let full_wire = serde_json::to_vec(&full).unwrap().len();
        assert_eq!(full.class, ImageClass::Unknown);
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::Exec {
                    image: full.clone(),
                },
            )
            .unwrap();
        // Independent expected ledger debit: creation identity allowance,
        // separately retained root admission ID, and actual serialized images.
        let mut expected = 4 * "root".len() + 256 + "root".len() + full_wire;
        let limit = crate::BUDGET_RETAINED_OBSERVATION_PAYLOAD_BYTES;
        let sample_charge = crate::BUDGET_COMBINED_DIAGNOSTICS_JSON_BYTES
            / (2 * crate::BUDGET_DIAGNOSTICS_PER_CLASS);
        let mut pid = 2;
        loop {
            let stable = format!("child-{pid}");
            let charge = 4 * stable.len() + 256 + full_wire;
            if expected + charge > limit {
                break;
            }
            journal
                .record(
                    pid,
                    stable,
                    ProcessEventKind::Fork {
                        parent_process_id: 1,
                        image: Some(full.clone()),
                    },
                )
                .unwrap();
            expected += charge;
            pid += 1;
        }
        assert!(pid > 3);
        // Free exactly enough image payload to admit one more inherited image
        // and leave sample_charge - 1. The writer/ledger helper is not used to
        // calculate expected sizes or this tuning amount.
        let next_stable = format!("child-{pid}");
        let next_charge = 4 * next_stable.len() + 256 + full_wire;
        let mut reduction = next_charge - (limit - expected) + sample_charge - 1;
        for child in 2..pid {
            if reduction == 0 {
                break;
            }
            let reduce = reduction.min(full_length - 1);
            let smaller = image_for(full_length - reduce);
            let smaller_wire = serde_json::to_vec(&smaller).unwrap().len();
            assert_eq!(full_wire - smaller_wire, reduce);
            journal
                .record(
                    child,
                    format!("child-{child}"),
                    ProcessEventKind::Exec { image: smaller },
                )
                .unwrap();
            expected -= reduce;
            reduction -= reduce;
        }
        assert_eq!(reduction, 0);
        journal
            .record(
                pid,
                next_stable,
                ProcessEventKind::Fork {
                    parent_process_id: 1,
                    image: Some(full.clone()),
                },
            )
            .unwrap();
        expected += next_charge;
        assert_eq!(limit - expected, sample_charge - 1);
        assert_eq!(journal.ledger.retained_payload_bytes(), expected);
        let before = journal.ledger.snapshot();
        let prefix = fs::read(&journal.temporary_path).unwrap();
        let count_bytes = (journal.count, journal.bytes);
        let sample = ProcessEventKind::CloneUnclassified {
            parent_process_id: 1,
            reason: "x".repeat(8192),
        };
        let stable = "unclassified".to_owned();
        crate::allocation_observer::arm_at_least(sample_charge);
        let rejected = journal.record_transition(2, stable, sample);
        let observed = crate::allocation_observer::finish_observations();
        assert!(rejected.is_err(), "retained diagnostic debit omitted");
        assert!(
            rejected
                .unwrap_err()
                .contains("retained observation storage")
        );
        assert_eq!(
            observed.at_least_threshold, 0,
            "refusal materialized diagnostic: {observed:?}"
        );
        assert_eq!(journal.ledger.snapshot(), before);
        assert_eq!(journal.ledger.retained_payload_bytes(), expected);
        assert_eq!((journal.count, journal.bytes), count_bytes);
        assert_eq!(fs::read(&journal.temporary_path).unwrap(), prefix);
        // Credit one actual image byte, then the same conservative diagnostic
        // reservation fits exactly. No unrelated budget knobs or test flags.
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::Exec {
                    image: image_for(full_length - 1),
                },
            )
            .unwrap();
        expected -= 1;
        journal
            .record(
                2,
                "unclassified".to_owned(),
                ProcessEventKind::CloneUnclassified {
                    parent_process_id: 1,
                    reason: "x".repeat(8192),
                },
            )
            .unwrap();
        expected += sample_charge;
        assert_eq!(expected, limit);
        assert_eq!(journal.ledger.retained_payload_bytes(), expected);
        assert_eq!(journal.ledger.snapshot().violations.len(), 1);
        // Exercise public record's cutoff as well at zero aggregate allowance.
        let before = journal.ledger.snapshot();
        let prefix = fs::read(&journal.temporary_path).unwrap();
        assert!(
            journal
                .record(
                    2,
                    "unclassified".to_owned(),
                    ProcessEventKind::CloneUnclassified {
                        parent_process_id: 1,
                        reason: "second".to_owned(),
                    }
                )
                .unwrap_err()
                .contains("retained observation storage")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        assert_eq!(journal.ledger.retained_payload_bytes(), limit);
        assert_eq!(fs::read(&journal.temporary_path).unwrap(), prefix);
        let staging = journal.temporary_path.clone();
        drop(journal);
        assert!(!staging.exists());
    }

    #[test]
    fn diagnostic_charge_commits_with_image_replacement_and_not_with_failed_append() {
        let policy = test_policy(ClosureMode::DeclaredTree, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let receipt = unique_path("diagnostic-debit-atomic.json");
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        let root = test_root_image(&policy);
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            )
            .unwrap();
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::Exec {
                    image: root.clone(),
                },
            )
            .unwrap();
        let before_debit = journal.ledger.retained_payload_bytes();
        let unknown = policy.classify_observed_image(
            &policy
                .root_path
                .with_file_name("unadmitted-diagnostic-control"),
            "unknown".to_owned(),
            23,
            "c".repeat(64),
        );
        assert_eq!(unknown.class, ImageClass::Unknown);
        let sample_charge = crate::BUDGET_COMBINED_DIAGNOSTICS_JSON_BYTES
            / (2 * crate::BUDGET_DIAGNOSTICS_PER_CLASS);
        let expected = before_debit - serde_json::to_vec(&root).unwrap().len()
            + serde_json::to_vec(&unknown).unwrap().len()
            + sample_charge;
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::Exec { image: unknown },
            )
            .unwrap();
        assert_eq!(journal.ledger.retained_payload_bytes(), expected);
        let before = journal.ledger.snapshot();
        let prefix = fs::read(&journal.temporary_path).unwrap();
        let counters = (journal.count, journal.bytes);
        journal.refuse_writes_after(journal.count).unwrap();
        assert!(
            journal
                .record(
                    2,
                    "unclassified".to_owned(),
                    ProcessEventKind::CloneUnclassified {
                        parent_process_id: 1,
                        reason: "prepared diagnostic cannot commit".to_owned(),
                    }
                )
                .is_err()
        );
        assert_eq!(journal.ledger.retained_payload_bytes(), expected);
        assert_eq!(journal.ledger.snapshot(), before);
        assert_eq!(fs::read(&journal.temporary_path).unwrap(), prefix);
        assert_eq!((journal.count, journal.bytes), counters);
        let staging = journal.temporary_path.clone();
        drop(journal);
        fs::remove_file(staging).unwrap();
    }

    #[test]
    fn oversized_public_event_material_is_refused_before_proportional_allocation() {
        let policy = test_policy(ClosureMode::DeclaredTree, Vec::new());
        let capability = test_capability(policy.policy.mode);
        for case in [
            "file-id",
            "path",
            "role",
            "sha256",
            "fork-image",
            "initial-image",
            "reason",
            "stable-id",
        ] {
            let receipt = unique_path("oversize-entry.json");
            let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
            journal
                .record(
                    1,
                    "root".to_owned(),
                    ProcessEventKind::ProcessCreate {
                        parent_process_id: None,
                    },
                )
                .unwrap();
            journal
                .record(
                    1,
                    "root".to_owned(),
                    ProcessEventKind::Exec {
                        image: test_root_image(&policy),
                    },
                )
                .unwrap();
            let before = journal.ledger.snapshot();
            let prefix = fs::read(&journal.temporary_path).unwrap();
            let mut image = test_root_image(&policy);
            let huge = "x".repeat(MAX_EVENT_RECORD_BYTES * 2);
            let stable = if case == "stable-id" {
                huge.clone()
            } else {
                "root".to_owned()
            };
            match case {
                "file-id" | "fork-image" | "initial-image" => image.file_id = huge.clone(),
                "path" => image.path = policy.root_path.with_file_name(&huge),
                "role" => image.roles = vec![huge.clone()],
                "sha256" => image.sha256 = huge.clone(),
                _ => {}
            }
            let event = match case {
                "fork-image" => ProcessEventKind::Fork {
                    parent_process_id: 1,
                    image: Some(image),
                },
                "initial-image" => ProcessEventKind::InitialImage { image },
                "reason" => ProcessEventKind::CloneUnclassified {
                    parent_process_id: 1,
                    reason: huge.clone(),
                },
                _ => ProcessEventKind::Exec { image },
            };
            // All caller-owned material and expected snapshots precede arming.
            crate::allocation_observer::arm_at_least(8192);
            let rejected = journal.record(1, stable, event);
            let observed = crate::allocation_observer::finish_observations();
            assert!(rejected.unwrap_err().contains("budget"), "{case}");
            assert_eq!(observed.at_least_threshold, 0, "{case}: {observed:?}");
            assert!(observed.largest < 8192, "{case}: {observed:?}");
            assert_eq!(journal.ledger.snapshot(), before);
            assert_eq!(fs::read(&journal.temporary_path).unwrap(), prefix);
            let staging = journal.temporary_path.clone();
            drop(journal);
            assert!(!staging.exists());
        }
    }

    #[test]
    fn admitted_long_image_diagnostics_do_not_construct_classification_copies() {
        let policy = test_policy(ClosureMode::DeclaredTree, Vec::new());
        let capability = test_capability(policy.policy.mode);
        for inconsistent_class in [false, true] {
            let receipt = unique_path("long-image-diagnostic.json");
            let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
            journal
                .record(
                    1,
                    "root".to_owned(),
                    ProcessEventKind::ProcessCreate {
                        parent_process_id: None,
                    },
                )
                .unwrap();
            journal
                .record(
                    1,
                    "root".to_owned(),
                    ProcessEventKind::Exec {
                        image: test_root_image(&policy),
                    },
                )
                .unwrap();
            journal.buffer.reset(MAX_EVENT_RECORD_BYTES);
            journal
                .buffer
                .write_all(&vec![0; MAX_EVENT_RECORD_BYTES])
                .unwrap();
            journal.buffer.reset(MAX_EVENT_RECORD_BYTES);
            let path = policy
                .root_path
                .with_file_name("x".repeat(crate::BUDGET_PATH_UTF8_BYTES - 1024));
            let mut image =
                policy.classify_observed_image(&path, "unknown".to_owned(), 1, "b".repeat(64));
            assert_eq!(image.class, ImageClass::Unknown);
            if inconsistent_class {
                image.class = ImageClass::Fixed;
            }
            let before = journal.ledger.snapshot();
            let prefix = fs::read(&journal.temporary_path).unwrap();
            let stable = "root".to_owned();
            crate::allocation_observer::arm_at_least(8192);
            let result = journal.record(1, stable, ProcessEventKind::Exec { image });
            let observed = crate::allocation_observer::finish_observations();
            if inconsistent_class {
                let error = result.unwrap_err();
                assert!(error.contains("classification disagrees"));
                assert!(serde_json::to_vec(&error).unwrap().len() <= crate::MAX_DIAGNOSTIC_BYTES);
                assert_eq!(observed.at_least_threshold, 0, "{observed:?}");
                assert_eq!(journal.ledger.snapshot(), before);
                assert_eq!(fs::read(&journal.temporary_path).unwrap(), prefix);
            } else {
                assert!(result.unwrap().has_policy_violation());
                assert_eq!(
                    observed.at_least_threshold, 1,
                    "only the live image owns a long copy: {observed:?}"
                );
                let snapshot = journal.ledger.snapshot();
                assert_eq!(snapshot.violation_count, 1);
                assert!(snapshot.violations[0].starts_with("unadmitted executable image"));
                assert!(snapshot.violations[0].ends_with("..."));
                assert!(
                    serde_json::to_vec(&snapshot.violations[0]).unwrap().len()
                        <= crate::MAX_DIAGNOSTIC_BYTES
                );
            }
            let staging = journal.temporary_path.clone();
            drop(journal);
            assert!(!staging.exists());
        }
    }

    #[test]
    fn diagnostic_prefix_allocation_does_not_copy_or_shrink_the_full_event_reason() {
        let policy = test_policy(ClosureMode::DeclaredTree, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let receipt = unique_path("bounded-reason.json");
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            )
            .unwrap();
        // Reserve the ordinary event wire buffer before observing diagnostic
        // allocation. Its legitimate large serialization is not a message copy.
        journal.buffer.reset(MAX_EVENT_RECORD_BYTES);
        journal
            .buffer
            .write_all(&vec![0; MAX_EVENT_RECORD_BYTES])
            .unwrap();
        journal.buffer.reset(MAX_EVENT_RECORD_BYTES);
        let reason = "\u{0000}".repeat(32768);
        let event = ProcessEventKind::CloneUnclassified {
            parent_process_id: 1,
            reason: reason.clone(),
        };
        let stable = "unclassified-child".to_owned();
        crate::allocation_observer::arm_at_least(8192);
        let outcome = journal.record(2, stable, event);
        let observed = crate::allocation_observer::finish_observations();
        assert!(outcome.unwrap().has_policy_violation());
        assert_eq!(observed.at_least_threshold, 0, "{observed:?}");
        let snapshot = journal.ledger.snapshot();
        assert_eq!(snapshot.violation_count, 1);
        assert!(snapshot.violations[0].ends_with("..."));
        assert!(
            serde_json::to_vec(&snapshot.violations[0]).unwrap().len()
                <= crate::MAX_DIAGNOSTIC_BYTES
        );
        let accepted = fs::read(&journal.temporary_path).unwrap();
        let row: ProcessEvent = serde_json::from_slice(
            accepted
                .split(|b| *b == b'\n')
                .filter(|row| !row.is_empty())
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            row.event,
            ProcessEventKind::CloneUnclassified {
                parent_process_id: 1,
                reason
            }
        );
        let oversized_cause = "é\n\"".repeat(MAX_EVENT_RECORD_BYTES);
        crate::allocation_observer::arm_at_least(8192);
        journal.cutoff(crate::CaptureStage::NativeObservation, &oversized_cause);
        let observed = crate::allocation_observer::finish_observations();
        assert_eq!(observed.at_least_threshold, 0, "{observed:?}");
        let crate::JournalCoverage::Prefix { cause, .. } = journal.coverage() else {
            panic!("missing cutoff");
        };
        assert!(serde_json::to_vec(cause).unwrap().len() <= crate::MAX_DIAGNOSTIC_BYTES);
        let staging = journal.temporary_path.clone();
        drop(journal);
        assert!(!staging.exists());
    }

    #[test]
    fn actual_capture_refusal_freezes_one_exact_prefix() {
        let path = unique_path("capture-cutoff.json");
        let policy = test_policy(ClosureMode::DeclaredTree, Vec::new());
        let cap = test_capability(policy.policy.mode);
        let mut journal = EventJournal::create(&path, &policy, &cap).unwrap();
        journal
            .record(
                1,
                "root".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            )
            .unwrap();
        let accepted = fs::read(&journal.temporary_path).unwrap();
        assert!(
            journal
                .record(
                    2,
                    "unowned".to_owned(),
                    ProcessEventKind::Exec {
                        image: test_root_image(&policy)
                    }
                )
                .is_err()
        );
        assert!(
            journal
                .record(
                    1,
                    "root".to_owned(),
                    ProcessEventKind::ProcessExit { exit_code: 137 }
                )
                .unwrap_err()
                .contains("already cut off")
        );
        assert_eq!(fs::read(&journal.temporary_path).unwrap(), accepted);
        let crate::JournalCoverage::Prefix {
            next_sequence,
            accepted_records,
            accepted_bytes,
            accepted_sha256,
            ..
        } = journal.coverage()
        else {
            panic!("missing cutoff")
        };
        assert_eq!(
            (*next_sequence, *accepted_records, *accepted_bytes),
            (2, 1, accepted.len() as u64)
        );
        assert_eq!(accepted_sha256, &sha256_bytes(&accepted));
        let coverage = journal.coverage().clone();
        let published = journal.publish().unwrap();
        let mut receipt = crate::Receipt::running(&policy, &cap);
        receipt.apply_verified_event_log(&published.verified);
        receipt.journal_coverage = coverage;
        receipt.native_custody = crate::NativeCustody::Linux {
            remaining_tasks: 0,
            remaining_processes: 0,
            wait_exhausted: true,
            root_exit_code: Some(137),
        };
        receipt.record_error(
            "capture omitted the actual unknown exec; native cleanup closed separately",
        );
        receipt.finish(false);
        receipt.attach_evidence(published).unwrap();
        assert!(receipt.terminal_is_consistent());
        assert_eq!(receipt.accounting.active_processes, 1);
        assert!(receipt.native_custody.is_closed());
        receipt.complete = true;
        receipt.state = crate::SupervisorState::Complete;
        assert!(!receipt.terminal_is_consistent());
    }

    /// A Write boundary that really stores the requested prefix in a File,
    /// then reports an error. The oracle reads those bytes independently;
    /// this models a partial syscall sequence, not a fake successful journal.
    struct FailAfter<'a> {
        file: &'a mut File,
        remaining: usize,
    }
    impl Write for FailAfter<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("injected append exhaustion"));
            }
            let count = self.file.write(&bytes[..bytes.len().min(self.remaining)])?;
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    #[test]
    fn failed_or_partial_append_cannot_commit_admission_or_reuse_its_sequence() {
        for windows in [false, true] {
            for written in [0, 17] {
                let receipt = unique_path("partial-append.json");
                let policy = test_policy(ClosureMode::Leaf, Vec::new());
                let mut capability = test_capability(policy.policy.mode);
                if windows {
                    capability.platform = "windows".to_owned();
                    capability.backend = "debug-process+nested-job".to_owned();
                }
                let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
                {
                    journal
                        .record_transition(
                            41,
                            "owned-root".to_owned(),
                            ProcessEventKind::ProcessCreate {
                                parent_process_id: None,
                            },
                        )
                        .unwrap();
                }
                let staging = journal.temporary_path.clone();
                let accepted = fs::read(&staging).unwrap();
                let before = journal.ledger.snapshot();
                let event = if windows {
                    ProcessEventKind::InitialImage {
                        image: test_root_image(&policy),
                    }
                } else {
                    ProcessEventKind::Exec {
                        image: test_root_image(&policy),
                    }
                };
                let error = journal
                    .record_with_append(41, "owned-root".to_owned(), event, |file, bytes| {
                        FailAfter {
                            file,
                            remaining: written,
                        }
                        .write_all(bytes)
                    })
                    .unwrap_err();
                assert!(error.contains("injected append exhaustion"), "{error}");
                assert_eq!(journal.ledger.snapshot(), before);
                assert_eq!(
                    journal.verified().unwrap().admission,
                    Admission::Eligible {}
                );
                assert_eq!(journal.count, 1);
                assert_eq!(journal.bytes, accepted.len() as u64);
                assert_eq!(
                    crate::hex_lower(&journal.digest.clone().finalize()),
                    sha256_bytes(&accepted)
                );
                let partial = fs::read(&staging).unwrap();
                assert_eq!(&partial[..accepted.len()], accepted);
                assert_eq!(partial.len(), accepted.len() + written);
                // A real terminal observation still belongs to the platform
                // cleanup owner. It cannot be disguised as the failed sequence.
                let refused = journal
                    .record_with_append(
                        41,
                        "owned-root".to_owned(),
                        ProcessEventKind::ProcessExit { exit_code: 137 },
                        |_, _| panic!("poisoned journal must never attempt another write"),
                    )
                    .unwrap_err();
                assert!(refused.contains(&error));
                assert_eq!(fs::read(&staging).unwrap(), partial);
                assert_eq!(journal.ledger.snapshot(), before);
                assert!(journal.publish().unwrap_err().contains("poisoned"));
                assert_eq!(fs::read(&staging).unwrap(), partial); // Drop did not retry/delete.
                fs::remove_file(staging).unwrap();
            }
        }
    }

    #[test]
    fn actual_read_only_file_failure_preserves_the_ledger_without_descriptor_aliasing() {
        let receipt = unique_path("readonly-append.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        let staging = journal.temporary_path.clone();
        let before = journal.ledger.snapshot();
        // Owned File values close their own descriptors exactly once. No raw
        // fd replacement, double close or unrelated-descriptor corruption.
        journal.file = Some(File::open(&staging).unwrap());
        assert!(
            journal
                .record_transition(
                    41,
                    "root".to_owned(),
                    ProcessEventKind::ProcessCreate {
                        parent_process_id: None,
                    }
                )
                .unwrap_err()
                .contains("cannot append")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        assert_eq!(journal.count, 0);
        drop(journal);
        assert_eq!(fs::read(&staging).unwrap(), b"");
        fs::remove_file(staging).unwrap();
    }

    fn completed_journal<'a>(receipt: &Path, policy: &'a ValidatedPolicy) -> EventJournal<'a> {
        let capability = test_capability(policy.policy.mode);
        let mut journal = EventJournal::create(receipt, &policy, &capability).unwrap();
        journal
            .record_transition(
                41,
                "root".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            )
            .unwrap();
        journal
            .record_transition(
                41,
                "root".to_owned(),
                ProcessEventKind::ProcessExit { exit_code: 42 },
            )
            .unwrap();
        journal
    }

    #[test]
    fn sync_failure_preserves_the_complete_written_journal_without_publishing() {
        let receipt = unique_path("sync-failure.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let journal = completed_journal(&receipt, &policy);
        let staging = journal.temporary_path.clone();
        let bytes = fs::read(&staging).unwrap();
        let destination = event_artifact_path(&receipt, &sha256_bytes(&bytes)).unwrap();
        let error = journal
            .publish_with(
                |_| Err(io::Error::other("injected file sync failure")),
                |_, _| panic!("a failed sync cannot reach rename"),
            )
            .unwrap_err();
        assert!(error.contains("injected file sync failure"));
        assert_eq!(fs::read(&staging).unwrap(), bytes);
        assert!(!destination.exists());
        fs::remove_file(staging).unwrap();
    }

    #[test]
    fn actual_rename_failure_preserves_staging_and_the_existing_destination() {
        let receipt = unique_path("rename-failure.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let journal = completed_journal(&receipt, &policy);
        let staging = journal.temporary_path.clone();
        let bytes = fs::read(&staging).unwrap();
        let destination = event_artifact_path(&receipt, &sha256_bytes(&bytes)).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("sentinel"), b"untouched").unwrap();
        let error = journal.publish().unwrap_err();
        assert!(error.contains("publication not acknowledged"), "{error}");
        assert_eq!(fs::read(&staging).unwrap(), bytes);
        assert_eq!(
            fs::read(destination.join("sentinel")).unwrap(),
            b"untouched"
        );
        fs::remove_file(staging).unwrap();
        fs::remove_dir_all(destination).unwrap();
    }

    #[test]
    fn directory_sync_failure_keeps_renamed_evidence_without_acknowledging_it() {
        let receipt = unique_path("directory-sync-failure.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let journal = completed_journal(&receipt, &policy);
        let staging = journal.temporary_path.clone();
        let bytes = fs::read(&staging).unwrap();
        let destination = event_artifact_path(&receipt, &sha256_bytes(&bytes)).unwrap();
        let error = journal
            .publish_with(
                |file| file.sync_all(),
                |from, to| {
                    fs::rename(from, to).map_err(|error| error.to_string())?;
                    Err("injected directory sync failure after actual rename".to_owned())
                },
            )
            .unwrap_err();
        assert!(error.contains("injected directory sync failure"));
        assert!(error.contains(&destination.display().to_string()));
        assert!(!staging.exists());
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        fs::remove_file(destination).unwrap();
    }

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

    pub(crate) fn test_policy(
        mode: ClosureMode,
        derived_roots: Vec<DerivedRoot>,
    ) -> ValidatedPolicy {
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

    pub(crate) fn test_capability(mode: ClosureMode) -> Capability {
        Capability {
            schema: CAPABILITY_SCHEMA.to_owned(),
            platform: "linux".to_owned(),
            mode,
            backend: "ptrace-exitkill".to_owned(),
            admission: Admission::Eligible {},
            pre_entry_exec_authority: true,
            pre_entry_process_create_authority: true,
            recursive_descendant_authority: true,
            required_environment: crate::platform::required_environment(),
        }
    }

    pub(crate) fn test_root_image(policy: &ValidatedPolicy) -> FileIdentity {
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
    fn regular_input_owner_binds_empty_exact_growth_and_same_size_replacement() {
        use std::io::Read;
        let path = unique_path("regular-input.json");
        fs::write(&path, b"").unwrap();
        let empty = OpenedRegularFile::open(&path).unwrap();
        assert_eq!(empty.read_all(0).unwrap(), b"");
        drop(empty);
        fs::write(&path, b"abcd").unwrap();
        let opened = OpenedRegularFile::open(&path).unwrap();
        assert!(opened.read_all(3).is_err());
        assert_eq!(opened.read_all(4).unwrap(), b"abcd");
        opened.rewind().unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[b'x'; 1024])
            .unwrap();
        let mut consumed = Vec::new();
        opened.bounded_reader().read_to_end(&mut consumed).unwrap();
        assert_eq!(
            consumed, b"abcdx",
            "growth reads at most the old extent plus one probe"
        );
        opened.rewind().unwrap();
        assert!(opened.read_all(4096).is_err());
        assert!(opened.verify().is_err());
        drop(opened);
        fs::write(&path, b"abcd").unwrap();
        let opened = OpenedRegularFile::open(&path).unwrap();
        fs::write(&path, b"wxyz").unwrap();
        assert!(
            opened.verify().is_err(),
            "same-sized writes change the mutation token"
        );
        drop(opened);
        fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn regular_input_owner_keeps_nonblocking_flag_and_rejects_replaced_name() {
        use std::os::fd::AsRawFd;
        let path = unique_path("regular-input-generation.json");
        let replacement = unique_path("replacement-generation.json");
        fs::write(&path, b"same").unwrap();
        let opened = OpenedRegularFile::open(&path).unwrap();
        let flags = unsafe { libc::fcntl(opened.file().as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_ne!(
            flags & libc::O_NONBLOCK,
            0,
            "pre-open type checks cannot close the FIFO replacement race"
        );
        fs::write(&replacement, b"same").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(
            opened.verify().is_err(),
            "equal bytes cannot replace OS generation identity"
        );
        drop(opened);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn launch_failure_preserves_eligible_incomplete_evidence() {
        let path = unique_path("launch-failure.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let journal = EventJournal::create(&path, &policy, &capability).unwrap();
        let published = assert_published_replay_matches(journal, &path, &policy, &capability);
        let mut receipt = crate::Receipt::running(&policy, &capability);
        receipt.apply_verified_event_log(&published.verified);
        receipt.record_error("actual launch admission failed before a root image");
        receipt.finish(false);
        receipt.attach_evidence(published).unwrap();
        assert_eq!(receipt.capability.admission, Admission::Eligible {});
        assert!(receipt.terminal_is_consistent());
        assert!(!receipt.complete);
        receipt.complete = true;
        assert!(!receipt.terminal_is_consistent());
    }

    #[test]
    fn admission_is_derived_from_the_accepted_root_and_initial_image() {
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        for windows in [false, true] {
            let path = unique_path("admission.json");
            let mut capability = test_capability(policy.policy.mode);
            if windows {
                capability.platform = "windows".to_owned();
                capability.backend = "debug-process+nested-job".to_owned();
            }
            capability.admission = Admission::Admitted {
                root_stable_process_id: "forged".to_owned(),
                root_create_sequence: 4,
                initial_image_sequence: 5,
            };
            let mut journal = EventJournal::create(&path, &policy, &capability).unwrap();
            let root_image = test_root_image(&policy);
            let image_event = |image| {
                if windows {
                    ProcessEventKind::InitialImage { image }
                } else {
                    ProcessEventKind::Exec { image }
                }
            };
            let before = journal.ledger.snapshot();
            assert!(
                journal
                    .record_transition(41, "root".to_owned(), image_event(root_image.clone()))
                    .is_err()
            );
            assert_eq!(journal.ledger.snapshot(), before);
            journal
                .record_transition(
                    41,
                    "root".to_owned(),
                    ProcessEventKind::ProcessCreate {
                        parent_process_id: None,
                    },
                )
                .unwrap();
            assert_eq!(journal.ledger.snapshot().admission, Admission::Eligible {});
            let wrong = policy.classify_observed_image(
                &policy.root_path.with_file_name("wrong"),
                "wrong".to_owned(),
                1,
                root_image.sha256.clone(),
            );
            let before = journal.ledger.snapshot();
            assert!(
                journal
                    .record_transition(41, "root".to_owned(), image_event(wrong))
                    .is_err()
            );
            assert_eq!(journal.ledger.snapshot(), before);
            journal
                .record_transition(41, "root".to_owned(), image_event(root_image.clone()))
                .unwrap();
            let expected = Admission::Admitted {
                root_stable_process_id: "root".to_owned(),
                root_create_sequence: 1,
                initial_image_sequence: 2,
            };
            assert_eq!(journal.ledger.snapshot().admission, expected);
            if windows {
                let before = journal.ledger.snapshot();
                assert!(
                    journal
                        .record_transition(41, "root".to_owned(), image_event(root_image))
                        .unwrap_err()
                        .contains("repeats admission")
                );
                assert_eq!(journal.ledger.snapshot(), before);
            }
            journal
                .record_transition(
                    41,
                    "root".to_owned(),
                    ProcessEventKind::ProcessExit { exit_code: 0 },
                )
                .unwrap();
            let published = assert_published_replay_matches(journal, &path, &policy, &capability);
            assert_eq!(published.verified.admission, expected);
            assert_ne!(published.verified.admission, capability.admission);
        }
    }

    #[test]
    fn event_journal_is_adjacent_durable_and_stream_verified() {
        let receipt = unique_path("receipt.json");
        let policy = test_policy(ClosureMode::Leaf, Vec::new());
        let capability = test_capability(policy.policy.mode);
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        journal
            .record_transition(
                1,
                "test:1".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            )
            .unwrap();
        journal
            .record_transition(
                1,
                "test:1".to_owned(),
                ProcessEventKind::ProcessExit { exit_code: 0 },
            )
            .unwrap();
        let published = journal.publish().unwrap();
        assert_eq!(published.verified.admission, Admission::Eligible {});
        let mut obsolete = published.event_log.clone();
        obsolete.schema = "molt.proof-process-event-log.v2".to_owned();
        assert!(verify_event_artifact(&receipt, &obsolete, &policy, &capability).is_err());
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
        let image = test_root_image(&policy);
        let mut journal = EventJournal::create(&receipt, &policy, &capability).unwrap();
        // The retired combined dialect is no longer representable by the API
        // and strict decoding rejects it before any journal transition.
        let combined =
            serde_json::json!({"kind":"process-create", "parent_process_id":null, "image":image});
        assert!(
            serde_json::from_value::<ProcessEventKind>(combined)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
        assert_eq!(journal.count, 0);
        journal
            .record_transition(
                1,
                "test:1".to_owned(),
                ProcessEventKind::ProcessCreate {
                    parent_process_id: None,
                },
            )
            .unwrap();
        let before = journal.ledger.snapshot();
        let wrong = policy.classify_observed_image(
            &policy.root_path.with_file_name("wrong-root"),
            "wrong".to_owned(),
            2,
            "b".repeat(64),
        );
        assert!(
            journal
                .record_transition(
                    1,
                    "test:1".to_owned(),
                    ProcessEventKind::InitialImage { image: wrong }
                )
                .is_err()
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record_transition(
                1,
                "test:1".to_owned(),
                ProcessEventKind::InitialImage {
                    image: image.clone(),
                },
            )
            .unwrap();
        let child = ProcessEventKind::ProcessCreate {
            parent_process_id: Some(1),
        };
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record_transition(1, "test:2".to_owned(), child.clone())
                .unwrap_err()
                .contains("reuses live process id")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record_transition(2, "test:2".to_owned(), child)
            .unwrap();
        let before = journal.ledger.snapshot();
        let mut forged = image.clone();
        forged.roles.push("forged-role".to_owned());
        assert!(
            journal
                .record_transition(
                    2,
                    "test:2".to_owned(),
                    ProcessEventKind::InitialImage { image: forged }
                )
                .unwrap_err()
                .contains("classification disagrees")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record_transition(
                2,
                "test:2".to_owned(),
                ProcessEventKind::InitialImage { image },
            )
            .unwrap();
        for id in [2, 1] {
            journal
                .record_transition(
                    id,
                    format!("test:{id}"),
                    ProcessEventKind::ProcessExit { exit_code: 0 },
                )
                .unwrap();
        }
        let published = assert_published_replay_matches(journal, &receipt, &policy, &capability);
        assert_eq!(published.event_log.count, 6);
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
            .record_transition(1, "test:1".to_owned(), events[0].event.clone())
            .unwrap();
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record_transition(2, "pre-exec:2".to_owned(), events[2].event.clone())
                .unwrap_err()
                .contains("inherited live parent image")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record_transition(
                2,
                "pre-exec:2".to_owned(),
                ProcessEventKind::Fork {
                    parent_process_id: 1,
                    image: None,
                },
            )
            .unwrap();
        journal
            .record_transition(
                2,
                "pre-exec:2".to_owned(),
                ProcessEventKind::ProcessExit { exit_code: 0 },
            )
            .unwrap();
        journal
            .record_transition(1, "test:1".to_owned(), events[1].event.clone())
            .unwrap();
        let before = journal.ledger.snapshot();
        for image in forged_images {
            assert!(
                journal
                    .record_transition(
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
            .record_transition(2, "test:2".to_owned(), events[2].event.clone())
            .unwrap();
        assert!(
            journal
                .record_transition(1, "test:1".to_owned(), events[3].event.clone())
                .unwrap()
                .has_policy_violation()
        );
        journal
            .record_transition(2, "test:2".to_owned(), events[4].event.clone())
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
            .record_transition(1, "test:1".to_owned(), events[0].event.clone())
            .unwrap();
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record_transition(1, "test:1".to_owned(), events[2].event.clone())
                .unwrap_err()
                .contains("initial root image")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record_transition(1, "test:1".to_owned(), events[1].event.clone())
            .unwrap();
        journal
            .record_transition(1, "test:1".to_owned(), events[2].event.clone())
            .unwrap();
        let before = journal.ledger.snapshot();
        assert!(
            journal
                .record_transition(1, "test:1".to_owned(), events[3].event.clone())
                .unwrap_err()
                .contains("identity changed")
        );
        assert_eq!(journal.ledger.snapshot(), before);
        journal
            .record_transition(
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
                .record_transition(
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

/// Read admitted policy/receipt bytes from one retained generation; event logs
/// use the same owner with bounded streaming instead of retaining the full log.
pub fn read_bounded_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    OpenedRegularFile::open(path)
        .and_then(|opened| opened.read_all(limit))
        .map_err(|error| error.to_string())
}
