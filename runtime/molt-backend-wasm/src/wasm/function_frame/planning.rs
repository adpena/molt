use super::{WasmFrameControlMode, WasmFunctionFrame, WasmFunctionFramePlan};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::context::CompileFuncContext;
use crate::wasm::frame_locals::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm::local_analysis::{LocalVariableAnalysis, analyze_local_variables};
use crate::wasm::multi_return_layout::WasmMultiReturnLayout;
use crate::wasm_plan::wasm_scalar_integer_fast_path_for_op;
use crate::wasm_values::box_none;
use crate::{FunctionIR, OpIR};
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{Function, ValType};

#[derive(Clone, Copy)]
struct FrameLocalAllocationPolicy<'a> {
    read_vars: &'a BTreeSet<String>,
    param_set: &'a BTreeSet<String>,
    coalesced_map: &'a BTreeMap<String, String>,
    dead_sink_idx: u32,
}

fn ensure_frame_local(
    locals: &mut WasmFrameLocals,
    local_types: &mut Vec<ValType>,
    local_count: &mut u32,
    policy: FrameLocalAllocationPolicy<'_>,
    name: &str,
    as_dead_out: bool,
) -> u32 {
    if let Some(&idx) = locals.get(name) {
        return idx;
    }
    if as_dead_out && !policy.read_vars.contains(name) && !policy.param_set.contains(name) {
        locals.insert(name.to_string(), policy.dead_sink_idx);
        return policy.dead_sink_idx;
    }
    if let Some(repr) = policy.coalesced_map.get(name)
        && repr != name
        && let Some(&repr_idx) = locals.get(repr)
    {
        locals.insert(name.to_string(), repr_idx);
        return repr_idx;
    }
    let idx = *local_count;
    locals.insert(name.to_string(), idx);
    local_types.push(ValType::I64);
    *local_count += 1;
    idx
}

impl WasmFunctionFramePlan {
    pub(in crate::wasm) fn for_function(
        func_ir: &FunctionIR,
        ctx: &CompileFuncContext<'_>,
    ) -> Self {
        let mut locals = WasmFrameLocals::new();
        let mut local_count = 0;
        let mut local_types = Vec::new();

        for (idx, name) in func_ir.params.iter().enumerate() {
            locals.insert(name.clone(), idx as u32);
            local_count += 1;
        }

        if func_ir.name.ends_with("_poll") {
            let self_param_idx = locals.get("self").copied().unwrap_or(0);
            locals.insert(WasmFrameLocals::SELF_PARAM_NAME.to_string(), self_param_idx);
            let self_idx = locals.get("self").copied();
            if self_idx.is_none() || self_idx == Some(self_param_idx) {
                locals.insert("self".to_string(), local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
            if local_count == 0 {
                local_count = 1;
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

        let mut needs_field_fast = false;
        let mut needs_alloc_resolve = false;
        let has_arena_eligible = func_ir.ops.iter().any(|op| op.arena_eligible == Some(true));
        let scalar_plan = ScalarRepresentationPlan::for_function_ir(func_ir);
        let mut stateful = false;
        let mut saw_jump_or_label = false;
        let mut fast_int_count: usize = 0;
        let mut const_seed_seen: BTreeSet<String> = BTreeSet::new();
        let mut const_seed_locals_all: Vec<(u32, i64)> = Vec::new();
        let mut seeded_runtime_const_ops: Vec<(usize, OpIR)> = Vec::new();
        let allocation_policy = FrameLocalAllocationPolicy {
            read_vars: &read_vars,
            param_set: &param_set,
            coalesced_map: &coalesced_map,
            dead_sink_idx,
        };
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                fast_int_count += 1;
            }
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
                if let Some(const_policy) = WasmConstOpPolicy::for_op(op) {
                    locals.ensure_literal_scratch_for_policy(
                        out,
                        const_policy,
                        &mut local_types,
                        &mut local_count,
                    );
                    if !const_seed_seen.contains(out) {
                        if let Some(bits) = const_policy.inline_seed_bits(op) {
                            if !is_dead {
                                const_seed_seen.insert(out.clone());
                                const_seed_locals_all.push((out_local_idx, bits));
                            }
                        } else if const_policy.needs_dispatch_runtime_seed() && !is_dead {
                            const_seed_seen.insert(out.clone());
                            seeded_runtime_const_ops.push((op_idx, op.clone()));
                        }
                    }
                }
            }
            match op.kind.as_str() {
                "store" | "store_init" | "load" | "guarded_load" | "guarded_field_get"
                | "guarded_field_set" | "guarded_field_init" => needs_field_fast = true,
                "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
                | "chan_recv_yield" => stateful = true,
                "jump" | "label" => saw_jump_or_label = true,
                "alloc_task" => {
                    let tk = op.task_kind.as_deref().unwrap_or("future");
                    let has_prefix = tk == "generator";
                    let has_args = op.args.as_ref().is_some_and(|a| !a.is_empty());
                    if has_prefix || has_args {
                        needs_alloc_resolve = true;
                    }
                }
                _ => {}
            }
        }

        for undef in used_vars.difference(&defined_vars) {
            if let Some(&local_idx) = locals.get(undef.as_str())
                && local_idx != dead_sink_idx
                && !param_set.contains(undef.as_str())
                && !const_seed_seen.contains(undef)
            {
                const_seed_seen.insert(undef.clone());
                const_seed_locals_all.push((local_idx, box_none()));
            }
        }

        if needs_field_fast {
            locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmTmp0,
                &mut local_types,
                &mut local_count,
            );
            locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmTmp1,
                &mut local_types,
                &mut local_count,
            );
        }

        if needs_alloc_resolve {
            locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmAllocResolve,
                &mut local_types,
                &mut local_count,
            );
        }

        let arena_local: Option<u32> = if has_arena_eligible {
            Some(locals.ensure_synthetic(
                WasmFrameSyntheticLocal::WasmScopeArena,
                &mut local_types,
                &mut local_count,
            ))
        } else {
            None
        };

        for scratch in WasmFrameSyntheticLocal::MOLT_SCRATCH {
            locals.ensure_synthetic(scratch, &mut local_types, &mut local_count);
        }

        let const_cache =
            locals.allocate_constant_cache(fast_int_count, &mut local_types, &mut local_count);

        let jumpful = !stateful && saw_jump_or_label;
        let tail_call_eligible = !stateful;

        if stateful && !locals.contains_key(WasmFrameLocals::SELF_PARAM_NAME) {
            let self_param_idx = locals
                .get("self")
                .copied()
                .or_else(|| {
                    func_ir
                        .params
                        .first()
                        .and_then(|name| locals.get(name))
                        .copied()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "stateful wasm function {} missing self parameter",
                        func_ir.name
                    )
                });
            locals.insert(WasmFrameLocals::SELF_PARAM_NAME.to_string(), self_param_idx);
            if !locals.contains_key("self") {
                locals.insert("self".to_string(), self_param_idx);
            }
        }

        let dispatch_locals =
            locals.allocate_dispatch_locals(stateful, jumpful, &mut local_types, &mut local_count);
        let const_seed_locals = if stateful || jumpful {
            const_seed_locals_all
        } else {
            Vec::new()
        };
        let seeded_runtime_const_ops = if stateful || jumpful {
            seeded_runtime_const_ops
        } else {
            Vec::new()
        };
        let seeded_runtime_const_op_indices: BTreeSet<usize> = seeded_runtime_const_ops
            .iter()
            .map(|(idx, _)| *idx)
            .collect();

        emit_seed_debug(
            func_ir,
            &locals,
            &const_seed_locals,
            seeded_runtime_const_ops.len(),
        );

        let multi_return = WasmMultiReturnLayout::build(
            func_ir,
            ctx.multi_return_candidates,
            &mut locals,
            &mut local_types,
            &mut local_count,
        );

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
                arena_local,
                dispatch_locals,
                const_cache,
                const_seed_locals,
                seeded_runtime_const_ops,
                seeded_runtime_const_op_indices,
                multi_return,
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

fn emit_seed_debug(
    func_ir: &FunctionIR,
    locals: &WasmFrameLocals,
    const_seed_locals: &[(u32, i64)],
    runtime_const_op_count: usize,
) {
    if std::env::var("MOLT_DEBUG_WASM_SEEDS_FUNC").ok().as_deref() != Some(func_ir.name.as_str()) {
        return;
    }
    eprintln!(
        "WASM_SEEDS_FUNC name={} seeds={:?} runtime_const_ops={}",
        func_ir.name, const_seed_locals, runtime_const_op_count
    );
    for name in &func_ir.params {
        if let Some(idx) = locals.get(name) {
            eprintln!("WASM_SEEDS_PARAM name={} slot={}", name, idx);
        }
    }
    let mut slot_to_names: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for local in locals.named_locals() {
        slot_to_names
            .entry(local.slot())
            .or_default()
            .push(local.name().to_string());
    }
    for (slot, _) in const_seed_locals {
        if let Some(names) = slot_to_names.get(slot) {
            eprintln!("WASM_SEEDS_SLOT slot={} names={:?}", slot, names);
        }
    }
}
