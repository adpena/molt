use std::collections::BTreeSet;

use super::super::super::config::{DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_STDLIB_BATCH_SIZE};
use super::super::super::native_batch::{
    partition_functions_for_batches, resolved_batch_op_budget_limit, resolved_batch_size_limit,
};

pub(super) struct StdlibBatchPlan {
    pub(super) all_function_names: BTreeSet<String>,
    pub(super) module_context: molt_backend::NativeBackendModuleContext,
    pub(super) batches: Vec<Vec<molt_backend::FunctionIR>>,
}

impl StdlibBatchPlan {
    pub(super) fn from_functions(stdlib_funcs: Vec<molt_backend::FunctionIR>) -> Self {
        let all_function_names = stdlib_funcs.iter().map(|f| f.name.clone()).collect();
        let module_context = molt_backend::SimpleBackend::build_module_context(&stdlib_funcs);
        let batches = partition_functions_for_batches(
            stdlib_funcs,
            stdlib_batch_size(),
            stdlib_batch_ops_budget(),
        );
        Self {
            all_function_names,
            module_context,
            batches,
        }
    }

    pub(super) fn total_batches(&self) -> usize {
        self.batches.len()
    }

    pub(super) fn into_only_batch(mut self) -> Vec<molt_backend::FunctionIR> {
        self.batches.pop().unwrap_or_default()
    }
}

pub(super) fn stdlib_batch_ops_budget() -> usize {
    resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET)
}

fn stdlib_batch_size() -> usize {
    resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE)
}

pub(super) fn log_stdlib_batch(
    log_prefix: &str,
    batch_idx: usize,
    total_batches: usize,
    batch_funcs: &[molt_backend::FunctionIR],
    batch_ops_budget: usize,
) {
    let batch_ops = batch_funcs.iter().map(|f| f.ops.len()).sum::<usize>();
    eprintln!(
        "{log_prefix}: stdlib batch {}/{} ({} functions, {} ops / budget {})",
        batch_idx + 1,
        total_batches,
        batch_funcs.len(),
        batch_ops,
        batch_ops_budget
    );
}
