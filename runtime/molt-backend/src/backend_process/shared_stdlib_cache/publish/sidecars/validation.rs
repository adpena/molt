use std::path::Path;

use super::super::files::sha256_file_hex;
use super::super::paths::stdlib_cache_archive_digest_sidecar_path;
use super::read::{
    read_stdlib_cache_key, read_stdlib_cache_manifest, read_stdlib_cache_origins_unlocked,
    read_stdlib_cache_partition_manifest,
};

/// The recorded archive digest of a generation whose identity sidecars match
/// the expected key, manifest and partition. Reads only the small sidecars;
/// the payload still needs [`shared_stdlib_archive_payload_matches`].
pub(in super::super) fn shared_stdlib_cache_recorded_digest(
    stdlib_path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition_manifest: Option<&str>,
) -> Option<String> {
    let expected_key = expected_key.filter(|key| !key.is_empty())?;
    let expected_manifest = expected_manifest.filter(|manifest| !manifest.is_empty())?;
    if read_stdlib_cache_key(stdlib_path).as_deref() != Some(expected_key)
        || read_stdlib_cache_manifest(stdlib_path).as_deref() != Some(expected_manifest)
    {
        return None;
    }
    let cached_partition_manifest = read_stdlib_cache_partition_manifest(stdlib_path)?;
    if let Some(expected_partition_manifest) =
        expected_partition_manifest.filter(|manifest| !manifest.is_empty())
        && cached_partition_manifest != expected_partition_manifest
    {
        return None;
    }
    let recorded = std::fs::read_to_string(stdlib_cache_archive_digest_sidecar_path(stdlib_path))
        .ok()?
        .trim()
        .to_string();
    (!recorded.is_empty()).then_some(recorded)
}

/// Whether the archive bytes at `path` have `digest` and pass the producer's
/// archive admission. This reads the whole payload.
pub(in super::super) fn shared_stdlib_archive_payload_matches(path: &Path, digest: &str) -> bool {
    let Ok(actual_archive_digest) = sha256_file_hex(path) else {
        return false;
    };
    if actual_archive_digest != digest {
        return false;
    }
    if let Err(error) = crate::backend_process::native_batch::validate_native_archive_file(path) {
        eprintln!(
            "MOLT_BACKEND: shared stdlib archive admission failed for '{}': {error}",
            path.display()
        );
        return false;
    }
    true
}

pub(in super::super) fn shared_stdlib_cache_matches_unlocked(
    stdlib_path: &Path,
    expected_key: Option<&str>,
    expected_manifest: Option<&str>,
    expected_partition_manifest: Option<&str>,
) -> bool {
    shared_stdlib_cache_recorded_digest(
        stdlib_path,
        expected_key,
        expected_manifest,
        expected_partition_manifest,
    )
    .is_some_and(|digest| {
        let Ok(provenance) = read_stdlib_cache_origins_unlocked(stdlib_path, &digest) else {
            return false;
        };
        match super::super::paths::native_stdlib_archive_content_key(stdlib_path) {
            Err(_) => return false,
            Ok(Some(_)) if provenance.is_none() => return false,
            Ok(_) => {}
        }
        shared_stdlib_archive_payload_matches(stdlib_path, &digest)
    })
}

/// Bind ordered original worker outputs to the actual archive member payloads.
#[cfg(test)]
fn origin_error(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
pub(super) fn validate_stdlib_archive_origins(
    bytes: &[u8],
    workers: &[crate::backend_process::native_batch::NativeWorkerOrigin],
) -> std::io::Result<()> {
    use crate::backend_process::input::ContentIdentity;
    let archive = object::read::archive::ArchiveFile::parse(bytes)
        .map_err(|error| origin_error(&error.to_string()))?;
    if archive.is_thin() {
        return Err(origin_error("stdlib origin cannot bind a thin archive"));
    }
    let mut members = archive.members();
    for (index, worker) in workers.iter().enumerate() {
        worker.validate()?;
        let member = members
            .next()
            .ok_or_else(|| origin_error("stdlib origin has extra workers"))?
            .map_err(|error| origin_error(&error.to_string()))?;
        if member.name() != format!("molt_{index:08}.o").as_bytes() {
            return Err(origin_error("stdlib origin member order differs"));
        }
        let data = member
            .data(bytes)
            .map_err(|error| origin_error(&error.to_string()))?;
        let size = usize::try_from(worker.output.size)
            .map_err(|_| origin_error("stdlib worker output is too large"))?;
        let original = data
            .get(..size)
            .ok_or_else(|| origin_error("stdlib origin member is shorter than emitted output"))?;
        if ContentIdentity::of_bytes(original) != worker.output {
            return Err(origin_error("stdlib origin names different member bytes"));
        }
        if data.len() != size {
            let object =
                object::File::parse(original).map_err(|error| origin_error(&error.to_string()))?;
            // ar_archive_writer's Darwin transport aligns members to eight
            // bytes using newline padding; no other extra bytes are admitted.
            if object.format() != object::BinaryFormat::MachO
                || !matches!(
                    archive.kind(),
                    object::read::archive::ArchiveKind::Bsd
                        | object::read::archive::ArchiveKind::Bsd64
                )
                || data.len() - size != (8 - size % 8) % 8
                || data[size..].iter().any(|byte| *byte != b'\n')
            {
                return Err(origin_error(
                    "stdlib member has bytes outside its emitted output",
                ));
            }
        }
    }
    if members.next().is_some() {
        return Err(origin_error("stdlib origin omits archive members"));
    }
    Ok(())
}
