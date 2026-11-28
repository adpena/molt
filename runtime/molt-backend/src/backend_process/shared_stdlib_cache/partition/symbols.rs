use molt_backend::passes::AppCallableManifest;
use molt_backend::tir::cache::CompilationCache;
use molt_backend::{FunctionIR, NativeBackendModuleContext, SimpleIR};

use molt_backend::stdlib_modules::is_user_owned_function;
use molt_ir::ir::FunctionIrContractDigest;

pub(crate) struct ReachableStdlibPartition {
    pub(crate) user_remaining: Vec<FunctionIR>,
    pub(crate) stdlib_funcs: Vec<FunctionIR>,
    pub(crate) user_facts: Option<Vec<molt_backend::NativeSourceFunction>>,
    pub(crate) stdlib_contracts: Option<Vec<Option<FunctionIrContractDigest>>>,
    pub(crate) module_context: NativeBackendModuleContext,
    pub(crate) app_callable_manifest: AppCallableManifest,
}

pub(crate) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_modules: Option<&std::collections::BTreeSet<String>>,
    module_registry_roots: &std::collections::BTreeSet<String>,
    carrier_cache: Option<&mut CompilationCache>,
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
    let (module_context, source_facts) =
        molt_backend::SimpleBackend::prepare_module_context_with_source_facts(
            &mut ir.functions,
            carrier_cache,
        );
    let mut user_facts = Vec::new();
    let mut stdlib_contracts = Vec::new();
    let (user_remaining, stdlib_funcs) =
        molt_backend::stdlib_modules::partition_user_owned_functions_with(
            ir,
            |function| is_user_owned_function(function, entry_module, stdlib_modules),
            |source, user_owned| {
                if let Some(rows) = &source_facts {
                    let facts = rows[source.index()];
                    if user_owned {
                        user_facts.push(facts);
                    } else {
                        stdlib_contracts.push(facts.contract);
                    }
                }
            },
        );
    let user_facts = source_facts.as_ref().map(|_| {
        user_facts.resize(
            user_remaining.len(),
            molt_backend::NativeSourceFunction::default(),
        );
        user_facts
    });
    let stdlib_contracts = stdlib_contracts
        .iter()
        .any(Option::is_some)
        .then_some(stdlib_contracts);
    ReachableStdlibPartition {
        user_remaining,
        stdlib_funcs,
        user_facts,
        stdlib_contracts,
        module_context,
        app_callable_manifest,
    }
}
