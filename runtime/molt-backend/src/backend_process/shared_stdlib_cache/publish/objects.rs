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

use super::files::sha256_file_hex;
use super::lock::with_shared_stdlib_cache_publish_lock;
use crate::backend_process::native_batch::{NativeBatchObjectJob, validate_native_object_bytes};

const MAGIC: &[u8; 8] = b"MOLTNO01";
const HEADER_SIZE: usize = 8 + 32 + 32 + 8;

pub(crate) struct StdlibObjectCache {
    root: PathBuf,
    authority: serde_json::Value,
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
        let _admitted_callables = molt_backend::runtime_callable_symbols_from_env();
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
        let parent = archive
            .parent()
            .ok_or_else(|| invalid("stdlib archive has no cache directory"))?;
        Ok(Some(Self {
            root: parent.join("native-stdlib-objects-v1"),
            authority,
        }))
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
        self.root.join(&hex[..2]).join(format!("{hex}.entry"))
    }

    /// Copy an admitted immutable generation to the job's private ordinary
    /// object path. Archive assembly retains its existing format/target checks.
    pub(crate) fn restore(&self, key: &[u8; 32], output: &Path) -> io::Result<Option<u64>> {
        let path = self.path(key);
        with_shared_stdlib_cache_publish_lock(&path, || {
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            // Publication replaces files atomically and never edits a mapped
            // generation. The shared lock also prevents a competing publisher.
            let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }?;
            let object = admitted_object(&bytes, key)?;
            molt_artifact_publish::write_bytes_atomically(output, object)?;
            Ok(Some(object.len() as u64))
        })
    }

    pub(crate) fn publish(&self, key: &[u8; 32], output: &Path) -> io::Result<u64> {
        let file = File::open(output)?;
        let object = unsafe { memmap2::MmapOptions::new().map(&file) }?;
        validate_native_object_bytes(&object)?;
        let output_digest: [u8; 32] = Sha256::digest(&object).into();
        let path = self.path(key);
        with_shared_stdlib_cache_publish_lock(&path, || {
            match File::open(&path) {
                Ok(existing) => {
                    let bytes = unsafe { memmap2::MmapOptions::new().map(&existing) }?;
                    let admitted = admitted_object(&bytes, key)?;
                    if admitted != object.as_ref() {
                        return Err(invalid(
                            "identical native object inputs produced different output bytes",
                        ));
                    }
                    return Ok(object.len() as u64);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            molt_artifact_publish::write_atomically(&path, |writer| {
                writer.write_all(MAGIC)?;
                writer.write_all(key)?;
                writer.write_all(&output_digest)?;
                writer.write_all(&(object.len() as u64).to_le_bytes())?;
                writer.write_all(&object)
            })?;
            Ok(object.len() as u64)
        })
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn admitted_object<'a>(bytes: &'a [u8], key: &[u8; 32]) -> io::Result<&'a [u8]> {
    if bytes.len() < HEADER_SIZE || &bytes[..8] != MAGIC || &bytes[8..40] != key {
        return Err(invalid(
            "native stdlib constituent has an invalid schema or input identity",
        ));
    }
    let length = u64::from_le_bytes(bytes[72..80].try_into().unwrap());
    let object = &bytes[HEADER_SIZE..];
    if length != object.len() as u64 || Sha256::digest(object)[..] != bytes[40..72] {
        return Err(invalid(
            "native stdlib constituent output digest/extent mismatch",
        ));
    }
    validate_native_object_bytes(object)?;
    Ok(object)
}

#[cfg(test)]
#[path = "objects_tests.rs"]
mod tests;
