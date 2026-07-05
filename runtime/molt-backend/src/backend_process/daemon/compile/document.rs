use super::super::DaemonJobRequest;
use super::ir::backend_ir_document_from_json_path;

pub(super) fn take_daemon_job_document(
    job: &mut DaemonJobRequest,
) -> Result<molt_backend::BackendIrDocument, String> {
    if let Some(document) = job.ir.take() {
        return Ok(document);
    }
    if let Some(ir_path) = job.ir_path.as_deref() {
        return backend_ir_document_from_json_path(ir_path);
    }
    Err("missing ir for cache miss".to_string())
}
