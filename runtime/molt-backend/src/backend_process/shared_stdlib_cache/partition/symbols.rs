use molt_backend::SimpleIR;

pub(crate) fn emitted_module_symbol(name: &str) -> Option<&str> {
    name.strip_prefix("molt_init_")
}

pub(crate) fn emitted_name_matches_module_symbol(name: &str, module_symbol: &str) -> bool {
    if let Some(rest) = name.strip_prefix("molt_init_") {
        return rest == module_symbol;
    }
    name.starts_with(&format!("{module_symbol}__"))
}

pub(crate) fn is_user_owned_symbol(
    name: &str,
    entry_module: &str,
    stdlib_module_symbols: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    let entry_init = format!("molt_init_{entry_module}");
    if name == "molt_main"
        || name == "molt_host_init"
        || name.starts_with(&format!("{entry_module}__"))
        || name == entry_init
        || name == "molt_init___main__"
        || name == "molt_isolate_import"
        || name == "molt_isolate_bootstrap"
    {
        return true;
    }
    if let Some(stdlib_module_symbols) = stdlib_module_symbols {
        if let Some(module_symbol) = emitted_module_symbol(name) {
            return !stdlib_module_symbols.contains(module_symbol);
        }
        return !stdlib_module_symbols
            .iter()
            .any(|module_symbol| emitted_name_matches_module_symbol(name, module_symbol));
    }
    false
}

pub(crate) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_module_symbols: Option<&std::collections::BTreeSet<String>>,
    module_registry_roots: &std::collections::BTreeSet<String>,
) -> (Vec<molt_backend::FunctionIR>, Vec<molt_backend::FunctionIR>) {
    molt_backend::inject_runtime_exit(ir);
    // Import bedrock: init bodies are reachable only through the registry
    // blob's MODULE_INIT_TABLE relocations, so the registry's init symbols
    // are dead-function-elimination roots here (invariant I5).
    molt_backend::eliminate_dead_functions_with_roots(ir, module_registry_roots);
    molt_backend::eliminate_dead_imports(ir);
    molt_backend::eliminate_dead_ops(ir);
    let user_func_set: std::collections::BTreeSet<String> = ir
        .functions
        .iter()
        .filter(|f| is_user_owned_symbol(&f.name, entry_module, stdlib_module_symbols))
        .map(|f| f.name.clone())
        .collect();
    let all_funcs: Vec<_> = ir.functions.drain(..).collect();
    let (user_remaining, mut stdlib_funcs): (Vec<_>, Vec<_>) = all_funcs
        .into_iter()
        .partition(|f| user_func_set.contains(&f.name));
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    stdlib_funcs.retain(|f| seen.insert(f.name.clone()));
    (user_remaining, stdlib_funcs)
}
