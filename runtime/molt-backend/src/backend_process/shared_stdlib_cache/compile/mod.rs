mod batched;
mod plan;
mod stable_batches;

use std::io;
use std::path::Path;

use super::publish::StdlibObjectCache;

pub(crate) fn compile_stdlib_cache_archive(
    stdlib_path: &Path,
    stdlib_funcs: Vec<molt_backend::FunctionIR>,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    log_prefix: &str,
    module_context: molt_backend::NativeBackendModuleContext,
    cache: Option<&StdlibObjectCache>,
) -> io::Result<()> {
    if cache.is_none() {
        eprintln!(
            "{log_prefix}: stdlib object reuse inactive (no compiler manifest or explicit compilation diagnostics)"
        );
    }
    let plan = plan::StdlibBatchPlan::from_functions(stdlib_funcs, module_context);
    batched::compile_batched_stdlib_cache_archive(
        stdlib_path,
        plan,
        profile,
        target_triple,
        log_prefix,
        cache,
    )
}
