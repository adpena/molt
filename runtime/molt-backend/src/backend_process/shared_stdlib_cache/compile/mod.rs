mod batched;
mod plan;
mod stable_batches;

use std::io;

use super::publish::StdlibObjectCache;
pub(crate) use batched::PlannedStdlibArchive;

/// Close and hash the constituent jobs of a stdlib archive in member order.
/// The plan fixes the archive's content key before anything compiles, so an
/// unchanged partition can be served whole; `PlannedStdlibArchive::assemble`
/// restores or compiles its constituents otherwise.
pub(crate) fn plan_stdlib_cache_archive(
    stdlib_funcs: Vec<molt_backend::FunctionIR>,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    module_context: molt_backend::NativeBackendModuleContext,
    cache: Option<&StdlibObjectCache>,
) -> io::Result<PlannedStdlibArchive> {
    let plan = plan::StdlibBatchPlan::from_functions(stdlib_funcs, module_context);
    batched::plan_batched_stdlib_cache_archive(plan, profile, target_triple, cache)
}
