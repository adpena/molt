use std::{
    io::{self, Read},
    path::Path,
};

#[cfg(test)]
use super::super::lock::with_shared_stdlib_cache_publish_lock;
use super::super::paths::{
    native_stdlib_archive_content_key, stdlib_cache_key_sidecar_path,
    stdlib_cache_manifest_sidecar_path, stdlib_cache_origin_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path,
};

use crate::backend_process::input::ContentIdentity;
use crate::backend_process::native_batch::NativeWorkerOrigin;

const ORIGIN_SCHEMA: &str = "molt.stdlib-archive-origin.v2";
pub(super) const MAX_ORIGIN_BYTES: u64 = 64 * 1024 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StdlibArchiveOrigin {
    pub(super) schema: String,
    pub(super) output: ContentIdentity,
    pub(super) workers: Vec<NativeWorkerOrigin>,
    // deserialize_with makes the nullable field required: serde's ordinary
    // Option decoding would silently turn an absent field into None.
    #[serde(deserialize_with = "deserialize_content_key")]
    pub(super) content_key: Option<String>,
}

/// Original closed constituent provenance travels with its archive input key.
/// The key binds the full object-cache authority, not just worker job bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StdlibArchiveProvenance {
    pub(crate) content_key: Option<String>,
    pub(crate) workers: Vec<NativeWorkerOrigin>,
}

fn valid_content_key(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn deserialize_content_key<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <Option<String> as serde::Deserialize>::deserialize(deserializer)?;
    if value.as_deref().is_some_and(|key| !valid_content_key(key)) {
        return Err(serde::de::Error::custom(
            "invalid stdlib archive content key",
        ));
    }
    Ok(value)
}

impl StdlibArchiveOrigin {
    pub(super) fn new(
        output: ContentIdentity,
        provenance: &StdlibArchiveProvenance,
    ) -> io::Result<Self> {
        let workers = &provenance.workers;
        if provenance
            .content_key
            .as_deref()
            .is_some_and(|key| !valid_content_key(key))
        {
            return Err(invalid_origin("invalid stdlib archive content key"));
        }
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
            content_key: provenance.content_key.clone(),
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
) -> io::Result<Option<StdlibArchiveProvenance>> {
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
    if let Some(expected) = native_stdlib_archive_content_key(path)?
        && record.content_key.as_deref() != Some(expected)
    {
        return Err(invalid_origin(
            "stdlib archive content key differs from selected generation",
        ));
    }
    Ok(Some(StdlibArchiveProvenance {
        content_key: record.content_key,
        workers: record.workers,
    }))
}

/// Admit the complete original constituent provenance of this archive snapshot.
/// No process/image admission is granted here; the receiver still replays each
/// worker's original closed supervisor evidence session.
#[cfg(test)]
pub(crate) fn read_shared_stdlib_cache_origins(
    path: &Path,
) -> io::Result<Option<StdlibArchiveProvenance>> {
    with_shared_stdlib_cache_publish_lock(path, || {
        // An ordinary historical archive may have no provenance. Refuse that
        // claim cheaply instead of reading an archive-sized payload first.
        if !stdlib_cache_origin_sidecar_path(path).try_exists()? {
            return Ok(None);
        }
        let file = molt_artifact_publish::open_regular_artifact(path)?;
        let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }?;
        let identity = ContentIdentity::of_bytes(&bytes);
        let Some(provenance) = read_stdlib_cache_origins_unlocked(path, &identity.sha256)? else {
            return Ok(None);
        };
        super::validation::validate_stdlib_archive_origins(&bytes, &provenance.workers)?;
        Ok(Some(provenance))
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
