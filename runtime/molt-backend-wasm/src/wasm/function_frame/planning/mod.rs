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

        let mut seed_plan = FrameConstSeedPlan::default();
        let allocation_policy = FrameLocalAllocationPolicy {
            read_vars: &read_vars,
            param_set: &param_set,
            coalesced_map: &coalesced_map,
            dead_sink_idx,
        };
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            if let Some(var) = &op.var {
                let var_is_dead_out = op.kind == "store_var";
                ensure_frame_local(
                    &mut locals,
                    &mut local_types,
                    &mut local_count,
                    allocation_policy,
                    var,
                    var_is_dead_out,
                );
            }
            if let Some(args) = &op.args {
                for arg in args {
                    ensure_frame_local(
                        &mut locals,
                        &mut local_types,
                        &mut local_count,
                        allocation_policy,
                        arg,
                        false,
                    );
                }
            }
            if let Some(out) = &op.out {
                let out_local_idx = ensure_frame_local(
                    &mut locals,
                    &mut local_types,
                    &mut local_count,
                    allocation_policy,
                    out,
                    true,
                );
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

        let const_cache = locals.allocate_constant_cache(
            requirements.fast_int_count(),
            &mut local_types,
            &mut local_count,
        );

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
