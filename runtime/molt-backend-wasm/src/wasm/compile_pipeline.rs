use super::WasmBackend;
use super::control_flow::has_non_linear_control_flow;
use super::final_functions::WasmFinalFunctions;
use crate::SimpleIR;
use crate::wasm::WasmCompileOutput;
use crate::wasm_plan::{WasmStageAudit, emit_wasm_stage_audit, simple_ir_stage_shape};

impl WasmBackend {
    pub fn compile(self, ir: SimpleIR) -> Vec<u8> {
        self.compile_with_diagnostics(ir).wasm
    }

    pub fn compile_with_diagnostics(self, ir: SimpleIR) -> WasmCompileOutput {
        self.compile_checked(ir)
            .unwrap_or_else(|error| panic!("WASM SimpleIR admission failed: {error}"))
    }

    pub fn compile_checked(
        self,
        ir: SimpleIR,
    ) -> Result<WasmCompileOutput, molt_ir::ir_schema::FunctionOpShapeDiagnostic> {
        molt_ir::ir_schema::validate_simple_ir_op_shapes(&ir)?;
        self.code_slots.validate_functions(&ir.functions)?;
        molt_ir::ir::validate_module_slots(self.module_registry.as_ref(), &ir.functions)?;
        Ok(self.compile_admitted(ir))
    }

    fn compile_admitted(self, ir: SimpleIR) -> WasmCompileOutput {
        let stage_audit = WasmStageAudit::from_environment();
        let mut ir = ir;
        let target_info = crate::tir::target_info::TargetInfo::wasm_release_fast();
        crate::apply_profile_order(&mut ir);
        let source_callables =
            molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions);
        molt_tir::passes::run_simple_ir_pipeline(&mut ir, &target_info);
        split_wasm_megafunctions(&mut ir);
        let typed = super::tir_pipeline::run_tir_pipeline(&mut ir, &target_info, stage_audit);
        let projection_start = stage_audit.start();
        emit_wasm_stage_audit(
            stage_audit,
            "before-final-projection",
            || simple_ir_stage_shape(&ir.functions),
            None,
            None,
            None,
            || Some(0),
        );
        let mut final_functions = WasmFinalFunctions::from_final_tir(ir, typed);
        emit_wasm_stage_audit(
            stage_audit,
            "after-final-projection",
            || simple_ir_stage_shape(&final_functions.ir().functions),
            None,
            None,
            None,
            || projection_start.map(|start| start.elapsed().as_millis()),
        );

        // Existing stage audits bracket the terminal pipeline as well as TIR.
        // A before marker preserves the active boundary on interruption. Normal
        // compilation does not construct audit projections or sample the clock.
        let audit_start = stage_audit.start();
        let audit = |stage, functions: &[crate::FunctionIR]| {
            emit_wasm_stage_audit(
                stage_audit,
                stage,
                || simple_ir_stage_shape(functions),
                None,
                None,
                None,
                || audit_start.map(|start| start.elapsed().as_millis()),
            );
        };

        // Catalog initializers are address/ModuleId reached and therefore have
        // no ordinary SimpleIR call edge. Keep exactly the canonical catalog
        // roots before lowering the WASM ModuleId dispatch table.
        let module_registry_roots: std::collections::BTreeSet<String> = self
            .module_registry
            .as_ref()
            .map(|registry| registry.init_symbols.iter().cloned().collect())
            .unwrap_or_default();
        audit(
            "before-function-reachability",
            &final_functions.ir().functions,
        );
        final_functions.retain_reachable(&module_registry_roots);
        let ir = final_functions.ir();
        audit("after-function-reachability", &ir.functions);

        if let Some(config) = crate::should_dump_ir() {
            for func_ir in &ir.functions {
                if crate::dump_ir_matches(&config, &func_ir.name) {
                    crate::dump_ir_ops(func_ir, &config.mode);
                }
            }
        }

        audit("before-trampoline-analysis", &ir.functions);
        let trampoline_analysis =
            super::trampoline_analysis::analyze_wasm_trampolines_with_source(ir, source_callables);
        audit("after-trampoline-analysis", &ir.functions);
        let stage_shape = stage_audit.capture_simple_ir_shape(&ir.functions);
        final_functions.exclude_escaped_callables(&trampoline_analysis.escaped_callable_targets);
        audit("before-module-emission", &final_functions.ir().functions);
        let output = self.emit_wasm_module(final_functions, trampoline_analysis, stage_audit);
        emit_wasm_stage_audit(
            stage_audit,
            "after-module-emission",
            || stage_shape.as_ref().expect("enabled audit shape").clone(),
            Some(output.wasm.len()),
            None,
            None,
            || audit_start.map(|start| start.elapsed().as_millis()),
        );
        output
    }
}

// One target-admission predicate serves both pre-lift and post-rewrite bounds.
// Sequential splitting is not proven for WASM's nonlinear dispatch machine.
pub(super) fn split_wasm_megafunctions(
    ir: &mut SimpleIR,
) -> std::collections::BTreeMap<String, String> {
    crate::passes::split_megafunctions_with_filter(ir, |function| {
        !function.is_extern && !has_non_linear_control_flow(&function.ops)
    })
}
