use std::{io, path::Path};

use molt_backend::SimpleBackend;

use super::super::super::io_limits::{read_json_artifact, write_output_path};
use super::super::batching::{NativeBatchModuleMetadata, NativeBatchObjectJob};

pub(crate) fn compile_native_batch_object_job(
    job: NativeBatchObjectJob,
    output_path: &Path,
) -> io::Result<()> {
    let metadata: NativeBatchModuleMetadata =
        read_json_artifact(&job.module_context_path, "native batch module metadata")?;
    let mut backend = SimpleBackend::new_with_target(job.target_triple.as_deref());
    backend.skip_ir_passes = true;
    backend.skip_shared_stdlib_partition = true;
    backend.emit_app_callable_resolver = job.emit_app_callable_resolver;
    backend.app_callable_manifest = job.app_callable_manifest;
    backend.external_function_names = job.external_function_names;
    backend.module_registry = job.module_registry;
    backend.set_module_context(metadata.module_context);
    let output = backend.compile(job.ir);
    write_output_path(output_path, &output.bytes)
}

pub(crate) fn compile_native_batch_object_job_file(
    job_path: &Path,
    output_path: &Path,
) -> io::Result<()> {
    let job: NativeBatchObjectJob = read_json_artifact(job_path, "native batch object job")?;
    compile_native_batch_object_job(job, output_path)
}
