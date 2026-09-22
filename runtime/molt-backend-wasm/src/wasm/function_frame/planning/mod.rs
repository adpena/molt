mod debug;
mod local_alloc;
mod requirements;
mod seeds;

use super::{WasmFrameControlMode, WasmFunctionFrame, WasmFunctionFramePlan};
use crate::FunctionIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::frame_locals::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm::local_analysis::{LocalVariableAnalysis, analyze_local_variables};
use debug::emit_seed_debug;
use local_alloc::{FrameLocalAllocationPolicy, ensure_frame_local};
use molt_tir::tir::simple_def_use::{
    simple_ir_out_result, visit_simple_ir_defined_names, visit_simple_ir_reads,
};
use requirements::FrameRuntimeRequirements;
pub(super) use seeds::FrameConstAnchor;
use seeds::FrameConstSeedPlan;
use wasm_encoder::{Function, ValType};

impl WasmFunctionFramePlan {
    pub(in crate::wasm) fn for_function(func_ir: &FunctionIR) -> Self {
        let scalar_plan = ScalarRepresentationPlan::for_function_ir_for_target(
            func_ir,
            &crate::tir::target_info::TargetInfo::wasm_release_fast(),
        );
        let mut requirements = FrameRuntimeRequirements::default();
        for op in &func_ir.ops {
            requirements.observe_op(&scalar_plan, op);
        }
        let mut locals = WasmFrameLocals::new();
        let mut local_count = 0;
        let mut local_types = Vec::new();

        for (idx, name) in func_ir.params.iter().enumerate() {
            locals.insert(name.clone(), idx as u32);
            local_count += 1;
        }

        if requirements.stateful() {
            let self_param_idx = func_ir
                .params
                .first()
                .and_then(|name| locals.get(name))
                .copied()
                .unwrap_or_else(|| {
                    panic!(
                        "stateful wasm function {} missing task parameter",
                        func_ir.name
                    )
                });
            locals.insert(WasmFrameLocals::SELF_PARAM_NAME.to_string(), self_param_idx);
            let self_idx = locals.get("self").copied();
            if self_idx.is_none() || self_idx == Some(self_param_idx) {
                locals.insert("self".to_string(), local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
        }

        let LocalVariableAnalysis {
            read_vars,
            param_set,
            runtime_lookup_only_vars,
            coalesced_map,
            defined_vars,
            used_vars,
        } = analyze_local_variables(func_ir);

        let dead_sink_idx = locals.ensure_synthetic(
            WasmFrameSyntheticLocal::DeadSink,
            &mut local_types,
            &mut local_count,
        );

        let const_cache = locals.allocate_constant_cache(
            requirements.fast_int_count(),
            &mut local_types,
            &mut local_count,
        );

        let mut seed_plan = FrameConstSeedPlan::default();
        let allocation_policy = FrameLocalAllocationPolicy {
            read_vars: &read_vars,
            param_set: &param_set,
            coalesced_map: &coalesced_map,
            dead_sink_idx,
        };
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            visit_simple_ir_reads(op, |read| {
                ensure_frame_local(
                    &mut locals,
                    &mut local_types,
                    &mut local_count,
                    allocation_policy,
                    read.name,
                    false,
                );
            });
            // Result position is independent of its wire field: iterator and
            // checked results use `var`, unpack results use trailing `args`,
            // and bindings may define both a destination and a snapshot.
            visit_simple_ir_defined_names(op, |name| {
                ensure_frame_local(
                    &mut locals,
                    &mut local_types,
                    &mut local_count,
                    allocation_policy,
                    name,
                    true,
                );
            });
            if let Some(out) = &op.out {
                let out_local_idx = locals.result_or_sink_slot(simple_ir_out_result(op));
                let is_dead = out_local_idx == dead_sink_idx;
                seed_plan.observe_const_output(
                    op_idx,
                    op,
                    out,
                    out_local_idx,
                    is_dead,
                    &mut locals,
                    &mut local_types,
                    &mut local_count,
                );
            }
        }

        seed_plan.seed_undefined_locals(
            &used_vars,
            &defined_vars,
            &param_set,
            &locals,
            dead_sink_idx,
        );

        requirements.ensure_synthetic_locals(&mut locals, &mut local_types, &mut local_count);

        for scratch in WasmFrameSyntheticLocal::MOLT_SCRATCH {
            locals.ensure_synthetic(scratch, &mut local_types, &mut local_count);
        }

        let stateful = requirements.stateful();
        let jumpful = requirements.jumpful();
        let tail_call_eligible = requirements.tail_call_eligible();

        let dispatch_locals =
            locals.allocate_dispatch_locals(stateful, jumpful, &mut local_types, &mut local_count);
        let (const_seed_locals, const_anchors, const_anchor_by_op_index) =
            seed_plan.into_frame_plans(stateful || jumpful);

        emit_seed_debug(func_ir, &locals, &const_seed_locals, const_anchors.len());

        let control_mode = if stateful {
            WasmFrameControlMode::Stateful
        } else if jumpful {
            WasmFrameControlMode::Jumpful
        } else {
            WasmFrameControlMode::Plain
        };
        debug_assert_eq!(control_mode.needs_dispatch(), dispatch_locals.is_some());

        let _ = local_count;
        Self {
            local_types,
            frame: WasmFunctionFrame {
                locals,
                runtime_lookup_only_vars,
                scalar_plan,
                control_mode,
                tail_call_eligible,
                dispatch_locals,
                const_cache,
                const_seed_locals,
                const_anchors,
                const_anchor_by_op_index,
            },
        }
    }

    pub(in crate::wasm) fn into_function_and_frame(self) -> (Function, WasmFunctionFrame) {
        (
            Function::new_with_locals_types(self.local_types),
            self.frame,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpIR;

    #[test]
    fn frame_slots_follow_reads_and_definitions_not_wire_field_spelling() {
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "field_role_slots".into(),
            params: vec!["source".into()],
            ops: vec![
                OpIR {
                    kind: "iter_next_unboxed".into(),
                    var: Some("dead_value".into()),
                    out: Some("live_done".into()),
                    args: Some(vec!["source".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "checked_add".into(),
                    var: Some("dead_checked".into()),
                    out: Some("dead_flag".into()),
                    args: Some(vec!["source".into(), "source".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "unpack_sequence".into(),
                    value: Some(2),
                    args: Some(vec![
                        "source".into(),
                        "dead_item".into(),
                        "live_item".into(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy_var".into(),
                    var: Some("transport_only".into()),
                    args: Some(vec!["live_item".into()]),
                    out: Some("dead_copy".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".into(),
                    var: Some("dead_binding".into()),
                    out: Some("dead_snapshot".into()),
                    args: Some(vec!["source".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_index".into(),
                    out: Some("output_metadata".into()),
                    args: Some(vec!["source".into(); 3]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["live_done".into()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        };
        let plan = WasmFunctionFramePlan::for_function(&function);
        let locals = &plan.frame.locals;
        for name in [
            "dead_value",
            "dead_checked",
            "dead_flag",
            "dead_item",
            "dead_copy",
            "dead_binding",
            "dead_snapshot",
        ] {
            assert_eq!(locals.bound_result_slot(Some(name)), None, "{name}");
        }
        for name in ["source", "live_done", "live_item"] {
            assert!(locals.bound_result_slot(Some(name)).is_some(), "{name}");
        }
        for name in ["transport_only", "output_metadata"] {
            assert!(
                locals.get(name).is_none(),
                "metadata allocated a value: {name}"
            );
        }
    }
}
