use std::sync::Arc;

use molt_ir::addressed_program::{AddressedProgram, ProgramAssemblyError};

use super::super::DaemonIrSource;
use super::super::cache::DaemonCache;
use crate::backend_process::input::content::{AddressedInputIdentity, ContentReader};
use crate::backend_process::input::open_ir_file;

/// Only a complete, validated assembly can carry a codegen input observation.
pub(super) enum DaemonJobDocument {
    Ready(Box<molt_backend::BackendIrDocument>, AddressedInputIdentity),
    MissingFunctions(Vec<usize>),
}

/// Capture the exact lease bytes, admit its shipped bodies, and fill missing
/// positions from the one resident store. No full-program serialization occurs.
pub(super) fn read_daemon_job_document(
    source: &DaemonIrSource,
    cache: &DaemonCache,
) -> Result<DaemonJobDocument, String> {
    let DaemonIrSource::Lease(path) = source else {
        return Err("missing ir for cache miss".to_string());
    };
    let lease_error =
        |error: String| format!("failed to read MessagePack ir_path {path:?}: {error}");
    let file = open_ir_file(path).map_err(|error| lease_error(error.to_string()))?;
    let mut captured = ContentReader::new(file);
    let program = AddressedProgram::from_msgpack_reader(&mut captured)
        .map_err(|error| lease_error(format!("invalid msgpack IR: {error}")))?;
    let lease = captured.identity();
    let function_digests: String = program
        .digests()
        .iter()
        .map(|digest| digest.to_string())
        .collect();
    let admitted: Vec<_> = program
        .shipped()
        .map(|(digest, function)| (digest, Arc::clone(function)))
        .collect();
    let mut resident = cache.resident_functions(program.digests(), |digest| !program.ships(digest));
    let assembled = program.assemble(|position, _| resident[position].take());
    cache.insert_functions(admitted);
    match assembled {
        Ok(document) => Ok(DaemonJobDocument::Ready(
            Box::new(document),
            AddressedInputIdentity::new(lease, function_digests),
        )),
        Err(ProgramAssemblyError::MissingFunctions(positions)) => {
            Ok(DaemonJobDocument::MissingFunctions(positions))
        }
        Err(ProgramAssemblyError::Invalid(reason)) => Err(lease_error(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn captured_msgpack_identity_describes_only_successful_consumed_frame() {
        // Independent literal addressed lease; no production encoder or hasher
        // supplies the expected frame, length or digest.
        let bytes = b"\x82\xaffunction_bodies\x90\xb0function_digests\xc4\x00";
        let cache = DaemonCache::new(None);
        for suffix in [false, true] {
            let path = molt_ir::debug_artifacts::prepare_unique_debug_artifact_path(
                "daemon-input-frame-test",
            )
            .expect("prepare unique input fixture");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.write_all(bytes).unwrap();
            if suffix {
                file.write_all(&[0]).unwrap();
            }
            drop(file);
            let source = DaemonIrSource::Lease(path.to_str().unwrap().to_owned());
            let result = read_daemon_job_document(&source, &cache);
            std::fs::remove_file(&path).expect("remove owned input fixture");
            if suffix {
                assert!(matches!(result, Err(error) if error.contains("trailing bytes")));
                continue;
            }
            let DaemonJobDocument::Ready(document, identity) = result.unwrap() else {
                panic!("empty lease must assemble without missing functions");
            };
            assert!(document.ir.functions.is_empty());
            assert!(identity.function_digests.is_empty());
            assert_eq!(identity.lease.size, 37);
            assert_eq!(
                identity.lease.sha256,
                "34b6abb7d6c673da2c027b4727588df7e32b41cdcd938914e023bed019ef686a"
            );
        }
    }
}
