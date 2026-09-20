use super::super::constant_ops::{ConstantOpContext, emit_constant_op};
use super::super::control_flow::ControlKind;
use super::WasmFunctionEmitContext;
use super::call_ops::{CallOpContext, CallOpEmission, CallRetentionLiveness, emit_call_op};
use super::control_ops::{ControlOpContext, emit_control_op};
use super::core_runtime_ops::emit_core_runtime_op;
use super::local_slot_ops::emit_local_slot_op;
use super::local_state_ops::emit_local_state_op;
use super::numeric_ops::emit_numeric_op;
use super::object_attr_ops::emit_object_attr_op;
use super::runtime_service_ops::{RuntimeServiceOpContext, emit_runtime_service_op};
use crate::OpIR;
use molt_tir::tir::op_kinds_generated::simpleir_kind_is_wasm_split_barrier;
use molt_tir::tir::simple_def_use::visit_simple_ir_defined_names;
use std::collections::BTreeMap;
use wasm_encoder::Function;

impl<'a, 'ctx> WasmFunctionEmitContext<'a, 'ctx> {
    pub(in crate::wasm) fn emit_ops(
        &mut self,
        func: &mut Function,
        ops: &[OpIR],
        control_stack: &mut Vec<ControlKind>,
        try_stack: &mut Vec<usize>,
        label_stack: &mut Vec<i64>,
        label_depths: &mut BTreeMap<i64, usize>,
        base_idx: usize,
    ) {
        let backend = &mut self.backend;
        let func_ir = self.func_ir;
        let ctx = self.ctx;
        let call_site_abi = self.call_site_abi;
        let import_ids = self.import_ids;
        let native_callable_imports = ctx.native_callable_imports;
        let frame = self.frame;
        let runtime_lookup_only_vars = frame.runtime_lookup_only_vars();
        let locals = frame.locals();
        let const_cache = frame.const_cache();
        let scalar_plan = frame.scalar_plan();
        let func_index = self.func_index;
        let reloc_enabled = self.reloc_enabled;
        let native_eh_enabled = self.native_eh_enabled;
        let tail_call_enabled = self.tail_call_enabled;
        let tail_call_eligible = frame.tail_call_eligible();
        let tail_call_count = self.tail_call_count;

        // Call-boundary retention is a path-local value-epoch fact, unlike RC
        // coalescing. Build it over the exact plain/jumpful/stateful emission
        // region so future definitions sharing a physical local cannot retain
        // stale bits from an already-released SSA value.
        let call_liveness = CallRetentionLiveness::for_region(ops);
        let mut known_raw_ints: BTreeMap<u32, i64> = BTreeMap::new();
        let mut skip_next = false;

        for (rel_idx, op) in ops.iter().enumerate() {
            let op_idx = base_idx + rel_idx;

            if skip_next {
                skip_next = false;
                continue;
            }

            // These facts describe physical slot contents, not immutable SSA
            // names. Every emitter can write a slot, including binding-only
            // operations and multi-result operations. Invalidate before dispatch
            // so an early handled return cannot leave a stale constant behind.
            // A read/write alias may lose a constant shortcut for this one op,
            // but it still reads the actual incoming value from its local.
            invalidate_raw_int_facts(op, locals, &mut known_raw_ints);

            if emit_numeric_op(
                func,
                op,
                op_idx,
                import_ids,
                locals,
                const_cache,
                scalar_plan,
                reloc_enabled,
                &known_raw_ints,
                &mut backend.numeric_lane_stats,
            ) {
                continue;
            }
            if emit_core_runtime_op(
                func,
                op,
                func_ir,
                import_ids,
                locals,
                scalar_plan,
                reloc_enabled,
                ops,
                op_idx,
            ) {
                continue;
            }
            if emit_object_attr_op(
                backend,
                func,
                op,
                func_ir,
                ctx,
                import_ids,
                locals,
                func_index,
                reloc_enabled,
                op_idx,
            ) {
                continue;
            }
            if emit_local_state_op(
                backend,
                func,
                op,
                import_ids,
                locals,
                const_cache,
                frame,
                func_index,
                reloc_enabled,
            ) {
                continue;
            }
            if emit_local_slot_op(func, op, import_ids, locals, reloc_enabled) {
                continue;
            }

            let mut call_op_context = CallOpContext {
                func_ir,
                call_site_abi,
                import_ids,
                native_callable_imports,
                runtime_lookup_only_vars,
                locals,
                const_cache,
                reloc_enabled,
                func_index,
                func_import_count: backend.func_import_count,
                table_relocations: &mut backend.table_relocations,
                frame,
                tail_call_enabled,
                tail_call_eligible,
                tail_call_count,
                // Call-site adjacency remains function-wide even when
                // stateful/jumpful emission presents one slice at a time.
                ops: &func_ir.ops,
                call_liveness: &call_liveness,
                rc_skip_inc: &self.analysis.rc_skip_inc,
                rc_skip_dec: &self.analysis.rc_skip_dec,
                call_live_idx: rel_idx,
                op_idx,
                try_stack_is_empty: try_stack.is_empty(),
            };
            match emit_call_op(&mut call_op_context, func, op) {
                CallOpEmission::Handled => continue,
                CallOpEmission::HandledAndSkipNext => {
                    skip_next = true;
                    continue;
                }
                CallOpEmission::NotHandled => {}
            }

            if emit_runtime_service_op(
                RuntimeServiceOpContext {
                    call_site_abi,
                    import_ids,
                    locals,
                    const_cache,
                    frame,
                    reloc_enabled,
                    native_eh_enabled,
                    raise_exits_function: try_stack.is_empty(),
                    func_index,
                    func_import_count: backend.func_import_count,
                    table_relocations: &mut backend.table_relocations,
                },
                func,
                op,
            ) {
                continue;
            }

            if emit_constant_op(
                ConstantOpContext {
                    backend,
                    ctx,
                    import_ids,
                    locals,
                    const_cache,
                    func_index,
                    reloc_enabled,
                    anchor_local: frame.const_anchor_for_op(op_idx),
                },
                func,
                op,
                &mut known_raw_ints,
            ) {
                continue;
            }

            emit_control_op(
                ControlOpContext {
                    func_ir,
                    import_ids,
                    locals,
                    const_cache,
                    scalar_plan,
                    frame,
                    control_stack,
                    try_stack,
                    label_stack,
                    label_depths,
                    reloc_enabled,
                    native_eh_enabled,
                    op_idx,
                },
                func,
                op,
            );
        }
    }
}

fn invalidate_raw_int_facts(
    op: &OpIR,
    locals: &crate::wasm::WasmFrameLocals,
    facts: &mut BTreeMap<u32, i64>,
) {
    if simpleir_kind_is_wasm_split_barrier(&op.kind) {
        facts.clear();
    } else {
        visit_simple_ir_defined_names(op, |name| {
            facts.remove(&locals.result_slot(name));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::WasmFrameLocals;

    #[test]
    fn raw_int_facts_follow_canonical_writes_and_control_boundaries() {
        let locals = WasmFrameLocals::from(BTreeMap::from([
            ("source".into(), 0),
            ("slot".into(), 1),
            ("result".into(), 2),
        ]));
        for op in [
            OpIR {
                kind: "store_var".into(),
                var: Some("slot".into()),
                out: Some("result".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "iter_next_unboxed".into(),
                var: Some("slot".into()),
                out: Some("result".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "unpack_sequence".into(),
                args: Some(vec!["source".into(), "slot".into(), "result".into()]),
                value: Some(2),
                ..OpIR::default()
            },
        ] {
            let mut facts = BTreeMap::from([(0, 7), (1, 8), (2, 9)]);
            invalidate_raw_int_facts(&op, &locals, &mut facts);
            assert_eq!(facts, BTreeMap::from([(0, 7)]), "{}", op.kind);
        }
        for kind in ["if", "else", "end_if", "label", "loop_start", "loop_end"] {
            let mut facts = BTreeMap::from([(0, 7)]);
            invalidate_raw_int_facts(
                &OpIR {
                    kind: kind.into(),
                    ..OpIR::default()
                },
                &locals,
                &mut facts,
            );
            assert!(facts.is_empty(), "{kind}");
        }
    }
}
