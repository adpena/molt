use std::io;

fn stdlib_partition_reference_kind(kind: &str) -> bool {
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

pub(crate) fn validate_shared_stdlib_partition(
    stdlib_funcs: &[molt_backend::FunctionIR],
    all_function_names: &std::collections::BTreeSet<String>,
) -> io::Result<()> {
    if let Some(issue) = shared_stdlib_partition_closure_issue(stdlib_funcs, all_function_names) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, issue));
    }
    Ok(())
}

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
