use molt_backend::{SimpleBackend, SimpleIR};

use super::super::super::stable_native_batches;

pub(super) struct NativeApplicationBatchPlan {
    pub(super) archive: molt_backend::NativeTerminalArchive,
    pub(super) batches: Vec<Vec<molt_backend::TerminalObjectRow>>,
}

impl NativeApplicationBatchPlan {
    pub(super) fn from_ir(
        mut ir: SimpleIR,
        batch_size: usize,
        batch_ops_budget: usize,
        module_context: Option<molt_backend::NativeBackendModuleContext>,
        app_callable_manifest: Option<molt_backend::passes::AppCallableManifest>,
        backend: &mut SimpleBackend,
    ) -> std::io::Result<Self> {
        let linkage = module_context
            .unwrap_or_else(|| SimpleBackend::prepare_module_context(&mut ir.functions));
        backend.app_callable_manifest = app_callable_manifest;
        backend.skip_ir_passes = true;
        backend.skip_shared_stdlib_partition = true;
        let mut archive = backend
            .prepare_terminal_archive(ir, linkage)
            .map_err(std::io::Error::other)?;
        let batches = stable_native_batches(
            std::mem::take(&mut archive.definitions),
            batch_size,
            batch_ops_budget,
        );
        Ok(Self { archive, batches })
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
        assert!(manifest.compiled_bodies.contains("shared_buffer_get"));
        let context = SimpleBackend::prepare_module_context(&mut full.functions);
        full.functions.retain(|function| function.name == "app");
        let plan = NativeApplicationBatchPlan::from_ir(
            full,
            1,
            100,
            Some(context),
            Some(manifest),
            &mut SimpleBackend::new(),
        )
        .unwrap();
        assert!(
            plan.archive
                .callables
                .compiled_bodies
                .contains("shared_buffer_get")
        );
        assert_eq!(plan.batches.len(), 1);
        assert_eq!(plan.batches[0][0].declaration.name, "app");
    }
}
