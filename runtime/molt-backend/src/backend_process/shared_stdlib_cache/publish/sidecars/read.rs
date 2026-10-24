use std::{
    io::{self, Read},
    path::Path,
};

#[cfg(test)]
use super::super::lock::with_shared_stdlib_cache_publish_lock;
use super::super::paths::{
    stdlib_cache_key_sidecar_path, stdlib_cache_manifest_sidecar_path,
    stdlib_cache_origin_sidecar_path, stdlib_cache_partition_manifest_sidecar_path,
};

use crate::backend_process::input::ContentIdentity;
use crate::backend_process::native_batch::NativeWorkerOrigin;

const ORIGIN_SCHEMA: &str = "molt.stdlib-archive-origin.v1";
pub(super) const MAX_ORIGIN_BYTES: u64 = 64 * 1024 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StdlibArchiveOrigin {
    pub(super) schema: String,
    pub(super) output: ContentIdentity,
    pub(super) workers: Vec<NativeWorkerOrigin>,
}

impl StdlibArchiveOrigin {
    pub(super) fn new(output: ContentIdentity, workers: &[NativeWorkerOrigin]) -> io::Result<Self> {
        if workers.is_empty() || workers.len() > 16_384 {
            return Err(invalid_origin(
                "stdlib archive origin has no bounded constituents",
            ));
        }
        for worker in workers {
            worker.validate()?;
        }
        Ok(Self {
            schema: ORIGIN_SCHEMA.into(),
            output,
            workers: workers.to_vec(),
        })
    }
}

fn invalid_origin(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Read under the archive publication lock. The caller separately admits the
/// recorded archive digest against the bytes it copies; this only decodes the
/// generation's metadata and never manufactures absent historical provenance.
pub(in super::super::super) fn read_stdlib_cache_origins_unlocked(
    path: &Path,
    digest: &str,
) -> io::Result<Option<Vec<NativeWorkerOrigin>>> {
    let file =
        match molt_artifact_publish::open_regular_artifact(&stdlib_cache_origin_sidecar_path(path))
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
    if file.metadata()?.len() > MAX_ORIGIN_BYTES {
        return Err(invalid_origin("stdlib archive origin exceeds its bound"));
    }
    let mut encoded = Vec::new();
    file.take(MAX_ORIGIN_BYTES + 1).read_to_end(&mut encoded)?;
    if encoded.len() as u64 > MAX_ORIGIN_BYTES {
        return Err(invalid_origin("stdlib archive origin exceeds its bound"));
    }
    let record: StdlibArchiveOrigin = serde_json::from_slice(&encoded)
        .map_err(|error| invalid_origin(&format!("invalid stdlib archive origin: {error}")))?;
    if record.schema != ORIGIN_SCHEMA
        || record.output.sha256 != digest
        || record.output.size != std::fs::metadata(path)?.len()
        || record.workers.is_empty()
        || record.workers.len() > 16_384
    {
        return Err(invalid_origin(
            "stdlib archive origin names a different generation",
        ));
    }
    for worker in &record.workers {
        worker.validate()?;
    }
    Ok(Some(record.workers))
}

/// Admit the complete original constituent provenance of this archive snapshot.
/// No process/image admission is granted here; the receiver still replays each
/// worker's original closed supervisor evidence session.
#[cfg(test)]
pub(crate) fn read_shared_stdlib_cache_origins(
    path: &Path,
) -> io::Result<Option<Vec<NativeWorkerOrigin>>> {
    with_shared_stdlib_cache_publish_lock(path, || {
        // An ordinary historical archive may have no provenance. Refuse that
        // claim cheaply instead of reading an archive-sized payload first.
        if !stdlib_cache_origin_sidecar_path(path).try_exists()? {
            return Ok(None);
        }
        let file = molt_artifact_publish::open_regular_artifact(path)?;
        let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }?;
        let identity = ContentIdentity::of_bytes(&bytes);
        let Some(workers) = read_stdlib_cache_origins_unlocked(path, &identity.sha256)? else {
            return Ok(None);
        };
        super::validation::validate_stdlib_archive_origins(&bytes, &workers)?;
        Ok(Some(workers))
    })
}

pub(crate) fn read_stdlib_cache_key(stdlib_path: &Path) -> Option<String> {
    read_trimmed_sidecar(&stdlib_cache_key_sidecar_path(stdlib_path))
}

pub(crate) fn read_stdlib_cache_manifest(stdlib_path: &Path) -> Option<String> {
    read_trimmed_sidecar(&stdlib_cache_manifest_sidecar_path(stdlib_path))
}

pub(crate) fn read_stdlib_cache_partition_manifest(stdlib_path: &Path) -> Option<String> {
    read_trimmed_sidecar(&stdlib_cache_partition_manifest_sidecar_path(stdlib_path))
}

fn read_trimmed_sidecar(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
