use std::collections::{BTreeMap, BTreeSet};

use molt_backend::FunctionIR;
use molt_backend::ir::ExecutionContextPolicy;

pub(crate) type InheritedFunctionDeclarations = BTreeMap<String, FunctionIR>;

pub(crate) fn inherited_function_declarations(
    functions: &[FunctionIR],
) -> InheritedFunctionDeclarations {
    functions
        .iter()
        .filter(|func| func.execution_context == ExecutionContextPolicy::Inherited)
        .map(|func| {
            (
                func.name.clone(),
                FunctionIR {
                    name: func.name.clone(),
                    params: func.params.clone(),
                    ops: Vec::new(),
                    param_types: func.param_types.clone(),
                    source_file: func.source_file.clone(),
                    is_extern: true,
                    execution_context: func.execution_context,
                },
            )
        })
        .collect()
}

pub(crate) fn append_referenced_inherited_declarations(
    batch_functions: &mut Vec<FunctionIR>,
    declarations: &InheritedFunctionDeclarations,
) {
    let local_names = batch_functions
        .iter()
        .map(|func| func.name.as_str())
        .collect::<BTreeSet<_>>();
    let referenced = batch_functions
        .iter()
        .flat_map(|func| func.ops.iter())
        .filter(|op| op.passes_execution_context)
        .filter_map(|op| op.s_value.as_deref())
        .filter(|target| !local_names.contains(*target) && declarations.contains_key(*target))
        .collect::<BTreeSet<_>>();
    let external_declarations = referenced
        .into_iter()
        .map(|name| {
            declarations
                .get(name)
                .expect("referenced inherited declaration was checked above")
                .clone()
        })
        .collect::<Vec<_>>();
    batch_functions.extend(external_declarations);
}

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
