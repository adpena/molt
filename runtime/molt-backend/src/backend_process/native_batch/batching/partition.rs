use molt_ir::deterministic_hash::HashMap;
use std::collections::BTreeSet;

use molt_backend::FunctionIR;
use molt_ir::FunctionId;

/// One extern declaration per row of the source document, in id order: the
/// rows a batch references but does not hold become its extern rows.
pub(crate) struct ExternalFunctionDeclarations {
    rows: Vec<FunctionIR>,
    by_symbol: HashMap<String, FunctionId>,
}

pub(crate) fn external_function_declarations(
    functions: &[FunctionIR],
) -> ExternalFunctionDeclarations {
    let rows = functions
        .iter()
        .map(|func| {
            func.extern_declaration()
                .unwrap_or_else(|error| panic!("invalid batch declaration source: {error}"))
        })
        .collect::<Vec<_>>();
    let mut by_symbol = HashMap::with_capacity_and_hasher(rows.len(), Default::default());
    for (index, row) in rows.iter().enumerate() {
        if by_symbol
            .insert(row.name.clone(), FunctionId::from_index(index))
            .is_some()
        {
            panic!("duplicate batch declaration for {}", row.name);
        }
    }
    ExternalFunctionDeclarations { rows, by_symbol }
}

/// Make `batch_functions` (bodies of the source document) a document of its
/// own: the bodies keep their order as rows, each source row they reference
/// becomes an extern row, and every callee names a row of the batch
/// (HF-847 slice 1, C2). A body's source row is found by its linker symbol,
/// once per body, at this partition boundary.
pub(crate) fn append_referenced_external_declarations(
    batch_functions: &mut Vec<FunctionIR>,
    declarations: &ExternalFunctionDeclarations,
) {
    let bodies = std::mem::take(batch_functions)
        .into_iter()
        .map(|body| {
            let old = *declarations.by_symbol.get(&body.name).unwrap_or_else(|| {
                panic!(
                    "batch body `{}` is not a row of its source document",
                    body.name
                )
            });
            (old, body)
        })
        .collect();
    *batch_functions = molt_ir::function_table::project_document(bodies, |old| {
        declarations.rows[old.index()].clone()
    });
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
