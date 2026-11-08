mod debug;
mod local_alloc;
mod requirements;
mod seeds;

use super::{WasmFrameControlMode, WasmFunctionFrame, WasmFunctionFramePlan};
use crate::FunctionIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::frame_locals::{
    WasmFrameAnonymousLocal, WasmFrameLocals, WasmFrameSyntheticLocal,
};
use crate::wasm::local_analysis::{LocalVariableAnalysis, analyze_local_variables};
use crate::wasm::state_dispatch::NonLinearControlPlan;
use debug::emit_seed_debug;
use local_alloc::FrameLocalAllocator;
use molt_tir::tir::simple_def_use::{
    simple_ir_out_result, visit_simple_ir_defined_names, visit_simple_ir_reads,
};
use requirements::FrameRuntimeRequirements;
pub(super) use seeds::FrameConstAnchor;
use seeds::FrameConstSeedPlan;
use wasm_encoder::{Function, ValType};

impl WasmFunctionFramePlan {
    #[cfg(test)]
    pub(in crate::wasm) fn for_codegen_fixture(func_ir: &FunctionIR) -> Self {
        let scalar_plan = ScalarRepresentationPlan::for_function_ir_for_target(
            func_ir,
            &crate::tir::TargetInfo::wasm_release_fast(),
        );
        Self::for_function(func_ir, scalar_plan)
    }

    pub(in crate::wasm) fn for_function(
        func_ir: &FunctionIR,
        scalar_plan: ScalarRepresentationPlan,
    ) -> Self {
        let guard_facts = molt_tir::passes::RuntimeGuardFacts::for_function(func_ir);
        let mut requirements = FrameRuntimeRequirements::default();
        for op in &func_ir.ops {
            requirements.observe_op(&scalar_plan, op);
        }
        let control_mode = if requirements.stateful() {
            WasmFrameControlMode::Stateful
        } else if requirements.jumpful() {
            WasmFrameControlMode::Jumpful
        } else {
            WasmFrameControlMode::Plain
        };
        let control_plan = NonLinearControlPlan::for_function(func_ir, control_mode);
        let needs_dispatch = control_plan
            .as_ref()
            .is_some_and(NonLinearControlPlan::needs_dispatch);
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
            storage,
            defined_vars,
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
        let occupancy = storage.occupancy();
        let mut allocator =
            FrameLocalAllocator::new(&read_vars, &param_set, &storage, dead_sink_idx);
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            visit_simple_ir_reads(op, |read| {
                allocator.ensure(
                    &mut locals,
                    &mut local_types,
                    &mut local_count,
                    read.name,
                    false,
                );
            });
            // Result position is independent of its wire field: iterator and
            // checked results use `var`, unpack results use trailing `args`,
            // and bindings may define both a destination and a snapshot.
            visit_simple_ir_defined_names(op, |name| {
                allocator.ensure(&mut locals, &mut local_types, &mut local_count, name, true);
            });
            if let Some(out) = &op.out {
                let out_local_idx = locals.result_or_sink_slot(simple_ir_out_result(op));
                let is_dead = out_local_idx == dead_sink_idx;
                // An entry seed is a write at function entry: only a value
                // observable before any operation needs one, and only such
                // values are kept out of every other local at that position.
                let seed_at_entry = occupancy.is_none_or(|occupancy| occupancy.is_entry_live(out));
                seed_plan.observe_const_output(
                    op_idx,
                    op,
                    out,
                    out_local_idx,
                    is_dead,
                    seed_at_entry,
                );
            }
        }

        seed_plan.seed_undefined_locals(
            &read_vars,
            &defined_vars,
            &param_set,
            &locals,
            dead_sink_idx,
            occupancy,
        );
        let value_occupancy = storage.into_occupancy();

        requirements.ensure_synthetic_locals(&mut locals, &mut local_types, &mut local_count);

        for scratch in WasmFrameSyntheticLocal::MOLT_SCRATCH {
            locals.ensure_synthetic(scratch, &mut local_types, &mut local_count);
        }

        let guard_profile_local = guard_facts.has_profile_only().then(|| {
            let local = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            local
        });
        let stateful = requirements.stateful();
        let jumpful = requirements.jumpful();
        let owns_frame = func_ir.execution_context == crate::ir::ExecutionContextPolicy::Local;
        // Python frames remain observable during callees, including finalizers.
        let tail_call_eligible = requirements.tail_call_eligible() && !owns_frame;
        let owned_frame_attempt = owns_frame.then(|| {
            let slot = local_count;
            local_types.push(ValType::I32);
            local_count += 1;
            slot
        });
        let frame_homes = func_ir
            .ops
            .iter()
            .filter(|op| {
                matches!(
                    op.kind,
                    molt_ir::OpKind::FrameHomeStore
                        | molt_ir::OpKind::FrameHomeCell
                        | molt_ir::OpKind::FrameHomePrivateCell
                        | molt_ir::OpKind::FrameHomeLoad
                        | molt_ir::OpKind::FrameHomeTake
                        | molt_ir::OpKind::FrameHomeClear
                )
            })
            .filter_map(|op| op.value)
            .max()
            .or_else(|| {
                func_ir
                    .ops
                    .iter()
                    .any(|op| {
                        op.kind == molt_ir::OpKind::FrameSetLocation || op.frame_location.is_some()
                    })
                    .then_some(-1)
            })
            .map(|slot| {
                let local = local_count;
                local_types.push(ValType::I32);
                local_types.push(ValType::I64);
                local_count += 2;
                super::WasmFrameHomes {
                    local,
                    displaced: local + 1,
                    slots: slot + 1,
                    at_entry: !func_ir
                        .ops
                        .iter()
                        .any(|op| op.kind == molt_ir::OpKind::TraceEnterSlot),
                }
            });

        let dispatch_locals = locals.allocate_dispatch_locals(
            stateful,
            needs_dispatch,
            &mut local_types,
            &mut local_count,
        );
        let route_local = locals.allocate_route_local(
            control_plan
                .as_ref()
                .is_some_and(NonLinearControlPlan::needs_route_local),
            &mut local_types,
            &mut local_count,
        );
        let (const_seed_locals, const_anchors, const_anchor_by_op_index) =
            seed_plan.into_frame_plans(stateful || jumpful);

        let const_anchor_region = (!const_anchors.is_empty()).then(|| {
            let base = locals.allocate_anonymous(
                WasmFrameAnonymousLocal::ConstAnchorRegionBase,
                &mut local_types,
                &mut local_count,
            );
            let temporary = locals.allocate_anonymous(
                WasmFrameAnonymousLocal::ConstAnchorTemporary,
                &mut local_types,
                &mut local_count,
            );
            super::WasmConstAnchorRegion::new(base, temporary, const_anchors.len())
        });
        emit_seed_debug(func_ir, &locals, &const_seed_locals, const_anchors.len());

        debug_assert_eq!(needs_dispatch, dispatch_locals.is_some());

        locals.allocate_refcount_scratch(&mut local_types, &mut local_count);
        Self {
            local_types,
            frame: WasmFunctionFrame {
                locals,
                value_occupancy,
                runtime_lookup_only_vars,
                scalar_plan,
                guard_facts,
                guard_profile_local,
                control_mode,
                control_plan,
                tail_call_eligible: tail_call_eligible && const_anchors.is_empty(),
                owned_frame_attempt,
                frame_homes,
                python_eh_boundary: false,
                dispatch_locals,
                route_local,
                const_cache,
                const_seed_locals,
                const_anchors,
                const_anchor_region,
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
    use std::collections::BTreeSet;

    #[test]
    fn frame_slots_follow_reads_and_definitions_not_wire_field_spelling() {
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "field_role_slots".into(),
            params: vec!["source".into()],
            ops: vec![
                OpIR {
                    var: Some("dead_value".into()),
                    out: Some("live_done".into()),
                    args: Some(vec!["source".into()]),
                    ..OpIR::new(molt_ir::OpKind::IterNextUnboxed)
                },
                OpIR {
                    var: Some("dead_checked".into()),
                    out: Some("dead_flag".into()),
                    args: Some(vec!["source".into(), "source".into()]),
                    ..OpIR::new(molt_ir::OpKind::CheckedAdd)
                },
                OpIR {
                    value: Some(2),
                    args: Some(vec![
                        "source".into(),
                        "dead_item".into(),
                        "live_item".into(),
                    ]),
                    ..OpIR::new(molt_ir::OpKind::UnpackSequence)
                },
                OpIR {
                    var: Some("transport_only".into()),
                    args: Some(vec!["live_item".into()]),
                    out: Some("dead_copy".into()),
                    ..OpIR::new(molt_ir::OpKind::CopyVar)
                },
                OpIR {
                    var: Some("dead_binding".into()),
                    out: Some("dead_snapshot".into()),
                    args: Some(vec!["source".into()]),
                    ..OpIR::new(molt_ir::OpKind::StoreVar)
                },
                OpIR {
                    out: Some("output_metadata".into()),
                    args: Some(vec!["source".into(); 3]),
                    ..OpIR::new(molt_ir::OpKind::StoreIndex)
                },
                OpIR {
                    args: Some(vec!["live_done".into()]),
                    ..OpIR::new(molt_ir::OpKind::Ret)
                },
            ],
            ..FunctionIR::default()
        };
        let plan = WasmFunctionFramePlan::for_codegen_fixture(&function);
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

    #[test]
    fn renamed_values_share_physical_locals_by_exact_liveness() {
        let prefixes = ["v", "_v", "_bb", "local_"];
        let names: Vec<String> = (0..48)
            .map(|index| format!("{}{index}", prefixes[index % prefixes.len()]))
            .collect();
        let mut ops = vec![OpIR {
            out: Some(names[0].clone()),
            value: Some(0),
            ..OpIR::new(molt_ir::OpKind::Const)
        }];
        for pair in names.windows(2) {
            ops.push(OpIR {
                args: Some(vec![pair[0].clone()]),
                out: Some(pair[1].clone()),
                ..OpIR::new(molt_ir::OpKind::Copy)
            });
        }
        ops.push(OpIR {
            args: Some(vec![names[names.len() - 1].clone()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        });
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "renamed_chain".into(),
            ops,
            ..FunctionIR::default()
        };
        let plan = WasmFunctionFramePlan::for_codegen_fixture(&function);
        let physical: BTreeSet<u32> = names
            .iter()
            .map(|name| plan.frame.locals[name.as_str()])
            .collect();
        // Each copy reads one value and defines the next. A result never
        // reuses its own operand, so the chain alternates between two locals.
        assert_eq!(physical.len(), 2, "{physical:?}");
    }

    #[test]
    fn one_counted_loop_keeps_local_sharing_for_the_whole_frame() {
        let named = |kind, args: &[&str], var: Option<&str>, out: Option<&str>| OpIR {
            args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
            var: var.map(str::to_string),
            out: out.map(str::to_string),
            ..OpIR::new(kind)
        };
        let copy_chain = |prefix: &str, first: &str, ops: &mut Vec<OpIR>| -> Vec<String> {
            let names: Vec<String> = (0..200).map(|index| format!("{prefix}{index}")).collect();
            let mut previous = first.to_string();
            for name in &names {
                ops.push(named(molt_ir::OpKind::Copy, &[&previous], None, Some(name)));
                previous = name.clone();
            }
            names
        };
        // A module-body shape: long straight-line work on both sides of one
        // counted loop in its normalized final form; the value before the
        // loop is read again after it.
        let mut ops = Vec::new();
        let mut values = copy_chain("before", "start", &mut ops);
        ops.push(named(
            molt_ir::OpKind::StoreVar,
            &["start"],
            Some("carrier"),
            None,
        ));
        ops.push(OpIR {
            value: Some(1),
            ..OpIR::new(molt_ir::OpKind::Label)
        });
        ops.extend([
            named(molt_ir::OpKind::LoopStart, &[], None, None),
            named(
                molt_ir::OpKind::LoadVar,
                &[],
                Some("carrier"),
                Some("index"),
            ),
            named(molt_ir::OpKind::Lt, &["index", "stop"], None, Some("cond")),
            named(molt_ir::OpKind::LoopBreakIfFalse, &["cond"], None, None),
            named(molt_ir::OpKind::Add, &["index", "stop"], None, Some("next")),
            named(molt_ir::OpKind::StoreVar, &["next"], Some("carrier"), None),
            named(molt_ir::OpKind::LoopContinue, &[], None, None),
            named(molt_ir::OpKind::LoopEnd, &[], None, None),
        ]);
        values.extend(copy_chain("after", "before199", &mut ops));
        ops.push(named(molt_ir::OpKind::Ret, &["after199"], None, None));
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "module_body_with_counted_loop".into(),
            params: vec!["start".into(), "stop".into()],
            ops,
            ..FunctionIR::default()
        };
        values.extend(["carrier", "index", "cond", "next"].map(str::to_string));
        let plan = WasmFunctionFramePlan::for_codegen_fixture(&function);
        let physical: BTreeSet<u32> = values
            .iter()
            .map(|name| plan.frame.locals[name.as_str()])
            .collect();
        // One local per name would take 404; the chains alternate between two
        // locals, and the loop adds `before199` plus its own few values.
        assert!(
            physical.len() <= 6,
            "{} locals: {physical:?}",
            physical.len()
        );
    }

    #[test]
    fn jumpful_frames_seed_only_values_observable_at_entry() {
        let label = |id| OpIR {
            value: Some(id),
            ..OpIR::new(molt_ir::OpKind::Label)
        };
        let jump = |id| OpIR {
            value: Some(id),
            ..OpIR::new(molt_ir::OpKind::Jump)
        };
        let constant = |out: &str, value| OpIR {
            out: Some(out.into()),
            value: Some(value),
            ..OpIR::new(molt_ir::OpKind::Const)
        };
        let read = |name: &str| OpIR {
            args: Some(vec![name.into()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        };
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "entry_seeds".into(),
            ops: vec![
                constant("early", 5),
                jump(1),
                label(3),
                read("seeded"),
                label(1),
                OpIR {
                    args: Some(vec!["early".into()]),
                    var: Some("bound".into()),
                    ..OpIR::new(molt_ir::OpKind::StoreVar)
                },
                jump(3),
                // Never executes: `seeded` is observable only through its seed.
                constant("seeded", 7),
                read("bound"),
            ],
            ..FunctionIR::default()
        };
        let plan = WasmFunctionFramePlan::for_codegen_fixture(&function);
        let locals = &plan.frame.locals;
        let seeded: Vec<u32> = plan
            .frame
            .const_seed_locals
            .iter()
            .map(|&(local, _)| local)
            .collect();
        assert_eq!(seeded, vec![locals["seeded"]]);
        assert_ne!(locals["seeded"], locals["early"]);
    }
}
