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
        app_callable_manifest: Option<BTreeSet<String>>,
    ) -> Self {
        let profile = ir.profile;
        let mut all_functions = ir.functions;
        let module_context = module_context
            .unwrap_or_else(|| SimpleBackend::prepare_module_context(&mut all_functions));
        let all_function_names = all_functions.iter().map(|f| f.name.clone()).collect();
        let external_function_declarations = external_function_declarations(&all_functions);
        // Shared-stdlib partitioning has already removed bodies referenced by
        // application metadata. Its full-program manifest must travel with the
        // partition rather than being recomputed from an incomplete universe.
        let app_callable_manifest = app_callable_manifest
            .unwrap_or_else(|| molt_backend::compute_app_callable_manifest_checked(&all_functions));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_stdlib_body_manifest_survives_application_partition() {
        let mut full: SimpleIR = serde_json::from_value(serde_json::json!({
            "functions": [
                {"return_abi": "void", "name": "app", "params": ["kernel"], "ops": [
                    {"kind": "const_str", "out": "descriptor", "s_value":
                        "{\"python_bodies\":{\"buffer_get\":{\"symbol\":\"shared_buffer_get\"}}}"},
                    {"kind": "call", "s_value": "molt_gpu_kernel_descriptor_set",
                        "args": ["kernel", "descriptor"]},
                    {"kind": "ret_void"}
                ]},
                {"return_abi": "void", "name": "shared_buffer_get", "params": [],
                    "ops": [{"kind": "ret_void"}]}
            ]
        }))
        .expect("valid full-program IR");
        let manifest = molt_backend::compute_app_callable_manifest_checked(&full.functions);
        assert!(manifest.contains("shared_buffer_get"));
        let context = SimpleBackend::prepare_module_context(&mut full.functions);
        full.functions.retain(|function| function.name == "app");
        let plan = NativeApplicationBatchPlan::from_ir(full, 1, 100, Some(context), Some(manifest));
        assert!(plan.app_callable_manifest.contains("shared_buffer_get"));
        assert_eq!(plan.batches.len(), 1);
        assert_eq!(plan.batches[0][0].name, "app");
    }
}
