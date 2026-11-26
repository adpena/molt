use super::*;
use crate::tir::values::TirValue;

#[test]
fn call_site_identity_rejects_opaque_gpu_spoofs_and_preserves_internal_calls() {
    for kind in [
        molt_ir::OpKind::CallFunc,
        molt_ir::OpKind::CallFunction,
        molt_ir::OpKind::CallIndirect,
        molt_ir::OpKind::CallBind,
        molt_ir::OpKind::CallGuarded,
        molt_ir::OpKind::InvokeFfi,
    ] {
        for symbol in ["molt_gpu_thread_id", "constfn"] {
            let mut caller = caller_calling_const(symbol);
            caller.blocks.get_mut(&caller.entry_block).unwrap().ops[0].origin = Some(kind);
            assert!(
                sites_in(caller.clone(), &[symbol]).1.is_empty(),
                "{kind}: {symbol}"
            );
        }
    }
    for (gpu_kind, symbol) in [
        (molt_ir::OpKind::GpuThreadId, "molt_gpu_thread_id"),
        (molt_ir::OpKind::GpuBlockId, "molt_gpu_block_id"),
        (molt_ir::OpKind::GpuBlockDim, "molt_gpu_block_dim"),
        (molt_ir::OpKind::GpuGridDim, "molt_gpu_grid_dim"),
        (molt_ir::OpKind::GpuBarrier, "molt_gpu_barrier"),
    ] {
        for original in [
            None,
            Some(molt_ir::OpKind::Call),
            Some(molt_ir::OpKind::CallInternal),
        ] {
            let mut caller = caller_calling_const(symbol);
            if let Some(kind) = original {
                caller.blocks.get_mut(&caller.entry_block).unwrap().ops[0].origin = Some(kind);
            }
            let sites = sites_in(caller.clone(), &[symbol]).1;
            assert_eq!(sites.len(), 1, "{symbol}: {original:?}");
        }
        let mut intrinsic = caller_calling_const(symbol);
        intrinsic
            .blocks
            .get_mut(&intrinsic.entry_block)
            .unwrap()
            .ops[0]
            .origin = Some(gpu_kind);
        assert!(
            sites_in(intrinsic.clone(), &[symbol]).1.is_empty(),
            "{gpu_kind}"
        );
    }
    let mut caller = caller_calling_const("constfn");
    let op = &mut caller.blocks.get_mut(&caller.entry_block).unwrap().ops[0];
    op.opcode = OpCode::Copy;
    op.origin = Some(molt_ir::OpKind::CallInternal);
    assert!(sites_in(caller.clone(), &["constfn"]).1.is_empty());
}

#[test]
fn run_inliner_inlines_const_call() {
    // g() { x = constfn(); return x + 1 }, constfn() = 42.
    // After inlining + re-running the pipeline, the Call is gone, the merged
    // function is valid SSA, and the callee's `const 42` now lives inside g
    // (the call boundary is eliminated). SCCP folds `const(42) + 1` across the
    // continuation block argument, and DCE then deletes the dead argument and
    // the constant it carried.
    let callee = const_callee();
    let caller = caller_calling_const("constfn");
    let mut m = module(vec![caller, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::default());
    assert_eq!(stats.sites_inlined, 1, "one site inlined");
    assert_eq!(stats.functions_changed, 1, "g changed");
    // No Call op remains in g.
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "constfn call eliminated from g");
    // g is valid SSA after the pipeline re-run.
    crate::tir::verify::verify_function(g)
        .unwrap_or_else(|e| panic!("g invalid after inlining: {e:?}"));
    // The inlined `const 42` folded into the sum; nothing carries it.
    assert_eq!(const_ints(g), [43], "constfn() + 1 folds to 43");
}

#[test]
fn run_inliner_preserves_typed_execution_context_boundaries() {
    for policy in [
        molt_ir::ExecutionContextPolicy::Local,
        molt_ir::ExecutionContextPolicy::Inherited,
    ] {
        let mut callee = const_callee();
        callee.execution_context = policy;
        let caller = caller_calling_const("constfn");
        let mut module = module(vec![caller, callee]);
        let (call_graph, summaries) = analysis(&module);

        let stats = run_inliner(
            &mut module,
            &call_graph,
            &summaries,
            &TargetInfo::native_release_fast(),
            &HashSet::default(),
        );

        assert_eq!(stats.sites_inlined, 0, "policy {policy:?}");
        let caller = module.functions.iter().find(|f| f.name == "g").unwrap();
        assert!(
            caller
                .blocks
                .values()
                .flat_map(|block| &block.ops)
                .any(|op| op.opcode == OpCode::Call),
            "typed execution-context call must survive inlining for {policy:?}"
        );
    }
}

#[test]
fn run_inliner_preserves_generic_add_and_inlines_identity_with_args() {
    // Both cases retain the same two-parameter call boundary. Generic Add
    // may dispatch Python; Is is callback-free for every possible argument.
    for (callee, inlines) in [(add_callee(), false), (identity_callee(), true)] {
        let callee_name = callee.name.clone();
        let mut g = TirFunction::new(
            "g".into(),
            vec![TirType::I64, TirType::I64],
            callee.return_type.clone(),
            molt_ir::FunctionReturnAbi::Value,
        );
        let p = ValueId(0);
        let q = ValueId(1);
        let res = g.fresh_value();
        g.value_types.insert(res, callee.return_type.clone());
        let entry = g.entry_block;
        let mut call_attrs = AttrDict::default();
        call_attrs.insert("s_value".into(), AttrValue::Str(callee_name.clone()));
        let block = g.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![p, q],
            results: vec![res],
            attrs: call_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![res] };
        let mut m = module(vec![g, callee]);
        for function in &m.functions {
            crate::tir::verify::verify_function(function).unwrap();
        }
        let before: Vec<_> = m
            .functions
            .iter()
            .map(crate::tir::printer::print_function)
            .collect();
        let (cg, sm) = analysis(&m);
        assert_eq!(cg.has_opaque_call(callee_name.as_str()), !inlines);
        let tti = TargetInfo::native_release_fast();
        assert_eq!(
            classify_inline_eligibility(m.ids[1], &m.functions[1], &cg, &sm, &tti),
            if inlines {
                crate::tir::call_facts::InlineEligibility::Eligible
            } else {
                crate::tir::call_facts::InlineEligibility::WhyNot(
                    crate::tir::call_facts::InlineWhyNot::Recursive,
                )
            }
        );
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::default());
        assert_eq!(stats.sites_inlined, usize::from(inlines));
        assert_eq!(stats.functions_changed, usize::from(inlines));
        let g = m.functions.iter().find(|f| f.name == "g").unwrap();
        let calls: Vec<_> = g
            .blocks
            .values()
            .flat_map(|b| &b.ops)
            .filter(|op| op.opcode == OpCode::Call)
            .collect();
        assert_eq!(calls.len(), usize::from(!inlines));
        crate::tir::verify::verify_function(g).unwrap_or_else(|e| panic!("g invalid: {e:?}"));
        if inlines {
            assert!(
                g.blocks
                    .values()
                    .flat_map(|b| &b.ops)
                    .any(|op| op.opcode == OpCode::Is && op.operands == vec![p, q]),
                "inlined identity uses the caller's two parameters directly"
            );
        } else {
            assert_eq!(calls[0].operands, vec![p, q]);
            assert_eq!(calls[0].results, vec![res]);
            assert_eq!(
                m.functions
                    .iter()
                    .map(crate::tir::printer::print_function)
                    .collect::<Vec<_>>(),
                before,
                "generic arithmetic refusal must preserve both complete source bodies"
            );
        }
    }
}

#[test]
fn run_inliner_uses_hot_budget_for_numeric_raw_lane_call_results() {
    let callee = over_base_budget_float_callee("eval_like");
    let mut caller = loop_numeric_caller("eval_like");
    let mut forwarding_caller = caller.clone();
    forwarding_caller.name = "forwarding_caller".into();
    forwarding_caller.loop_roles.clear();
    let block = forwarding_caller
        .blocks
        .get_mut(&forwarding_caller.entry_block)
        .unwrap();
    block.ops.truncate(1);
    block.terminator = Terminator::Return {
        values: block.ops[0].results.clone(),
    };
    // An intermediate block and its copy hide the producer from a direct
    // operand census, but leave one real arithmetic demand.
    let entry_id = caller.entry_block;
    let continuation = caller.fresh_block();
    let merged = caller.fresh_value();
    let copy = caller.fresh_value();
    caller.value_types.insert(merged, TirType::F64);
    caller.value_types.insert(copy, TirType::F64);
    let entry = caller.blocks.get_mut(&entry_id).unwrap();
    let actual = entry.ops[0].results[0];
    let mut tail = entry.ops.split_off(1);
    tail.last_mut().unwrap().operands[0] = copy;
    tail.insert(0, TirOp::new(OpCode::Copy, vec![merged], vec![copy]));
    let terminator = std::mem::replace(
        &mut entry.terminator,
        Terminator::Branch {
            target: continuation,
            args: vec![actual],
        },
    );
    caller.blocks.insert(
        continuation,
        TirBlock {
            id: continuation,
            args: vec![crate::tir::values::TirValue {
                id: merged,
                ty: TirType::F64,
            }],
            ops: tail,
            terminator,
        },
    );
    let mut m = module(vec![caller, forwarding_caller, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();

    assert!(
        sm.get(id_of(&m, "eval_like")).unwrap().op_count > tti.inline_budget("eval_like"),
        "fixture must be over the base inline budget"
    );
    assert!(
        sm.get(id_of(&m, "eval_like")).unwrap().op_count <= tti.hot_inline_op_limit(),
        "fixture must stay inside the targeted numeric hot-inline cap"
    );

    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::default());
    assert_eq!(
        stats.sites_inlined, 1,
        "only the call that feeds arithmetic should spend the numeric inline budget"
    );
    let caller = m
        .functions
        .iter()
        .find(|f| f.name == "numeric_loop_caller")
        .unwrap();
    let calls = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "numeric raw-lane call boundary eliminated");
    let forwarding_caller = m
        .functions
        .iter()
        .find(|f| f.name == "forwarding_caller")
        .unwrap();
    assert_eq!(
        forwarding_caller.blocks[&forwarding_caller.entry_block].ops[0].opcode,
        OpCode::Call,
        "forwarding the same callee's result should retain its call boundary"
    );
    crate::tir::verify::verify_function(caller)
        .unwrap_or_else(|e| panic!("numeric_loop_caller invalid: {e:?}"));
}

#[test]
fn split_field_budget_is_local_to_the_consuming_call() {
    let mut callee = over_base_budget_float_callee("split_consumer");
    let parameter = callee.fresh_value();
    callee.param_types.push(TirType::Str);
    callee.value_types.insert(parameter, TirType::Str);
    callee
        .blocks
        .get_mut(&callee.entry_block)
        .unwrap()
        .args
        .push(TirValue {
            id: parameter,
            ty: TirType::Str,
        });
    let caller = |name: &str, split: bool| {
        let mut caller = TirFunction::new(
            name.into(),
            vec![TirType::Str, TirType::I64, TirType::I64],
            TirType::F64,
            molt_ir::FunctionReturnAbi::Value,
        );
        let field = caller.fresh_value();
        let result = caller.fresh_value();
        caller.value_types.insert(field, TirType::Str);
        caller.value_types.insert(result, TirType::F64);
        let block = caller.blocks.get_mut(&caller.entry_block).unwrap();
        if split {
            let mut op = TirOp::new(
                OpCode::Copy,
                vec![ValueId(0), ValueId(1), ValueId(2)],
                vec![field],
            );
            op.origin = Some(molt_ir::OpKind::StringSplitField);
            block.ops.push(op);
        }
        let argument = if split { field } else { ValueId(0) };
        let mut call = TirOp::new(OpCode::Call, vec![argument], vec![result]);
        call.attrs
            .insert("s_value".into(), AttrValue::Str("split_consumer".into()));
        block.ops.push(call);
        block.terminator = Terminator::Return {
            values: vec![result],
        };
        caller
    };
    let mut module = module(vec![caller("field", true), caller("string", false), callee]);
    let (graph, summaries) = analysis(&module);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut module, &graph, &summaries, &tti, &HashSet::default());
    assert_eq!(stats.changed_functions, vec![module.ids[0]]);
    crate::tir::verify::verify_function(&module.functions[0]).unwrap();
}

#[test]
fn run_inliner_uses_hot_budget_for_numeric_return_with_observation_exit() {
    let callee = over_base_budget_float_callee_with_return_type(
        "eval_like",
        TirType::Union(vec![TirType::F64, TirType::None]),
    );
    let caller = loop_numeric_caller("eval_like");
    let mut m = module(vec![caller, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();

    assert!(
        sm.get(id_of(&m, "eval_like")).unwrap().op_count > tti.inline_budget("eval_like"),
        "fixture must be over the base inline budget"
    );
    assert!(
        sm.get(id_of(&m, "eval_like")).unwrap().op_count <= tti.hot_inline_op_limit(),
        "fixture must stay inside the targeted numeric hot-inline cap"
    );

    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::default());
    assert_eq!(
        stats.sites_inlined, 1,
        "numeric callee with a None observation-exit return should inline when its normal result feeds arithmetic"
    );
    let caller = m
        .functions
        .iter()
        .find(|f| f.name == "numeric_loop_caller")
        .unwrap();
    let calls = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "numeric raw-lane call boundary eliminated");
    crate::tir::verify::verify_function(caller)
        .unwrap_or_else(|e| panic!("numeric_loop_caller invalid: {e:?}"));
}

#[test]
fn run_inliner_two_sites_same_block_both_inlined() {
    // g() { x = constfn(); y = constfn(); return x + y } - two calls to the
    // same inlinable leaf in one block. The reverse-order driver must splice
    // BOTH (a refused/early site must not block the other). After inlining,
    // zero Call ops remain and SSA is valid.
    let callee = const_callee();
    let mut g = TirFunction::new(
        "g".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let x = g.fresh_value();
    let y = g.fresh_value();
    let sum = g.fresh_value();
    let entry = g.entry_block;
    let mk_call = |name: &str, out: ValueId| {
        let mut a = AttrDict::default();
        a.insert("s_value".into(), AttrValue::Str(name.to_string()));
        TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![out],
            attrs: a,
            source_span: None,
        }
    };
    let block = g.blocks.get_mut(&entry).unwrap();
    block.ops.push(mk_call("constfn", x));
    block.ops.push(mk_call("constfn", y));
    block.ops.push(TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![x, y],
        results: vec![sum],
        attrs: AttrDict::default(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![sum] };

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::default());
    assert_eq!(stats.sites_inlined, 2, "both call sites inlined");
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "both constfn calls eliminated");
    crate::tir::verify::verify_function(g)
        .unwrap_or_else(|e| panic!("g invalid after 2-site inlining: {e:?}"));
    // Both inlined `const 42`s folded into the sum.
    assert_eq!(const_ints(g), [84], "constfn() + constfn() folds to 84");
}

// -- (c) exception-observation inlining ----------------------------------

#[test]
fn run_inliner_inlines_observation_callee_end_to_end() {
    // End-to-end through run_inliner (clone + splice + per-function pipeline
    // re-run): a value-returning observation-only callee is inlined, the Call
    // is gone, and the merged caller is valid SSA with collision-free labels.
    let callee = observation_callee("obs", 3);
    let caller = caller_calling_obs_with_label("c", "obs", 3);
    let mut m = module(vec![caller, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::default());
    assert_eq!(stats.sites_inlined, 1, "obs inlined into c");
    let c = m.functions.iter().find(|f| f.name == "c").unwrap();
    let calls: usize = c
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "obs call eliminated from c");
    crate::tir::verify::verify_function(c)
        .unwrap_or_else(|e| panic!("c invalid after observation inlining: {e:?}"));
    // Labels remain collision-free after the pipeline re-run.
    let labels: Vec<i64> = c.label_id_map.values().copied().collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), labels.len(), "labels distinct: {labels:?}");
}

#[test]
fn run_inliner_preserves_tiny_codegen_partitions_on_every_target() {
    for tti in [
        TargetInfo::native_release_fast(),
        TargetInfo::llvm_release_fast(),
    ] {
        for partitioned in [false, true] {
            let mut callee = const_callee();
            callee.attrs.insert(
                crate::tir::function::CODEGEN_PARTITION_ATTR.into(),
                AttrValue::Bool(partitioned),
            );
            let caller = caller_calling_const(&callee.name);
            let mut m = module(vec![caller, callee]);
            let (cg, summaries) = analysis(&m);
            let stats = run_inliner(&mut m, &cg, &summaries, &tti, &HashSet::default());
            assert_eq!(stats.sites_inlined, usize::from(!partitioned));
            let calls = m.functions[0]
                .blocks
                .values()
                .flat_map(|block| &block.ops)
                .filter(|op| op.opcode == OpCode::Call)
                .count();
            assert_eq!(calls, usize::from(partitioned));
            crate::tir::verify::verify_function(&m.functions[0]).unwrap();
        }
    }
}

/// `g(flag) { while flag: constfn() }` with the call's post-call check marked
/// as its call-return poll. With `shared_latch_check` the call's check is
/// also the loop latch's poll (the body block is the latch); otherwise a
/// separate latch block polls the back edge.
fn looped_caller_with_polls(shared_latch_check: bool) -> TirFunction {
    let mut g = TirFunction::new(
        "g".into(),
        vec![TirType::Bool],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    g.has_exception_handling = true;
    let flag = g.blocks[&g.entry_block].args[0].id;
    let call_res = g.fresh_value();
    let header = g.fresh_block();
    let body = g.fresh_block();
    let latch = g.fresh_block();
    let exit = g.fresh_block();
    let handler = g.fresh_block();
    let label = 99;
    let poll = || {
        let mut attrs = AttrDict::default();
        attrs.insert("value".into(), AttrValue::Int(label));
        let mut check = TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        };
        assert!(check.mark_async_work_poll());
        check
    };
    let mut call_attrs = AttrDict::default();
    call_attrs.insert("s_value".into(), AttrValue::Str("constfn".to_string()));
    let call = TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![call_res],
        attrs: call_attrs,
        source_span: None,
    };
    let block = |id, ops, terminator| TirBlock {
        id,
        args: vec![],
        ops,
        terminator,
    };
    g.blocks.get_mut(&g.entry_block).unwrap().terminator = Terminator::Branch {
        target: header,
        args: vec![],
    };
    g.blocks.insert(
        header,
        block(
            header,
            vec![],
            Terminator::CondBranch {
                cond: flag,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        ),
    );
    let body_target = if shared_latch_check { header } else { latch };
    g.blocks.insert(
        body,
        block(
            body,
            vec![call, poll()],
            Terminator::Branch {
                target: body_target,
                args: vec![],
            },
        ),
    );
    if !shared_latch_check {
        g.blocks.insert(
            latch,
            block(
                latch,
                vec![poll()],
                Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            ),
        );
    }
    g.blocks.insert(
        exit,
        block(exit, vec![], Terminator::Return { values: vec![] }),
    );
    g.blocks.insert(
        handler,
        block(handler, vec![], Terminator::Return { values: vec![] }),
    );
    g.label_id_map.insert(handler.0, label);
    g.loop_roles.insert(header, LoopRole::LoopHeader);
    g.value_types.insert(call_res, TirType::I64);
    g
}

fn async_work_polls(f: &TirFunction) -> usize {
    f.blocks
        .values()
        .flat_map(|block| block.ops.iter())
        .filter(|op| op.is_async_work_poll())
        .count()
}

#[test]
fn run_inliner_retires_an_inlined_calls_poll_and_keeps_the_back_edge_poll() {
    // J4 (call_anatomy.md 4.8): the call-return poll belongs to a call. The
    // inlined body is no call, so only the back edge keeps a poll. A poll
    // left behind runs every iteration and pins every frame home store.
    for (shared_latch_check, polls_before) in [(false, 2), (true, 1)] {
        let caller = looped_caller_with_polls(shared_latch_check);
        assert_eq!(async_work_polls(&caller), polls_before);
        let mut m = module(vec![caller, const_callee()]);
        let (cg, sm) = analysis(&m);
        let tti = TargetInfo::native_release_fast();
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::default());
        assert_eq!(stats.sites_inlined, 1, "shared={shared_latch_check}");
        let g = m.functions.iter().find(|f| f.name == "g").unwrap();
        crate::tir::verify::verify_function(g)
            .unwrap_or_else(|e| panic!("g invalid after inlining: {e:?}"));
        // Shared: the check is the latch's poll too, and stays one.
        assert_eq!(async_work_polls(g), 1, "shared={shared_latch_check}");
    }
}

/// The value of every `ConstInt` in `f`, sorted.
fn const_ints(f: &TirFunction) -> Vec<i64> {
    let mut values: Vec<i64> = f
        .blocks
        .values()
        .flat_map(|block| block.ops.iter())
        .filter(|op| op.opcode == OpCode::ConstInt)
        .filter_map(|op| match op.attrs.get("value") {
            Some(AttrValue::Int(value)) => Some(*value),
            _ => None,
        })
        .collect();
    values.sort_unstable();
    values
}

#[test]
fn capture_budget_removes_call_and_local_allocation_only_for_noncapturing_formal() {
    // The identity chain is live at the exit; each link compares a new result
    // to a dynamic input. The callee exceeds the ordinary budget.
    fn callee(name: &str, capture: bool) -> TirFunction {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; 2],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let mut compared = ValueId(1);
        for _ in 0..35 {
            let result = func.fresh_value();
            let op = TirOp::new(OpCode::Is, vec![compared, ValueId(1)], vec![result]);
            func.blocks.get_mut(&func.entry_block).unwrap().ops.push(op);
            compared = result;
        }
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Return {
            values: vec![if capture { ValueId(0) } else { compared }],
        };
        func
    }
    fn caller(name: &str, callee: &str, finalizer: bool) -> TirFunction {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let root = func.fresh_value();
        let result = func.fresh_value();
        let mut allocation = TirOp::new(OpCode::Alloc, vec![], vec![root]);
        allocation.attrs.insert("value".into(), AttrValue::Int(8));
        if finalizer {
            allocation
                .attrs
                .insert("defines_del".into(), AttrValue::Bool(true));
        }
        let mut call = TirOp::new(OpCode::Call, vec![root, ValueId(0)], vec![result]);
        call.attrs
            .insert("s_value".into(), AttrValue::Str(callee.into()));
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.extend([
            allocation,
            call,
            TirOp::new(OpCode::DecRef, vec![root], vec![]),
        ]);
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        func
    }
    for tti in [
        TargetInfo::native_release_fast(),
        TargetInfo::llvm_release_fast(),
    ] {
        let mut m = module(vec![
            caller("local", "reads", false),
            caller("returned", "retains", false),
            caller("finalizer", "reads", true),
            callee("reads", false),
            callee("retains", true),
        ]);
        let (cg, sm) = analysis(&m);
        for name in ["reads", "retains"] {
            let count = sm.get(id_of(&m, name)).unwrap().op_count;
            assert!(tti.inline_budget(name) < count && count <= tti.hot_inline_op_limit());
        }
        let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::default());
        assert_eq!(stats.sites_inlined, 1);
        for (body, preserved) in m.functions.iter().take(3).zip([false, true, true]) {
            let has = |opcode| {
                body.blocks
                    .values()
                    .flat_map(|block| &block.ops)
                    .any(|op| op.opcode == opcode)
            };
            assert_eq!(has(OpCode::Call), preserved, "{}", body.name);
            assert_eq!(has(OpCode::Alloc), preserved, "{}", body.name);
            crate::tir::verify::verify_function(body)
                .unwrap_or_else(|e| panic!("{}: {e:?}", body.name));
        }
    }
}
