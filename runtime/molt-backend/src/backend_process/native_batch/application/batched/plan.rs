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
        backend.program_scope = molt_backend::NativeProgramScope::ApplicationArchive;
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
    fn private_provider_is_bound_to_its_body_and_target_in_both_objects() {
        use crate::backend_process::native_batch::NativeBatchInput;
        let ir: SimpleIR = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/checked_provider.json"
        )))
        .unwrap();
        let mut plan =
            NativeApplicationBatchPlan::from_ir(ir, 1, 100, None, None, &mut SimpleBackend::new())
                .unwrap();
        let mut seen = Vec::new();
        for batch in plan.batches {
            let terminal = plan.archive.project(batch).unwrap();
            let names = molt_backend::NativeBackendModuleContext::object_dependencies(
                &terminal.declarations,
            );
            let context = plan.archive.linkage.project_object_dependencies(&names);
            // Use the actual worker's enum. Its untagged source alternative must
            // round trip compact typed function references on the JSON transport.
            let bytes = serde_json::to_vec(&NativeBatchInput::Terminal { terminal }).unwrap();
            let NativeBatchInput::Terminal { terminal } = serde_json::from_slice(&bytes).unwrap()
            else {
                panic!("application source was re-lifted")
            };
            let mut wire = serde_json::to_value(terminal).unwrap();
            let Some(checked) = wire["checked"].as_array().unwrap().first() else {
                continue;
            };
            let id = checked[0].as_u64().unwrap() as usize;
            let is_import = wire["declarations"][id]["is_extern"] == true;
            seen.push(is_import);
            if is_import {
                wire["checked"][0][1]["origin"] = serde_json::json!("forged origin");
            } else {
                let byte = wire["bodies"][id][0].as_u64().unwrap();
                wire["bodies"][id][0] = serde_json::json!(byte ^ 1);
            }
            let mut backend = SimpleBackend::new();
            backend.set_module_context(context);
            assert!(
                backend
                    .compile_terminal_object(serde_json::from_value(wire).unwrap())
                    .err()
                    .expect("forged provider accepted")
                    .contains("invalid private provider commitment")
            );
        }
        seen.sort();
        assert_eq!(seen, [false, true], "one definition and one import");
    }

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
