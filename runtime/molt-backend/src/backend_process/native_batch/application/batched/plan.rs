use std::collections::BTreeSet;

use molt_backend::{SimpleBackend, SimpleIR};

use super::super::super::partition_functions_for_batches;
use super::super::super::{ExternalFunctionDeclarations, external_function_declarations};

pub(super) struct NativeApplicationBatchPlan {
    pub(super) profile: Option<molt_backend::PgoProfileIR>,
    pub(super) all_function_names: BTreeSet<String>,
    pub(super) external_function_declarations: ExternalFunctionDeclarations,
    pub(super) app_callable_manifest: BTreeSet<String>,
    pub(super) module_context: molt_backend::NativeBackendModuleContext,
    pub(super) batches: Vec<Vec<molt_backend::FunctionIR>>,
}

impl NativeApplicationBatchPlan {
    pub(super) fn from_ir(
        ir: SimpleIR,
        batch_size: usize,
        batch_ops_budget: usize,
        module_context: Option<molt_backend::NativeBackendModuleContext>,
    ) -> Self {
        let profile = ir.profile;
        let all_functions = ir.functions;
        let all_function_names = all_functions.iter().map(|f| f.name.clone()).collect();
        let external_function_declarations = external_function_declarations(&all_functions);
        let app_callable_manifest =
            molt_backend::compute_app_callable_manifest_checked(&all_functions);
        let module_context =
            module_context.unwrap_or_else(|| SimpleBackend::build_module_context(&all_functions));
        let body_functions = all_functions
            .into_iter()
            .filter(|func| !func.is_extern)
            .collect();
        let batches = partition_functions_for_batches(body_functions, batch_size, batch_ops_budget);
        Self {
            profile,
            all_function_names,
            external_function_declarations,
            app_callable_manifest,
            module_context,
            batches,
        }
    }

    pub(super) fn total_batches(&self) -> usize {
        self.batches.len()
    }
}
