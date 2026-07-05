use std::io;
use std::path::Path;

use super::super::super::{
    NativeBatchJobSpec, merge_relocatable_objects, release_native_backend_batch_memory_to_os,
    run_native_batch_worker_with_failure_artifacts,
};

pub(super) fn run_native_application_batches(
    output_path: &Path,
    batch_specs: &[NativeBatchJobSpec],
    log_prefix: &str,
) -> io::Result<()> {
    release_native_backend_batch_memory_to_os();

    let mut batch_paths: Vec<std::path::PathBuf> = Vec::with_capacity(batch_specs.len());
    for (batch_idx, spec) in batch_specs.iter().enumerate() {
        eprintln!(
            "{log_prefix}: compiling materialized batch {}/{}",
            batch_idx + 1,
            batch_specs.len()
        );
        run_native_batch_worker_with_failure_artifacts(
            "native application batch worker",
            &spec.job_path,
            &spec.object_path,
        )?;
        batch_paths.push(spec.object_path.clone());
        release_native_backend_batch_memory_to_os();
    }

    merge_relocatable_objects(output_path, &batch_paths, None)
}
