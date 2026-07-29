use std::collections::BTreeSet;

use molt_backend::FunctionIR;

pub(crate) struct ExternalFunctionDeclarations(Vec<FunctionIR>);

impl ExternalFunctionDeclarations {
    fn get(&self, name: &str) -> Option<&FunctionIR> {
        self.0
            .binary_search_by(|declaration| declaration.name.as_str().cmp(name))
            .ok()
            .map(|index| &self.0[index])
    }
}

pub(crate) fn external_function_declarations(
    functions: &[FunctionIR],
) -> ExternalFunctionDeclarations {
    let mut declarations = functions
        .iter()
        .map(|func| {
            func.extern_declaration()
                .unwrap_or_else(|error| panic!("invalid batch declaration source: {error}"))
        })
        .collect::<Vec<_>>();
    declarations.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    if let Some(duplicate) = declarations
        .windows(2)
        .find(|pair| pair[0].name == pair[1].name)
    {
        panic!("duplicate batch declaration for {}", duplicate[0].name);
    }
    ExternalFunctionDeclarations(declarations)
}

pub(crate) fn append_referenced_external_declarations(
    batch_functions: &mut Vec<FunctionIR>,
    declarations: &ExternalFunctionDeclarations,
) {
    let external_declarations = {
        let mut local_names = batch_functions
            .iter()
            .map(|func| func.name.as_str())
            .collect::<Vec<_>>();
        local_names.sort_unstable();

        let mut referenced = batch_functions
            .iter()
            .flat_map(|func| func.ops.iter())
            .filter_map(molt_backend::ir::extern_direct_call_target)
            .filter(|target| local_names.binary_search(target).is_err())
            .filter(|target| declarations.get(target).is_some())
            .collect::<Vec<_>>();
        referenced.sort_unstable();
        referenced.dedup();
        referenced
            .into_iter()
            .map(|name| {
                declarations
                    .get(name)
                    .expect("referenced external declaration was checked above")
                    .clone()
            })
            .collect::<Vec<_>>()
    };
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
