use super::*;

impl WasmBackend {
    pub fn compile(self, ir: SimpleIR) -> Vec<u8> {
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
        let mut lir_fast_outputs: BTreeMap<String, crate::lower_to_wasm::WasmFunctionOutput> =
            BTreeMap::new();
        super::tir_pipeline::run_tir_pipeline(&mut ir, &mut lir_fast_outputs);

        // Fuse `obj.method(args)` (get_attr_generic_ptr + callargs_new +
        // callargs_push_pos + call_bind) into a single allocation-free
        // `call_method_ic` op, and `super().method(args)` into
        // `call_super_method_ic` (CPython LOAD_METHOD/CALL_METHOD parity).
        // Run as the LAST SimpleIR transformation before runtime import-surface planning
        // and codegen. TIR has first-class IC opcodes, but this backend consumes
        // the final SimpleIR stream, so fusion belongs after the TIR roundtrip
        // and module-phase inliner have produced that stream (identical placement
        // contract to the native backend, which fuses immediately before
        // `compile_func`). The fused op kinds are import dependencies via
        // OP_IMPORT_DEPS, so this must precede module-level runtime surface planning.
        // The IC ops are recognized as non-removable by `eliminate_dead_ops`
        // because method dispatch runs arbitrary user code, so the dead-op pass
        // below preserves them.
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

        // Dead function elimination: remove unreachable functions after inlining.
        crate::eliminate_dead_functions(&mut ir);
        crate::eliminate_dead_imports(&mut ir);
        crate::eliminate_dead_ops(&mut ir);

        if let Some(config) = crate::should_dump_ir() {
            for func_ir in &ir.functions {
                if crate::dump_ir_matches(&config, &func_ir.name) {
                    crate::dump_ir_ops(func_ir, &config.mode);
                }
            }
        }

        // Multi-value return candidate detection (section 3.1).
        // This analysis identifies internal functions whose call sites always
        // destructure the result via 2-3 consecutive tuple_index ops AND whose
        // body always returns via tuple_new of the matching arity.
        let multi_return_candidates = detect_multi_return_candidates(&ir);

        if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() == Ok("1")
            && !multi_return_candidates.is_empty()
        {
            eprintln!(
                "[molt-wasm-multi-return] {} candidate(s) detected:",
                multi_return_candidates.len()
            );
            let mut sorted: Vec<(&String, &usize)> = multi_return_candidates.iter().collect();
            sorted.sort_by_key(|(name, _)| *name);
            for (name, arity) in &sorted {
                eprintln!("  - {name} (returns {arity} values)");
            }
        }

        let trampoline_analysis =
            super::trampoline_analysis::analyze_wasm_trampolines(&ir, multi_return_candidates);
        self.emit_wasm_module(ir, lir_fast_outputs, trampoline_analysis)
    }
}
