use std::io;
use std::path::Path;

use super::super::super::{
    NativeBatchJobSpec, NativeBatchWork, release_native_backend_batch_memory_to_os,
    run_native_batch_workers, write_native_archive_objects,
};

pub(super) fn run_native_application_batches(
    output_path: &Path,
    batch_specs: &[NativeBatchJobSpec],
    log_prefix: &str,
) -> io::Result<()> {
    release_native_backend_batch_memory_to_os();

    // Batches compile in parallel worker processes on the request's pool; the
    // archive keeps batch order.
    let works: Vec<NativeBatchWork<'_>> = batch_specs
        .iter()
        .enumerate()
        .map(|(batch_idx, spec)| NativeBatchWork {
            job: &spec.job,
            job_name: &spec.job_name,
            object_path: &spec.object_path,
            progress: format!(
                "{log_prefix}: compiling materialized batch {}/{}",
                batch_idx + 1,
                batch_specs.len()
            ),
        })
        .collect();
    run_native_batch_workers("native application batch worker", &works)?;
    release_native_backend_batch_memory_to_os();

    let batch_paths: Vec<std::path::PathBuf> = batch_specs
        .iter()
        .map(|spec| spec.object_path.clone())
        .collect();
    write_native_archive_objects(output_path, &batch_paths)
}
