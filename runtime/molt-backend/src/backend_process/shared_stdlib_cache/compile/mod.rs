mod batched;
mod direct;
mod plan;

use std::io;
use std::path::Path;

use plan::StdlibBatchPlan;

pub(crate) fn compile_stdlib_cache_object(
    stdlib_path: &Path,
    stdlib_funcs: Vec<molt_backend::FunctionIR>,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    log_prefix: &str,
) -> io::Result<()> {
    let stdlib_count = stdlib_funcs.len();
    if stdlib_count == 0 {
        eprintln!("{log_prefix}: stdlib cache is empty (0 reachable functions)");
        return direct::compile_direct_stdlib_cache_object(
            stdlib_path,
            Vec::new(),
            profile,
            target_triple,
        );
    }

    let plan = StdlibBatchPlan::from_functions(stdlib_funcs);
    let stdlib_total_batches = plan.total_batches();
    if stdlib_total_batches == 1 {
        let batch_funcs = plan.into_only_batch();
        plan::log_stdlib_batch(
            log_prefix,
            0,
            1,
            &batch_funcs,
            plan::stdlib_batch_ops_budget(),
        );
        return direct::compile_direct_stdlib_cache_object(
            stdlib_path,
            batch_funcs,
            profile,
            target_triple,
        );
    }

    batched::compile_batched_stdlib_cache_object(
        stdlib_path,
        plan,
        profile,
        target_triple,
        log_prefix,
    )
}
