use molt_backend::passes::AppCallableManifest;
use molt_backend::{FunctionIR, NativeBackendModuleContext, SimpleIR};

use molt_backend::stdlib_modules::is_user_owned_function;

pub(crate) struct ReachableStdlibPartition {
    pub(crate) user_remaining: Vec<FunctionIR>,
    pub(crate) stdlib_funcs: Vec<FunctionIR>,
    pub(crate) module_context: NativeBackendModuleContext,
    pub(crate) app_callable_manifest: AppCallableManifest,
}

pub(crate) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_modules: Option<&std::collections::BTreeSet<String>>,
    module_registry_roots: &std::collections::BTreeSet<String>,
) -> ReachableStdlibPartition {
    // The partition boundary reads a decoded program (HF-847, D2); idempotent.
    ir.resolve_function_references()
        .unwrap_or_else(|error| panic!("stdlib partition decode failed: {error}"));
    // Import bedrock: init bodies are reachable only through the registry
    // blob's MODULE_INIT_TABLE relocations, so the registry's init symbols
    // are dead-function-elimination roots here (invariant I5).
    molt_backend::eliminate_dead_functions_with_roots(ir, module_registry_roots);
    // Compute the app resolver on the reached program while all app and
    // stdlib definitions are still visible in the same function table.
    let app_callable_manifest = molt_backend::compute_app_callable_manifest_checked(&ir.functions);
    let module_context = molt_backend::SimpleBackend::prepare_module_context(&mut ir.functions);
    let (user_remaining, stdlib_funcs) =
        molt_backend::stdlib_modules::partition_user_owned_functions(ir, |function| {
            is_user_owned_function(function, entry_module, stdlib_modules)
        });
    ReachableStdlibPartition {
        user_remaining,
        stdlib_funcs,
        module_context,
        app_callable_manifest,
    }
}
