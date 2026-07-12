use std::collections::BTreeSet;

use molt_backend::FunctionIR;

pub(crate) fn partition_functions_for_batches(
    functions: Vec<FunctionIR>,
    max_functions_per_batch: usize,
    max_ops_per_batch: usize,
) -> Vec<Vec<FunctionIR>> {
    let max_functions_per_batch = max_functions_per_batch.max(1);
    let max_ops_per_batch = max_ops_per_batch.max(1);

    let mut batches: Vec<Vec<FunctionIR>> = Vec::new();
    let mut current: Vec<FunctionIR> = Vec::new();
    let mut current_ops = 0usize;

    for func in functions {
        let func_ops = func.ops.len();
        let would_overflow_count = current.len() >= max_functions_per_batch;
        let would_overflow_ops =
            !current.is_empty() && current_ops.saturating_add(func_ops) > max_ops_per_batch;

        if would_overflow_count || would_overflow_ops {
            batches.push(std::mem::take(&mut current));
            current_ops = 0;
        }

        current_ops = current_ops.saturating_add(func_ops);
        current.push(func);
    }

    if !current.is_empty() {
        batches.push(current);
    }

    batches
}

pub(crate) fn batch_external_function_names(
    all_function_names: &BTreeSet<String>,
    batch_funcs: &[FunctionIR],
) -> BTreeSet<String> {
    let batch_names: BTreeSet<&str> = batch_funcs.iter().map(|func| func.name.as_str()).collect();
    all_function_names
        .iter()
        .filter(|name| !batch_names.contains(name.as_str()))
        .cloned()
        .collect()
}

pub(crate) fn deduplicate_functions_by_name(functions: &mut Vec<FunctionIR>) {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    functions.retain(|f| seen.insert(f.name.clone()));
}
