use super::WasmBackend;
use super::control_flow::has_non_linear_control_flow;
use crate::SimpleIR;
use crate::wasm::WasmCompileOutput;
use crate::wasm::lir_fast::compute_lir_wasm_lowering_plans_from_final_ir_with_escaped;
use crate::wasm_plan::{WasmStageAudit, emit_wasm_stage_audit, simple_ir_stage_shape};

/// Run a body-only operation against defined functions while preserving the
/// original declaration/body ordering in the module IR.
///
/// Extern declarations carry function metadata and no executable body.
/// Temporarily moving declarations out makes that boundary structural even for
/// shared passes whose API accepts an entire `Vec<FunctionIR>`. The operation may
/// analyze, mutate, or reorder defined bodies but must preserve their count.
pub(super) fn with_defined_function_bodies<R>(
    functions: &mut Vec<crate::FunctionIR>,
    operation: impl FnOnce(&mut Vec<crate::FunctionIR>) -> R,
) -> R {
    let original = std::mem::take(functions);
    let declaration_shape = original
        .iter()
        .map(|function| function.is_extern)
        .collect::<Vec<_>>();
    let mut defined = Vec::with_capacity(original.len());
    let mut declarations = Vec::new();
    for function in original {
        if function.is_extern {
            declarations.push(function);
        } else {
            defined.push(function);
        }
    }

    let result = operation(&mut defined);

    let mut defined = defined.into_iter();
    let mut declarations = declarations.into_iter();
    *functions = declaration_shape
        .into_iter()
        .map(|is_extern| {
            if is_extern {
                declarations
                    .next()
                    .expect("WASM extern declaration shape changed during body analysis")
            } else {
                defined
                    .next()
                    .expect("WASM defined-function shape changed during body analysis")
            }
        })
        .collect();
    assert!(
        defined.next().is_none(),
        "WASM body-only analysis added defined functions"
    );
    assert!(
        declarations.next().is_none(),
        "WASM extern declaration restoration left declarations unplaced"
    );
    result
}

fn apply_defined_function_ir_pass(ir: &mut SimpleIR, pass: impl FnOnce(&mut SimpleIR)) {
    with_defined_function_bodies(&mut ir.functions, |defined_functions| {
        let mut defined_ir = SimpleIR {
            functions: std::mem::take(defined_functions),
            profile: None,
        };
        pass(&mut defined_ir);
        *defined_functions = defined_ir.functions;
    });
}

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
        Ok(self.compile_admitted(ir))
    }

    fn compile_admitted(self, ir: SimpleIR) -> WasmCompileOutput {
        let stage_audit = WasmStageAudit::from_environment();
        let mut ir = ir;
        let target_info = crate::tir::target_info::TargetInfo::wasm_release_fast();
        crate::apply_profile_order(&mut ir);
        let source_callables =
            molt_tir::trampolines::CallableMetadata::from_functions(&ir.functions);
        for func_ir in ir
            .functions
            .iter_mut()
            .filter(|function| !function.is_extern)
        {
            crate::rewrite_stateful_loops(func_ir);
        }
        for func_ir in ir
            .functions
            .iter_mut()
            .filter(|function| !function.is_extern)
        {
            crate::eliminate_unbound_local_checks(func_ir);
            crate::eliminate_redundant_guard_tags(func_ir);
            crate::elide_dead_struct_allocs(func_ir);
        }
        for func_ir in ir
            .functions
            .iter_mut()
            .filter(|function| !function.is_extern)
        {
            crate::escape_analysis(func_ir);
        }
        for func_ir in ir
            .functions
            .iter_mut()
            .filter(|function| !function.is_extern)
        {
            crate::rc_coalescing(func_ir);
        }
        for func_ir in ir
            .functions
            .iter_mut()
            .filter(|function| !function.is_extern)
        {
            crate::fold_constants(&mut func_ir.ops);
        }
        // Fuse `obj.method(args)` (get_attr_generic_ptr + callargs_new +
        // callargs_push_pos + call_bind) into a single allocation-free
        // `call_method_ic` op, and `super().method(args)` into
        // `call_super_method_ic` (CPython LOAD_METHOD/CALL_METHOD parity).
        // Fusion rewrites source calls, so it runs before the TIR lift, as on
        // every native lane: the fused call carries its source call's adoption
        // (design 20 §1.6) into ownership planning, and the receiver moves into
        // the call as in CPython's method-form LOAD_ATTR and CALL. The IC opcodes
        // lower back to the same SimpleIR spellings after the roundtrip, and
        // `eliminate_dead_ops` keeps them because method dispatch runs arbitrary
        // user code.
        for func_ir in ir
            .functions
            .iter_mut()
            .filter(|function| !function.is_extern)
        {
            crate::passes::fuse_method_dispatch(func_ir);
        }
        split_wasm_megafunctions(&mut ir);
        super::tir_pipeline::run_tir_pipeline(&mut ir, &target_info, stage_audit);

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
        audit("before-function-reachability", &ir.functions);
        crate::eliminate_dead_functions_with_roots(&mut ir, &module_registry_roots);
        audit("after-function-reachability", &ir.functions);
        audit("before-import-reachability", &ir.functions);
        apply_defined_function_ir_pass(&mut ir, crate::eliminate_dead_imports);
        audit("after-import-reachability", &ir.functions);
        audit("before-operation-reachability", &ir.functions);
        apply_defined_function_ir_pass(&mut ir, |defined_ir| {
            crate::eliminate_dead_ops(defined_ir, &target_info);
        });
        audit("after-operation-reachability", &ir.functions);

        if let Some(config) = crate::should_dump_ir() {
            for func_ir in &ir.functions {
                if crate::dump_ir_matches(&config, &func_ir.name) {
                    crate::dump_ir_ops(func_ir, &config.mode);
                }
            }
        }

        audit("before-trampoline-analysis", &ir.functions);
        let trampoline_analysis =
            super::trampoline_analysis::analyze_wasm_trampolines_with_source(&ir, source_callables);
        audit("after-trampoline-analysis", &ir.functions);
        audit("before-lir-planning", &ir.functions);
        let lir_lowering_plans = compute_lir_wasm_lowering_plans_from_final_ir_with_escaped(
            &ir,
            &trampoline_analysis.escaped_callable_targets,
        );
        audit("after-lir-planning", &ir.functions);
        audit("before-module-emission", &ir.functions);
        let output =
            self.emit_wasm_module(&ir, lir_lowering_plans, trampoline_analysis, stage_audit);
        emit_wasm_stage_audit(
            stage_audit,
            "after-module-emission",
            || simple_ir_stage_shape(&ir.functions),
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
