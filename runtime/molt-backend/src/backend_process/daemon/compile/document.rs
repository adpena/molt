use std::sync::Arc;

use molt_ir::addressed_program::{AddressedProgram, ProgramAssemblyError};

use super::super::cache::DaemonCache;
use super::super::{DaemonIrSource, DaemonJobRequest};
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
    job: &DaemonJobRequest,
    cache: &DaemonCache,
) -> Result<DaemonJobDocument, String> {
    let DaemonIrSource::Lease(path) = &job.ir_source else {
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
