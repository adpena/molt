use std::io;
use std::path::Path;

use molt_backend::SimpleIR;

use super::super::config::{DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_BACKEND_BATCH_SIZE};
use super::{
    NativeApplicationObjectOptions, NativeApplicationObjectResult, resolved_batch_op_budget_limit,
    resolved_batch_size_limit,
};

mod batched;
mod ir;
mod single;

pub(crate) fn compile_native_application_object_to_path(
    mut ir: SimpleIR,
    output_path: &Path,
    mut options: NativeApplicationObjectOptions<'_>,
) -> io::Result<NativeApplicationObjectResult> {
    if options.stdlib_split_enabled && options.app_callable_manifest.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stdlib-split native application object requires a full-set app callable manifest",
        ));
    }

    ir::prepare_native_application_ir(&mut ir, &options);
    let stats = ir::native_application_stats(&ir);
    let batch_size = resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE);
    let batch_ops_budget = resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET);
    if stats.fits_single_batch(batch_size, batch_ops_budget) {
        return single::compile_single_native_application_object_to_path(
            ir,
            output_path,
            options,
            stats.function_count,
        );
    }

    batched::compile_batched_native_application_object_to_path(
        ir,
        output_path,
        &mut options,
        stats.function_count,
        batch_size,
        batch_ops_budget,
    )
}
