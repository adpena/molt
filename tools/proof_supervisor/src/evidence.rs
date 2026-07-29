use crate::{Accounting, EventKind, FileIdentity, ImageClass, ProcessEvent, sha256_bytes};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const EVENT_LOG_SCHEMA: &str = "molt.proof-process-event-log.v1";
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
    pub derived_images: IdentitySummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedEventLog {
    pub derived_images: IdentitySummary,
    pub accounting: Accounting,
}

pub struct EventJournal {
    temporary_path: PathBuf,
    receipt_path: PathBuf,
    file: Option<BufWriter<File>>,
    buffer: Vec<u8>,
    digest: Sha256,
    count: u64,
    bytes: u64,
    last_sequence: Option<u64>,
    derived: BTreeMap<String, FileIdentity>,
    published: bool,
}

impl EventJournal {
    pub fn create(receipt_path: &Path) -> Result<Self, String> {
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
            last_sequence: None,
            derived: BTreeMap::new(),
            published: false,
        })
    }

    pub fn record(&mut self, event: &ProcessEvent) -> Result<(), String> {
        if let Some(previous) = self.last_sequence
            && event.sequence <= previous
        {
            return Err(format!(
                "process event sequence is not strictly increasing: {previous} then {}",
                event.sequence
            ));
        }
        self.buffer.clear();
        serde_json::to_writer(&mut self.buffer, event)
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
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| "process event journal count overflow".to_owned())?;
        self.last_sequence = Some(event.sequence);
        if let Some(image) = &event.image
            && image.class == ImageClass::Derived
        {
            let key = crate::normalized_path_key(&image.path);
            if let Some(prior) = self.derived.get(&key) {
                if prior != image {
                    return Err(format!(
                        "derived image identity changed during run: {}",
                        image.path.display()
                    ));
                }
            } else {
                self.derived.insert(key, image.clone());
            }
        }
        Ok(())
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
        let derived_images =
            summarize_identities(std::mem::take(&mut self.derived).into_values().collect())?;
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
            derived_images,
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
    let mut previous_sequence = None;
    let mut derived = BTreeMap::new();
    let mut active_processes = BTreeMap::<String, u32>::new();
    let mut accounting = Accounting::default();
    let mut root_stable_id = None;
    loop {
        line.clear();
        let bytes = read_bounded_record(&mut reader, &mut line)
            .map_err(|error| format!("cannot read event log: {error}"))?;
        if bytes == 0 {
            break;
        }
        digest.update(&line);
        if line.last() != Some(&b'\n') {
            return Err("event log has a non-terminated final record".to_owned());
        }
        let event: ProcessEvent = serde_json::from_slice(&line[..line.len() - 1])
            .map_err(|error| format!("invalid process event record: {error}"))?;
        if let Some(previous) = previous_sequence
            && event.sequence <= previous
        {
            return Err("event log sequence is not strictly increasing".to_owned());
        }
        previous_sequence = Some(event.sequence);
        count = count
            .checked_add(1)
            .ok_or_else(|| "event log count overflow".to_owned())?;
        if let Some(image) = &event.image
            && image.class == ImageClass::Derived
        {
            let key = crate::normalized_path_key(&image.path);
            if let Some(prior) = derived.get(&key) {
                if prior != image {
                    return Err(format!(
                        "derived image identity changed within event log: {}",
                        image.path.display()
                    ));
                }
            } else {
                derived.insert(key, image.clone());
            }
        }
        match event.kind {
            EventKind::ProcessCreate | EventKind::Fork => {
                if active_processes
                    .insert(event.stable_process_id.clone(), event.process_id)
                    .is_some()
                {
                    return Err("event log creates one stable process identity twice".to_owned());
                }
                if root_stable_id.is_none() {
                    root_stable_id = Some(event.stable_process_id.clone());
                }
                accounting.total_processes = accounting.total_processes.saturating_add(1);
                accounting.observed_process_creates =
                    accounting.observed_process_creates.saturating_add(1);
                if event.kind == EventKind::ProcessCreate && event.image.is_some() {
                    accounting.observed_execs = accounting.observed_execs.saturating_add(1);
                    if root_stable_id.as_deref() == Some(&event.stable_process_id) {
                        accounting.root_execs = accounting.root_execs.saturating_add(1);
                    }
                }
            }
            EventKind::Exec => {
                if !active_processes.contains_key(&event.stable_process_id) {
                    return Err("event log exec has no live process creation".to_owned());
                }
                if event.image.is_none() {
                    return Err("event log exec has no classified image".to_owned());
                }
                accounting.observed_execs = accounting.observed_execs.saturating_add(1);
                if root_stable_id.as_deref() == Some(&event.stable_process_id) {
                    accounting.root_execs = accounting.root_execs.saturating_add(1);
                }
            }
            EventKind::ProcessExit => {
                if active_processes.remove(&event.stable_process_id).is_none() {
                    return Err("event log exit has no live process creation".to_owned());
                }
                accounting.observed_process_exits =
                    accounting.observed_process_exits.saturating_add(1);
            }
            EventKind::ThreadCreate | EventKind::CloneUnclassified => {}
        }
    }
    if count != expected.count {
        return Err(format!(
            "event log record count mismatch: receipt={} actual={count}",
            expected.count
        ));
    }
    let actual_sha256 = crate::hex_lower(&digest.finalize());
    if !crate::constant_time_eq(expected.sha256.as_bytes(), actual_sha256.as_bytes()) {
        return Err("event log digest mismatch".to_owned());
    }
    accounting.active_processes = active_processes.len() as u64;
    Ok(VerifiedEventLog {
        derived_images: summarize_identities(derived.into_values().collect())?,
        accounting,
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
    identities.sort_by(|left, right| {
        crate::normalized_path_key(&left.path).cmp(&crate::normalized_path_key(&right.path))
    });
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
fn windows_api_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path)
            .map_err(|error| format!("cannot canonicalize {}: {error}", path.display()));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = fs::canonicalize(parent).map_err(|error| {
        format!(
            "cannot canonicalize evidence directory {}: {error}",
            parent.display()
        )
    })?;
    let name = path
        .file_name()
        .ok_or_else(|| "evidence path must name a file".to_owned())?;
    Ok(canonical_parent.join(name))
}

#[cfg(windows)]
fn durable_replace(temporary: &Path, final_path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let old_path = windows_api_path(temporary)?;
    let new_path = windows_api_path(final_path)?;
    let old: Vec<u16> = old_path.as_os_str().encode_wide().chain([0]).collect();
    let new: Vec<u16> = new_path.as_os_str().encode_wide().chain([0]).collect();
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
    use crate::{EventKind, ProcessEvent};
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

    #[test]
    fn event_journal_is_adjacent_durable_and_stream_verified() {
        let receipt = unique_path("receipt.json");
        let mut journal = EventJournal::create(&receipt).unwrap();
        journal
            .record(&ProcessEvent {
                sequence: 6,
                kind: EventKind::ProcessCreate,
                process_id: 1,
                parent_process_id: None,
                stable_process_id: "test:1".to_owned(),
                image: None,
                exit_code: None,
            })
            .unwrap();
        journal
            .record(&ProcessEvent {
                sequence: 7,
                kind: EventKind::ProcessExit,
                process_id: 1,
                parent_process_id: None,
                stable_process_id: "test:1".to_owned(),
                image: None,
                exit_code: Some(0),
            })
            .unwrap();
        let published = journal.publish().unwrap();
        assert_eq!(published.event_log.count, 2);
        assert_eq!(published.derived_images, IdentitySummary::empty());
        assert_eq!(
            verify_event_artifact(&receipt, &published.event_log)
                .unwrap()
                .derived_images,
            published.derived_images
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
    fn derived_identity_drift_is_rejected_during_record_and_replay() {
        let receipt = unique_path("derived-drift.json");
        let derived_path = unique_path("derived-tool");
        let image = |sha256: &str| FileIdentity {
            path: derived_path.clone(),
            file_id: "device:inode".to_owned(),
            size_bytes: 4,
            sha256: sha256.to_owned(),
            class: ImageClass::Derived,
            roles: vec!["generated-tool".to_owned()],
        };
        let events = vec![
            ProcessEvent {
                sequence: 1,
                kind: EventKind::ProcessCreate,
                process_id: 1,
                parent_process_id: None,
                stable_process_id: "test:1".to_owned(),
                image: None,
                exit_code: None,
            },
            ProcessEvent {
                sequence: 2,
                kind: EventKind::Exec,
                process_id: 1,
                parent_process_id: None,
                stable_process_id: "test:1".to_owned(),
                image: Some(image(&"a".repeat(64))),
                exit_code: None,
            },
            ProcessEvent {
                sequence: 3,
                kind: EventKind::Exec,
                process_id: 1,
                parent_process_id: None,
                stable_process_id: "test:1".to_owned(),
                image: Some(image(&"b".repeat(64))),
                exit_code: None,
            },
        ];

        let mut journal = EventJournal::create(&receipt).unwrap();
        journal.record(&events[0]).unwrap();
        journal.record(&events[1]).unwrap();
        assert!(
            journal
                .record(&events[2])
                .unwrap_err()
                .contains("identity changed")
        );
        drop(journal);

        let mut bytes = Vec::new();
        for event in &events {
            serde_json::to_writer(&mut bytes, event).unwrap();
            bytes.push(b'\n');
        }
        let sha256 = sha256_bytes(&bytes);
        let path = event_artifact_path(&receipt, &sha256).unwrap();
        durable_atomic_write(&path, &bytes).unwrap();
        let summary = ArtifactSummary {
            schema: EVENT_LOG_SCHEMA.to_owned(),
            file: path.file_name().unwrap().to_string_lossy().into_owned(),
            count: events.len() as u64,
            bytes: bytes.len() as u64,
            sha256,
        };
        assert!(
            verify_event_artifact(&receipt, &summary)
                .unwrap_err()
                .contains("identity changed")
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn verifier_rejects_an_oversized_event_record_without_unbounded_read() {
        let receipt = unique_path("oversized-event.json");
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
            verify_event_artifact(&receipt, &summary)
                .unwrap_err()
                .contains("exceeds")
        );
        let _ = fs::remove_file(path);
    }
}
