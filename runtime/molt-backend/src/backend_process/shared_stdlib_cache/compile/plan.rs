use super::super::super::config::{DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_STDLIB_BATCH_SIZE};
use super::super::super::native_batch::{
    ExternalFunctionDeclarations, external_function_declarations, resolved_batch_op_budget_limit,
    resolved_batch_size_limit,
};

pub(super) struct StdlibBatchPlan {
    pub(super) external_function_declarations: ExternalFunctionDeclarations,
    pub(super) module_context: molt_backend::NativeBackendModuleContext,
    pub(super) batches: Vec<Vec<molt_backend::FunctionIR>>,
}

impl StdlibBatchPlan {
    pub(super) fn from_functions(
        stdlib_funcs: Vec<molt_backend::FunctionIR>,
        module_context: molt_backend::NativeBackendModuleContext,
    ) -> Self {
        let external_function_declarations = external_function_declarations(&stdlib_funcs);
        let body_functions = stdlib_funcs
            .into_iter()
            .filter(|func| !func.is_extern)
            .collect();
        let batches = crate::backend_process::native_batch::stable_native_batches(
            body_functions,
            stdlib_batch_size(),
            stdlib_batch_ops_budget(),
        );
        Self {
            external_function_declarations,
            module_context,
            batches,
        }
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
    batch_functions: usize,
    batch_ops: usize,
    batch_ops_budget: usize,
) {
    eprintln!(
        "{log_prefix}: stdlib batch {}/{} ({} functions, {} ops / budget {})",
        batch_idx + 1,
        total_batches,
        batch_functions,
        batch_ops,
        batch_ops_budget
    );
}
