use molt_backend::SimpleIR;

pub(crate) use molt_backend::stdlib_module_symbols::is_user_owned_symbol;

pub(crate) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_module_symbols: Option<&std::collections::BTreeSet<String>>,
    module_registry_roots: &std::collections::BTreeSet<String>,
) -> (
    Vec<molt_backend::FunctionIR>,
    Vec<molt_backend::FunctionIR>,
    molt_backend::NativeBackendModuleContext,
) {
    molt_backend::inject_runtime_exit(ir);
    // Import bedrock: init bodies are reachable only through the registry
    // blob's MODULE_INIT_TABLE relocations, so the registry's init symbols
    // are dead-function-elimination roots here (invariant I5).
    molt_backend::eliminate_dead_functions_with_roots(ir, module_registry_roots);
    molt_backend::eliminate_dead_imports(ir);
    let module_context = molt_backend::SimpleBackend::prepare_module_context(&mut ir.functions);
    molt_backend::eliminate_dead_ops(
        ir,
        &molt_backend::tir::target_info::TargetInfo::native_release_fast(),
    );
    let user_func_set: std::collections::BTreeSet<String> = ir
        .functions
        .iter()
        .filter(|f| {
            is_user_owned_symbol(
                module_context.original_function_name(&f.name),
                entry_module,
                stdlib_module_symbols,
            )
        })
        .map(|f| f.name.clone())
        .collect();
    let all_funcs: Vec<_> = ir.functions.drain(..).collect();
    let (user_remaining, mut stdlib_funcs): (Vec<_>, Vec<_>) = all_funcs
        .into_iter()
        .partition(|f| user_func_set.contains(&f.name));
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    stdlib_funcs.retain(|f| seen.insert(f.name.clone()));
    (user_remaining, stdlib_funcs, module_context)
}
