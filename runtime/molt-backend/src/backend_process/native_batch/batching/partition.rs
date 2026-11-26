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

pub(crate) fn batch_external_function_names(batch_funcs: &[FunctionIR]) -> BTreeSet<String> {
    batch_funcs
        .iter()
        .filter(|function| function.is_extern)
        .map(|function| function.name.clone())
        .collect()
}
