use super::super::{DaemonIrSource, DaemonJobRequest};
use crate::backend_process::input::{IrFormat, read_backend_ir_document};

pub(super) fn read_daemon_job_document(
    job: &DaemonJobRequest,
) -> Result<molt_backend::BackendIrDocument, String> {
    if let DaemonIrSource::Lease(path) = &job.ir_source {
        return read_backend_ir_document(IrFormat::MessagePack, Some(path))
            .map_err(|error| format!("failed to read MessagePack ir_path {path:?}: {error}"));
    }
    Err("missing ir for cache miss".to_string())
}
