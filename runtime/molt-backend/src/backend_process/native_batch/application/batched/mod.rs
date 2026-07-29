mod jobs;
mod plan;
mod run;

use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use molt_backend::SimpleIR;

use super::super::{NativeApplicationObjectOptions, NativeApplicationObjectResult};
use plan::NativeApplicationBatchPlan;

pub(crate) fn compile_batched_native_application_object_to_path(
    ir: SimpleIR,
    output_path: &Path,
    options: &mut NativeApplicationObjectOptions<'_>,
    function_count: usize,
    batch_size: usize,
    batch_ops_budget: usize,
) -> io::Result<NativeApplicationObjectResult> {
    let mut plan = NativeApplicationBatchPlan::from_ir(
        ir,
        batch_size,
        batch_ops_budget,
        options.module_context.take(),
    );
    let total_batches = plan.total_batches();
    if options.app_callable_manifest.is_none() {
        options.app_callable_manifest = Some(std::mem::take(&mut plan.app_callable_manifest));
    }

    let tmp_dir = native_application_batch_temp_dir();
    std::fs::create_dir_all(&tmp_dir)?;
    let compile_result = (|| -> io::Result<()> {
        let batch_specs = jobs::materialize_native_application_batch_jobs(
            &tmp_dir,
            plan,
            options,
            batch_ops_budget,
        )?;
        run::run_native_application_batches(output_path, &batch_specs, options.log_prefix)
    })();

    super::super::finish_native_batch_temp_dir(
        &tmp_dir,
        "native application batch temp dir",
        compile_result,
    )?;

    eprintln!(
        "Successfully compiled to {} ({} functions, {} batches)",
        output_path.display(),
        function_count,
        total_batches
    );
    Ok(NativeApplicationObjectResult {
        function_count,
        batch_count: total_batches,
    })
}

fn native_application_batch_temp_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "molt_batch_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}
