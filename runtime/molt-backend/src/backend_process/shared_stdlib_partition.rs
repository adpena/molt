use super::*;

#[cfg(feature = "native-backend")]
pub(crate) fn emitted_module_symbol(name: &str) -> Option<&str> {
    name.strip_prefix("molt_init_")
}

#[cfg(feature = "native-backend")]
pub(crate) fn emitted_name_matches_module_symbol(name: &str, module_symbol: &str) -> bool {
    if let Some(rest) = name.strip_prefix("molt_init_") {
        return rest == module_symbol;
    }
    name.starts_with(&format!("{module_symbol}__"))
}

#[cfg(feature = "native-backend")]
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

#[cfg(feature = "native-backend")]
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

#[cfg(feature = "native-backend")]
pub(crate) const STDLIB_PARTITION_MANIFEST_SCHEMA: &str = "stdlib-partition-v1";

#[cfg(feature = "native-backend")]
pub(crate) fn update_fnv1a64(mut hash: u64, bytes: &[u8]) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(feature = "native-backend")]
pub(crate) fn shared_stdlib_partition_manifest(
    stdlib_funcs: &[molt_backend::FunctionIR],
) -> io::Result<String> {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    let mut funcs: Vec<&molt_backend::FunctionIR> = stdlib_funcs.iter().collect();
    funcs.sort_by(|left, right| left.name.cmp(&right.name));

    let mut names: Vec<String> = Vec::with_capacity(funcs.len());
    let mut body_hash = FNV_OFFSET;
    for func in funcs {
        names.push(func.name.clone());
        body_hash = update_fnv1a64(body_hash, func.name.as_bytes());
        body_hash = update_fnv1a64(body_hash, &[0]);
        let body = serde_json::to_vec(&serde_json::json!({
            "name": &func.name,
            "params": &func.params,
            "ops": &func.ops,
            "param_types": &func.param_types,
            "source_file": &func.source_file,
            "is_extern": func.is_extern,
        }))
        .map_err(io::Error::other)?;
        body_hash = update_fnv1a64(body_hash, &body);
        body_hash = update_fnv1a64(body_hash, &[0xff]);
    }

    serde_json::to_string(&serde_json::json!({
        "schema": STDLIB_PARTITION_MANIFEST_SCHEMA,
        "function_count": names.len(),
        "functions": names,
        "body_hash": format!("{body_hash:016x}"),
    }))
    .map_err(io::Error::other)
}

#[cfg(feature = "native-backend")]
pub(crate) fn stdlib_partition_reference_kind(kind: &str) -> bool {
    matches!(
        kind,
        "call"
            | "call_internal"
            | "func_new"
            | "func_new_closure"
            | "func_new_builtin"
            | "code_new"
            | "call_guarded"
            | "call_indirect"
            | "alloc_task"
            | "generator_create"
            | "coro_create"
            | "fn_ptr_code_set"
            | "asyncgen_locals_register"
            | "gen_locals_register"
            | "task_new"
            | "generator_send"
            | "spawn"
            | "call_func"
            | "call_method"
            | "import_from"
            | "import_name"
            | "class_def"
            | "decorator"
            | "super_call"
            | "yield_from"
            | "await"
    )
}

#[cfg(feature = "native-backend")]
pub(crate) fn shared_stdlib_partition_closure_issue(
    stdlib_funcs: &[molt_backend::FunctionIR],
    all_function_names: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let partition_names: std::collections::BTreeSet<&str> =
        stdlib_funcs.iter().map(|func| func.name.as_str()).collect();
    let mut missing: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for func in stdlib_funcs {
        for op in &func.ops {
            if !stdlib_partition_reference_kind(op.kind.as_str()) {
                continue;
            }
            let Some(target) = op.s_value.as_deref() else {
                continue;
            };
            if !all_function_names.contains(target) {
                continue;
            }
            if !partition_names.contains(target) {
                missing.insert(format!("{} -> {}", func.name, target));
            }
        }
    }
    if missing.is_empty() {
        return None;
    }
    let preview: Vec<_> = missing.iter().take(8).cloned().collect();
    let suffix = if missing.len() > preview.len() {
        ", ..."
    } else {
        ""
    };
    Some(format!(
        "shared stdlib partition has unresolved SimpleIR function references: {}{}",
        preview.join(", "),
        suffix
    ))
}

#[cfg(feature = "native-backend")]
pub(crate) fn validate_shared_stdlib_partition(
    stdlib_funcs: &[molt_backend::FunctionIR],
    all_function_names: &std::collections::BTreeSet<String>,
) -> io::Result<()> {
    if let Some(issue) = shared_stdlib_partition_closure_issue(stdlib_funcs, all_function_names) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, issue));
    }
    Ok(())
}

#[cfg(feature = "native-backend")]
pub(crate) fn shared_stdlib_split_function_names(
    user_funcs: &[molt_backend::FunctionIR],
    stdlib_funcs: &[molt_backend::FunctionIR],
) -> std::collections::BTreeSet<String> {
    user_funcs
        .iter()
        .chain(stdlib_funcs.iter())
        .map(|func| func.name.clone())
        .collect()
}
