//! Constituent generations of the shared stdlib archive. The input identity
//! binds the exact self-contained job, admitted compiler/environment manifest,
//! executing backend bytes and effective ISA. One atomic envelope owns both
//! output digest and object bytes; there is no mixed object/sidecar generation.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use molt_ir::content_digest::bytes_to_lower_hex;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::files::{refresh_cache_entry_recency, sha256_file_hex};
use super::lock::with_shared_stdlib_cache_publish_lock;
use super::paths::{
    NATIVE_STDLIB_OBJECT_DIRECTORY, native_stdlib_archive_entry_path, stdlib_archive_cache_root,
};
use crate::backend_process::native_batch::{
    MappedObject, NativeBatchObjectJob, NativeWorkerOrigin, validate_native_object_bytes,
};

const MAGIC: &[u8; 8] = b"MOLTNO02";
const HEADER_SIZE: usize = 8 + 32 + 32 + 8 + 8;
// The worker transport admits at most 16 KiB for its complete origin.
const MAX_ORIGIN_BYTES: usize = 16_384;

pub(crate) struct StdlibObjectCache {
    /// The cache root that holds both constituent and archive generations.
    cache_root: PathBuf,
    authority: serde_json::Value,
}

/// A content-addressed archive generation: the archive assembled from one
/// ordered list of constituent inputs. Its sidecars carry the content key and
/// the program-independent constituent manifest instead of a program's.
pub(crate) struct ContentArchive {
    pub(crate) path: PathBuf,
    pub(crate) key: String,
    pub(crate) manifest: String,
}

struct DigestWriter<'a>(&'a mut Sha256);

impl Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl StdlibObjectCache {
    pub(crate) fn for_archive(
        archive: &Path,
        cache_key: Option<&str>,
        manifest: Option<&str>,
        target: Option<&str>,
    ) -> io::Result<Option<Self>> {
        let Some(manifest) = manifest else {
            // Standalone backend invocations without the CLI's admitted build
            // authority still compile through the same path, without reuse.
            return Ok(None);
        };
        let _admitted_callables = molt_backend::RuntimeCallableSymbols::capture();
        let mut authority: serde_json::Value = serde_json::from_str(manifest)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let fields = authority
            .as_object_mut()
            .ok_or_else(|| invalid("stdlib compiler manifest must be an object"))?;
        if fields.get("schema").and_then(|v| v.as_str()) != Some("stdlib-manifest-v2-archive")
            || fields.get("artifact_kind").and_then(|v| v.as_str()) != Some("archive")
            || fields.get("cache_key").and_then(|v| v.as_str()) != cache_key
            || cache_key.is_none()
            || fields.get("target_triple") != Some(&serde_json::json!(target))
        {
            return Err(invalid(
                "stdlib constituent authority disagrees with archive request",
            ));
        }
        for name in ["cache_variant", "compiler_fingerprint"] {
            if fields
                .get(name)
                .and_then(|v| v.as_str())
                .is_none_or(str::is_empty)
            {
                return Err(invalid(&format!(
                    "stdlib constituent authority is missing {name}"
                )));
            }
        }
        if molt_ir::backend_environment::compilation_diagnostics_requested() {
            eprintln!(
                "MOLT_BACKEND: stdlib object reuse disabled for requested compilation diagnostics"
            );
            return Ok(None);
        }
        // Only the whole-program payload key is replaced by the complete job
        // identity. Preserve every remaining manifest field, including future
        // fields, exact profiles, compiler and canonical codegen environment.
        fields.remove("cache_key");
        let executable = std::env::current_exe()?;
        fields.insert(
            "executing_backend_sha256".into(),
            serde_json::json!(sha256_file_hex(&executable)?),
        );
        fields.insert(
            "effective_codegen".into(),
            molt_backend::SimpleBackend::object_codegen_identity(target),
        );
        Ok(Some(Self {
            cache_root: stdlib_archive_cache_root(archive)?,
            authority,
        }))
    }

    /// The content identity of the archive assembled from constituents with
    /// these input keys, in member order. Each input key binds its complete
    /// job, the program-independent manifest, the executing backend bytes and
    /// the effective ISA, and the archive is a pure function of the ordered
    /// constituent outputs. So two programs with the same stdlib partition
    /// share one archive, as Go's build cache serves an unchanged package
    /// archive by its action ID.
    pub(crate) fn archive_key(inputs: &[[u8; 32]]) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"native-stdlib-archive-input-v1\0");
        digest.update((inputs.len() as u64).to_le_bytes());
        for input in inputs {
            digest.update(input);
        }
        digest.finalize().into()
    }

    pub(crate) fn content_archive(&self, key: &[u8; 32]) -> io::Result<ContentArchive> {
        let key = bytes_to_lower_hex(key);
        Ok(ContentArchive {
            path: native_stdlib_archive_entry_path(&self.cache_root, &key),
            key,
            manifest: serde_json::to_string(&self.authority).map_err(io::Error::other)?,
        })
    }

    pub(crate) fn input_key(&self, job: &NativeBatchObjectJob) -> io::Result<[u8; 32]> {
        let mut digest = Sha256::new();
        digest.update(b"native-stdlib-object-input-v1\0");
        // Named MessagePack is bit-preserving, ordered and includes all job
        // fields automatically. No path substitutes for semantic job content.
        let mut serializer =
            rmp_serde::Serializer::new(DigestWriter(&mut digest)).with_struct_map();
        (&self.authority, job)
            .serialize(&mut serializer)
            .map_err(io::Error::other)?;
        // Bind the existing FunctionIR contract's explicit version as well as
        // its entire ordered payload, never a hand-maintained field subset.
        for function in &job.ir.functions {
            let mut function_digest = Sha256::new();
            molt_backend::ir::write_function_ir_contract(
                function,
                &mut DigestWriter(&mut function_digest),
            )
            .map_err(io::Error::other)?;
            digest.update(function_digest.finalize());
        }
        Ok(digest.finalize().into())
    }

    fn path(&self, key: &[u8; 32]) -> PathBuf {
        let hex = bytes_to_lower_hex(key);
        self.cache_root
            .join(NATIVE_STDLIB_OBJECT_DIRECTORY)
            .join(hex)
            .join("object.entry")
    }

    /// Admit an immutable generation as a mapping of its object bytes.
    ///
    /// Publication replaces files atomically and never edits a generation in
    /// place, and the lock excludes a competing publisher while the bytes are
    /// verified. The mapping then owns that generation's bytes until archive
    /// assembly drops it, even if retention unlinks the entry meanwhile. No
    /// private copy is written: a warm entry costs a mapping and a digest.
    pub(crate) fn admit(
        &self,
        key: &[u8; 32],
    ) -> io::Result<Option<(MappedObject, NativeWorkerOrigin)>> {
        let path = self.path(key);
        with_shared_stdlib_cache_publish_lock(&path, || {
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }?;
            // Older envelopes have no emission origin. They are unadmitted
            // generations, never adapted or relabeled as a current emission.
            if !bytes.starts_with(MAGIC) {
                return Ok(None);
            }
            let (offset, origin) = admitted_object(&bytes, key)?;
            refresh_cache_entry_recency(path.parent().expect("entry owner"));
            Ok(Some((MappedObject::within(bytes, offset)?, origin)))
        })
    }

    pub(crate) fn publish(
        &self,
        key: &[u8; 32],
        output: &Path,
        origin: &NativeWorkerOrigin,
    ) -> io::Result<NativeWorkerOrigin> {
        let file = File::open(output)?;
        let object = unsafe { memmap2::MmapOptions::new().map(&file) }?;
        validate_native_object_bytes(&object)?;
        let output_digest: [u8; 32] = Sha256::digest(&object).into();
        validate_origin_output(origin, &output_digest, object.len())?;
        let encoded = serde_json::to_vec(origin).map_err(io::Error::other)?;
        if encoded.len() > MAX_ORIGIN_BYTES {
            return Err(invalid(
                "native stdlib constituent origin exceeds its bound",
            ));
        }
        let path = self.path(key);
        with_shared_stdlib_cache_publish_lock(&path, || {
            match File::open(&path) {
                Ok(existing) => {
                    let bytes = unsafe { memmap2::MmapOptions::new().map(&existing) }?;
                    if bytes.starts_with(MAGIC) {
                        let (offset, winner) = admitted_object(&bytes, key)?;
                        if &bytes[offset..] != object.as_ref() {
                            return Err(invalid(
                                "identical native object inputs produced different output bytes",
                            ));
                        }
                        // The first generation owns its original emitter even
                        // when another worker independently emits equal bytes.
                        return Ok(winner);
                    }
                    // Unsupported old envelopes are replaced only under this
                    // publication lock, after the complete new origin is bound.
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            molt_artifact_publish::write_atomically(&path, |writer| {
                writer.write_all(MAGIC)?;
                writer.write_all(key)?;
                writer.write_all(&output_digest)?;
                writer.write_all(&(object.len() as u64).to_le_bytes())?;
                writer.write_all(&(encoded.len() as u64).to_le_bytes())?;
                writer.write_all(&encoded)?;
                writer.write_all(&object)
            })?;
            Ok(origin.clone())
        })
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn admitted_object(bytes: &[u8], key: &[u8; 32]) -> io::Result<(usize, NativeWorkerOrigin)> {
    if bytes.len() < HEADER_SIZE || &bytes[..8] != MAGIC || &bytes[8..40] != key {
        return Err(invalid(
            "native stdlib constituent has an invalid schema or input identity",
        ));
    }
    let length = u64::from_le_bytes(bytes[72..80].try_into().unwrap());
    let origin_length = u64::from_le_bytes(bytes[80..88].try_into().unwrap());
    if origin_length == 0 || origin_length > MAX_ORIGIN_BYTES as u64 {
        return Err(invalid(
            "native stdlib constituent origin extent is invalid",
        ));
    }
    let offset = HEADER_SIZE + origin_length as usize;
    let encoded = bytes
        .get(HEADER_SIZE..offset)
        .ok_or_else(|| invalid("native stdlib constituent origin extent exceeds its entry"))?;
    let origin: NativeWorkerOrigin = serde_json::from_slice(encoded).map_err(|error| {
        invalid(&format!(
            "invalid native stdlib constituent origin: {error}"
        ))
    })?;
    let object = &bytes[offset..];
    let digest: [u8; 32] = Sha256::digest(object).into();
    if length != object.len() as u64 || digest[..] != bytes[40..72] {
        return Err(invalid(
            "native stdlib constituent output digest/extent mismatch",
        ));
    }
    validate_native_object_bytes(object)?;
    validate_origin_output(&origin, &digest, object.len())?;
    Ok((offset, origin))
}

fn validate_origin_output(
    origin: &NativeWorkerOrigin,
    digest: &[u8; 32],
    size: usize,
) -> io::Result<()> {
    origin.validate()?;
    if origin.output.size != size as u64 || origin.output.sha256 != bytes_to_lower_hex(digest) {
        return Err(invalid(
            "native stdlib constituent origin names different output bytes",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "objects_tests.rs"]
mod tests;
