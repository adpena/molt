use std::sync::Arc;

use molt_ir::addressed_program::{AddressedProgram, ProgramAssemblyError};

use super::super::cache::DaemonCache;
use super::super::{DaemonIrSource, DaemonJobRequest};
use crate::backend_process::input::open_ir_file;

/// The outcome of reading a job's lease against the resident function store.
pub(super) enum DaemonJobDocument {
    Ready(Box<molt_backend::BackendIrDocument>),
    /// Bodies the lease did not ship and the store no longer holds.
    MissingFunctions(Vec<usize>),
}

/// Admit the lease's shipped bodies, fill the rest from the store, and keep
/// every admitted body for later programs. A body evicted since the caller's
/// probe makes the caller ship it; nothing is compiled from a partial program.
pub(super) fn read_daemon_job_document(
    job: &DaemonJobRequest,
    cache: &DaemonCache,
) -> Result<DaemonJobDocument, String> {
    let DaemonIrSource::Lease(path) = &job.ir_source else {
        return Err("missing ir for cache miss".to_string());
    };
    let lease_error =
        |error: String| format!("failed to read MessagePack ir_path {path:?}: {error}");
    let mut file = open_ir_file(path).map_err(|error| lease_error(error.to_string()))?;
    let program = AddressedProgram::from_msgpack_reader(&mut file)
        .map_err(|error| lease_error(format!("invalid msgpack IR: {error}")))?;
    let admitted: Vec<_> = program
        .shipped()
        .map(|(digest, function)| (digest, Arc::clone(function)))
        .collect();
    let mut resident = cache.resident_functions(program.digests(), |digest| !program.ships(digest));
    let assembled = program.assemble(|position, _| resident[position].take());
    cache.insert_functions(admitted);
    match assembled {
        Ok(document) => Ok(DaemonJobDocument::Ready(Box::new(document))),
        Err(ProgramAssemblyError::MissingFunctions(positions)) => {
            Ok(DaemonJobDocument::MissingFunctions(positions))
        }
        Err(ProgramAssemblyError::Invalid(reason)) => Err(lease_error(reason)),
    }
}
