use super::WasmBackend;
use super::control_flow::has_non_linear_control_flow;
use crate::SimpleIR;
use crate::wasm::WasmCompileOutput;
use crate::wasm::lir_fast::compute_lir_wasm_lowering_plans_from_final_ir_with_escaped;

impl WasmBackend {
    pub fn compile(self, ir: SimpleIR) -> Vec<u8> {
        self.compile_with_diagnostics(ir).wasm
    }

    pub fn compile_with_diagnostics(self, ir: SimpleIR) -> WasmCompileOutput {
        let mut ir = ir;
        crate::apply_profile_order(&mut ir);
        for func_ir in &mut ir.functions {
            crate::rewrite_stateful_loops(func_ir);
        }
        for func_ir in &mut ir.functions {
            crate::eliminate_unbound_local_checks(func_ir);
            crate::eliminate_redundant_guard_tags(func_ir);
            crate::elide_dead_struct_allocs(func_ir);
        }
        for func_ir in &mut ir.functions {
            crate::escape_analysis(func_ir);
        }
        for func_ir in &mut ir.functions {
            crate::rc_coalescing(func_ir);
        }
        for func_ir in &mut ir.functions {
            crate::fold_constants(&mut func_ir.ops);
            crate::passes::hoist_loop_invariants(func_ir);
        }
        super::tir_pipeline::run_tir_pipeline(&mut ir);

        // Fuse `obj.method(args)` (get_attr_generic_ptr + callargs_new +
        // callargs_push_pos + call_bind) into a single allocation-free
        // `call_method_ic` op, and `super().method(args)` into
        // `call_super_method_ic` (CPython LOAD_METHOD/CALL_METHOD parity).
        // Run as the LAST SimpleIR transformation before runtime import-surface planning
        // and codegen. TIR has first-class IC opcodes, but this backend consumes
        // the final SimpleIR stream, so fusion belongs after the TIR roundtrip
        // and module-phase inliner have produced that stream (identical placement
        // contract to the native backend, which fuses immediately before
        // `compile_func`). The fused op kinds are recognized as non-removable by
        // `eliminate_dead_ops` because method dispatch runs arbitrary user code,
        // so the dead-op pass below preserves them.
        for func_ir in &mut ir.functions {
            crate::passes::fuse_method_dispatch(func_ir);
        }

        // Megafunction splitting is only sound on the current wasm path for
        // straight-line functions. Non-linear control is lowered into a
        // jumpful/stateful dispatch machine, and the generic sequential chunk
        // stub is not a proven semantics-preserving transform there.
        crate::passes::split_megafunctions_with_filter(&mut ir, |func_ir| {
            !has_non_linear_control_flow(&func_ir.ops)
        });

        // Catalog initializers are address/ModuleId reached and therefore have
        // no ordinary SimpleIR call edge. Keep exactly the canonical catalog
        // roots before lowering the WASM ModuleId dispatch table.
        let module_registry_roots: std::collections::BTreeSet<String> = self
            .module_registry
            .as_ref()
            .map(|registry| registry.init_symbols.iter().cloned().collect())
            .unwrap_or_default();
        crate::eliminate_dead_functions_with_roots(&mut ir, &module_registry_roots);
        crate::eliminate_dead_imports(&mut ir);
        crate::eliminate_dead_ops(&mut ir);

        if let Some(config) = crate::should_dump_ir() {
            for func_ir in &ir.functions {
                if crate::dump_ir_matches(&config, &func_ir.name) {
                    crate::dump_ir_ops(func_ir, &config.mode);
                }
            }
        }

        let trampoline_analysis = super::trampoline_analysis::analyze_wasm_trampolines(&ir);
        let lir_lowering_plans = compute_lir_wasm_lowering_plans_from_final_ir_with_escaped(
            &ir,
            &trampoline_analysis.escaped_callable_targets,
        );
        self.emit_wasm_module(ir, lir_lowering_plans, trampoline_analysis)
    }
}
