use molt_backend::SimpleIR;

use molt_backend::stdlib_modules::is_user_owned_function;

pub(crate) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_modules: Option<&std::collections::BTreeSet<String>>,
    module_registry_roots: &std::collections::BTreeSet<String>,
) -> (
    Vec<molt_backend::FunctionIR>,
    Vec<molt_backend::FunctionIR>,
    molt_backend::NativeBackendModuleContext,
) {
    // The partition boundary reads a decoded program (HF-847, D2); idempotent.
    ir.resolve_function_references()
        .unwrap_or_else(|error| panic!("stdlib partition decode failed: {error}"));
    molt_backend::inject_runtime_exit(ir);
    // Import bedrock: init bodies are reachable only through the registry
    // blob's MODULE_INIT_TABLE relocations, so the registry's init symbols
    // are dead-function-elimination roots here (invariant I5).
    molt_backend::eliminate_dead_functions_with_roots(ir, module_registry_roots);
    molt_backend::eliminate_dead_imports(ir);
    let module_context = molt_backend::SimpleBackend::prepare_module_context(&mut ir.functions);
    let (user_remaining, stdlib_funcs) =
        molt_backend::stdlib_modules::partition_user_owned_functions(ir, |function| {
            is_user_owned_function(function, entry_module, stdlib_modules)
        });
    (user_remaining, stdlib_funcs, module_context)
}
