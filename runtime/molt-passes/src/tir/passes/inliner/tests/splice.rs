use super::*;

#[test]
fn splice_removes_call_and_passes_verify() {
    let callee = const_callee();
    let caller = caller_calling_const("constfn");
    #[allow(unused_mut)]
    let (mut caller, site) = sites_in(caller, &["constfn"]);
    assert_eq!(site.len(), 1);
    let did =
        splice_call_site(&mut caller, &callee, &site[0], &mut Activations::default()).is_some();
    assert!(did, "splice succeeded");
    // No Call op remains anywhere.
    let remaining_calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(remaining_calls, 0, "the Call was eliminated");
    // The merged function is valid SSA.
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("merged fn invalid SSA: {e:?}"));
}

#[test]
fn splice_moves_loop_condition_with_the_conditional_terminator() {
    let callee = const_callee();
    let mut caller = caller_calling_const("constfn");
    let header = caller.entry_block;
    let body = caller.fresh_block();
    let leave = caller.fresh_block();
    let condition = crate::fixture_support::append_parameter(&mut caller, TirType::Bool);
    let call_result = caller.blocks[&header].ops[0].results[0];
    caller.blocks.get_mut(&header).unwrap().terminator = Terminator::CondBranch {
        cond: condition,
        then_block: body,
        then_args: vec![],
        else_block: leave,
        else_args: vec![],
    };
    caller.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![condition],
            },
        },
    );
    caller.blocks.insert(
        leave,
        TirBlock {
            id: leave,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![call_result],
            },
        },
    );
    caller.loop_roles.insert(header, LoopRole::LoopHeader);
    caller.loop_cond_blocks.insert(header, header);

    let (mut caller, sites) = sites_in(caller, &["constfn"]);
    let continuation =
        splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default())
            .expect("the conditional block's call must splice");
    assert_eq!(caller.loop_cond_blocks[&header], continuation);
    crate::tir::verify::verify_function(&caller).expect("valid SSA after the call split");
    let ops = crate::tir::lower_to_simple::lower_to_simple_ir(&caller);
    assert!(crate::tir::lower_to_simple::validate_labels(&ops));
    let exits: Vec<_> = ops
        .iter()
        .filter(|op| {
            matches!(
                op.kind,
                molt_ir::OpKind::LoopBreakIfTrue | molt_ir::OpKind::LoopBreakIfFalse
            )
        })
        .collect();
    assert_eq!(
        exits.len(),
        1,
        "the backend must receive the caller's structured break"
    );
    assert_eq!(exits[0].kind, molt_ir::OpKind::LoopBreakIfFalse);
    assert_eq!(
        exits[0].args.as_deref(),
        Some(caller.param_names.as_slice())
    );
}

#[test]
fn splice_void_return() {
    // Callee returns nothing; caller calls it for effect.
    let mut callee = TirFunction::new(
        "eff".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let entry = callee.entry_block;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

    let mut caller = TirFunction::new(
        "g".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let mut call_attrs = AttrDict::default();
    call_attrs.insert("s_value".into(), AttrValue::Str("eff".into()));
    let centry = caller.entry_block;
    let block = caller.blocks.get_mut(&centry).unwrap();
    block.ops.push(TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };

    #[allow(unused_mut)]
    let (mut caller, sites) = sites_in(caller, &["eff"]);
    assert_eq!(sites.len(), 1);
    let spliced =
        splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default()).is_some();
    assert!(spliced);
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("void-splice invalid: {e:?}"));
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0);
}

#[test]
fn refcount_guard_refuses_arg_incref() {
    // Caller: IncRef(arg); call f(arg). The guard must refuse the splice.
    let mut callee = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let centry = callee.entry_block;
    callee.blocks.get_mut(&centry).unwrap().terminator = Terminator::Return { values: vec![] };

    let mut caller = TirFunction::new(
        "g".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let arg = ValueId(0); // the caller's param
    let entry = caller.entry_block;
    let mut call_attrs = AttrDict::default();
    call_attrs.insert("s_value".into(), AttrValue::Str("f".into()));
    let block = caller.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::IncRef,
        operands: vec![arg],
        results: vec![],
        attrs: AttrDict::default(),
        source_span: None,
    });
    block.ops.push(TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![arg],
        results: vec![],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };

    #[allow(unused_mut)]
    let (mut caller, sites) = sites_in(caller, &["f"]);
    assert_eq!(sites.len(), 1);
    let spliced =
        splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default()).is_some();
    assert!(
        !spliced,
        "refcount guard must refuse a site with arg IncRef before the call"
    );
    // The call survives intact.
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 1, "refused site keeps its call");
}

// -- is_inlineable gates -------------------------------------------------

/// The set of every label value in `func`'s `label_id_map` plus every
/// exception-op `"value"` label, used to assert collision-freedom.
fn all_labels(func: &TirFunction) -> Vec<i64> {
    function_label_ids(func).into_iter().collect()
}

#[test]
fn splice_observation_callee_remaps_labels_collision_free() {
    // Callee exception label 3; caller ALSO uses label 3 (collision). After
    // splicing, the cloned exit block must carry a FRESH label (not 3), the
    // caller's original label 3 must survive, and no two blocks may share a
    // label value (which would make `exception_label_to_block` ambiguous and
    // emit duplicate `label N` ops in lower_to_simple - a miscompile).
    let callee = observation_callee("obs", 3);
    let caller = caller_calling_obs_with_label("c", "obs", 3);

    #[allow(unused_mut)]
    let (mut caller, sites) = sites_in(caller, &["obs"]);
    assert_eq!(sites.len(), 1);
    let spliced =
        splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default()).is_some();
    assert!(spliced, "spliced");

    // No two blocks share a label value.
    let labels: Vec<i64> = caller.label_id_map.values().copied().collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        labels.len(),
        "every block label is distinct (no collision): {labels:?}"
    );
    // The caller's original label 3 survived.
    assert!(all_labels(&caller).contains(&3), "caller label 3 preserved");

    // Every cloned CheckException's handler label resolves to a block that
    // carries that exact label in label_id_map (the exception edge resolves).
    let label_to_block: molt_ir::deterministic_hash::HashMap<i64, BlockId> = caller
        .label_id_map
        .iter()
        .map(|(b, l)| (*l, BlockId(*b)))
        .collect();
    for block in caller.blocks.values() {
        for op in &block.ops {
            if let Some(label) = exception_label_of(op) {
                assert!(
                    label_to_block.contains_key(&label),
                    "CheckException label {label} resolves to a block"
                );
            }
        }
    }
    // The merged function is valid SSA.
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("merged fn invalid SSA: {e:?}"));
    // The Call is gone.
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "obs call eliminated");
}

#[test]
fn splice_unwind_preserves_the_callers_exception_observer() {
    // The callee unwinds without a result. The splice must keep the
    // caller's observer on that path and leave the value continuation alone.
    for (idx, ty) in [TirType::I64, TirType::Bool, TirType::F64, TirType::DynBox]
        .into_iter()
        .enumerate()
    {
        let callee_name = format!("obs_{idx}");
        let caller_name = format!("c_{idx}");
        let callee = observation_callee_with_type(&callee_name, 30 + idx as i64, ty.clone());
        let caller =
            caller_calling_obs_with_label_and_type(&caller_name, &callee_name, 90, ty.clone());

        #[allow(unused_mut)]
        let (mut caller, sites) = sites_in(caller, &[callee_name.as_str()]);
        assert_eq!(sites.len(), 1);
        let spliced =
            splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default())
                .is_some();
        assert!(
            spliced,
            "value-returning observation callee inlines (not refused) for {ty:?}"
        );
        crate::tir::verify::verify_function(&caller).unwrap_or_else(|e| {
            panic!("merged fn invalid SSA after direct exception branch for {ty:?}: {e:?}")
        });

        assert!(
            caller.blocks.values().any(|block| {
                matches!(block.terminator, Terminator::Unwind)
                    && block.ops.iter().any(|op| {
                        op.opcode == OpCode::CheckException
                            && op.attrs.get("value") == Some(&AttrValue::Int(90))
                            && op.exception_edge_kind()
                                == Some(crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin)
                    })
            }),
            "inlined unwind retains the caller's observer for {ty:?}"
        );
    }
}

#[test]
fn splice_unwind_preserves_physical_result_reads_outside_the_success_continuation() {
    #[derive(Clone, Copy, Debug)]
    enum Use {
        ObserverPayload,
        HandlerOperand,
        HandlerTerminator,
        MergedCleanup,
    }

    let callee = observation_callee_with_type("obs", 7, TirType::DynBox);
    for use_kind in [
        Use::ObserverPayload,
        Use::HandlerOperand,
        Use::HandlerTerminator,
        Use::MergedCleanup,
    ] {
        let mut caller = caller_calling_obs_with_label_and_type("c", "obs", 90, TirType::DynBox);
        let entry = caller.entry_block;
        let result = caller.blocks[&entry].ops[1].results[0];
        let handler = BlockId(
            *caller
                .label_id_map
                .iter()
                .find(|(_, label)| **label == 90)
                .unwrap()
                .0,
        );
        match use_kind {
            Use::ObserverPayload => {
                let payload = caller.fresh_value();
                caller.value_types.insert(payload, TirType::DynBox);
                caller.blocks.get_mut(&handler).unwrap().args =
                    vec![crate::tir::values::TirValue {
                        id: payload,
                        ty: TirType::DynBox,
                    }];
                caller.blocks.get_mut(&entry).unwrap().ops[2].operands = vec![result];
            }
            Use::HandlerOperand => {
                caller
                    .blocks
                    .get_mut(&handler)
                    .unwrap()
                    .ops
                    .push(TirOp::new(OpCode::DelBoundary, vec![result], vec![]));
            }
            Use::HandlerTerminator => {
                caller.blocks.get_mut(&handler).unwrap().terminator = Terminator::Return {
                    values: vec![result],
                };
            }
            Use::MergedCleanup => {
                let cleanup = caller.fresh_block();
                for block in [entry, handler] {
                    caller.blocks.get_mut(&block).unwrap().terminator = Terminator::Branch {
                        target: cleanup,
                        args: vec![],
                    };
                }
                caller.blocks.insert(
                    cleanup,
                    TirBlock {
                        id: cleanup,
                        args: vec![],
                        ops: vec![TirOp::new(OpCode::DelBoundary, vec![result], vec![])],
                        terminator: Terminator::Unwind,
                    },
                );
            }
        }

        // A physical ABI result exists before its check even on failure.
        // Defining it only at the spliced normal continuation would make
        // these already-valid exceptional reads fail SSA dominance.
        let (mut caller, sites) = sites_in(caller, &["obs"]);
        assert_eq!(sites.len(), 1);
        crate::tir::verify::verify_function(&caller)
            .unwrap_or_else(|e| panic!("{use_kind:?}: original caller invalid: {e:?}"));
        let before = format!("{caller:?}");
        assert!(
            splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default())
                .is_none(),
            "{use_kind:?}: unwind must retain the call's physical result definition"
        );
        assert_eq!(
            format!("{caller:?}"),
            before,
            "{use_kind:?}: refusal is atomic"
        );
        crate::tir::verify::verify_function(&caller)
            .unwrap_or_else(|e| panic!("{use_kind:?}: refused caller invalid: {e:?}"));
    }
}

/// The spliced activation binds each parameter by its custody. A callee that
/// owns its parameter binds the caller's temporary through one owned
/// `binding_alias` where the call was; its return captures that binding first,
/// and each exit, the exception exit included, releases the binding before it
/// leaves. A borrowed parameter whose argument is the caller's own parameter
/// binds directly, and nothing in the activation releases it.
#[test]
fn splice_binds_parameters_by_custody_and_clears_them_at_every_exit() {
    use molt_ir::ParameterCustody;
    for (custody, pass_parameter) in [
        (ParameterCustody::Transferred, false),
        (ParameterCustody::Borrowed, true),
    ] {
        // obs(a) -> a, with a CheckException whose exception exit returns void.
        let mut callee = observation_callee_with_type("obs_custody", 7, TirType::DynBox);
        callee.set_parameter_custody(&[custody]);
        // c(p) { t = make(); r = obs(p or t); check; return r }
        let mut caller = TirFunction::new(
            "c_custody".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        caller.has_exception_handling = true;
        let parameter = ValueId(0);
        let temporary = caller.fresh_value();
        let result = caller.fresh_value();
        let handler = caller.fresh_block();
        let argument = if pass_parameter { parameter } else { temporary };
        let mut call_attrs = AttrDict::default();
        call_attrs.insert("s_value".into(), AttrValue::Str("obs_custody".into()));
        let mut call = TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![argument],
            results: vec![result],
            attrs: call_attrs,
            source_span: Some((3, 9)),
        };
        call.set_argument_custody(&[custody]);
        let mut check_attrs = AttrDict::default();
        check_attrs.insert("value".into(), AttrValue::Int(90));
        // The post-call observation of `obs`: the call's failure is Origin.
        check_attrs.insert(
            "exception_edge".into(),
            AttrValue::Int(crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin as i64),
        );
        let entry = caller.entry_block;
        let block = caller.blocks.get_mut(&entry).unwrap();
        block.ops = vec![
            TirOp {
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![temporary],
                attrs: AttrDict::from_iter([(
                    "s_value".into(),
                    AttrValue::Str("fixture_produce_owned".into()),
                )]),
                source_span: None,
            },
            call,
            TirOp {
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: check_attrs,
                source_span: None,
            },
        ];
        block.terminator = Terminator::Return {
            values: vec![result],
        };
        caller.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        caller.label_id_map.insert(handler.0, 90);
        caller.value_types.insert(temporary, TirType::DynBox);
        caller.value_types.insert(result, TirType::DynBox);

        #[allow(unused_mut)]
        let (mut caller, sites) = sites_in(caller, &["obs_custody"]);
        assert_eq!(sites.len(), 1);
        let spliced =
            splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default())
                .is_some();
        assert!(spliced, "{custody:?}");
        crate::tir::verify::verify_function(&caller)
            .unwrap_or_else(|e| panic!("{custody:?}: merged fn invalid SSA: {e:?}"));

        let ops: Vec<&TirOp> = caller.blocks.values().flat_map(|b| &b.ops).collect();
        let aliases_of = |source: ValueId| {
            ops.iter()
                .copied()
                .filter(|op| {
                    op.opcode == OpCode::Copy
                        && op.operands == [source]
                        && matches!(
                            op.origin,
                            Some(kind) if kind == molt_ir::OpKind::BindingAlias
                        )
                })
                .collect::<Vec<_>>()
        };
        let releases_of = |value: ValueId| {
            ops.iter()
                .filter(|op| op.opcode == OpCode::DelBoundary && op.operands == [value])
                .count()
        };
        let bindings = aliases_of(argument);
        if pass_parameter {
            assert!(bindings.is_empty(), "the caller's parameter binds directly");
            assert_eq!(
                releases_of(argument),
                0,
                "the activation releases no borrow"
            );
            continue;
        }
        assert_eq!(bindings.len(), 1, "one owned binding where the call was");
        assert_eq!(bindings[0].source_span, Some((3, 9)));
        let binding = bindings[0].results[0];
        assert_eq!(releases_of(binding), 2, "both exits clear the frame");
        let captures = aliases_of(binding);
        assert_eq!(captures.len(), 1, "the returned binding is captured");
        let capture = captures[0].results[0];
        let continuation_args: Vec<ValueId> = caller
            .blocks
            .values()
            .filter_map(|block| match &block.terminator {
                Terminator::Branch { args, .. } => Some(args.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            continuation_args,
            [capture],
            "the result carries the capture, never the binding"
        );
    }
}

/// Design 78 5.10/5.11: a closure callee inlines at a proven guarded call,
/// whose operand 0 is the callee's own function object. The splice reads that
/// object's closure tuple (`FunctionClosure`) and binds the environment
/// parameter to an owned alias of it, so the inlined body never depends on
/// the function object outliving its last use.
#[test]
fn a_closure_callee_inlines_at_a_proven_call_with_its_own_environment() {
    let mut callee = TirFunction::new(
        "clos".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    callee.param_names = vec![crate::MOLT_CLOSURE_PARAM_NAME.into(), "x".into()];
    let entry = callee.entry_block;
    let x = callee.blocks[&entry].args[1].id;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![x] };

    let mut caller = TirFunction::new(
        "g".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let centry = caller.entry_block;
    let (function, argument) = (
        caller.blocks[&centry].args[0].id,
        caller.blocks[&centry].args[1].id,
    );
    let result = caller.fresh_value();
    caller.value_types.insert(result, TirType::DynBox);
    let mut call = TirOp::new(OpCode::Call, vec![function, argument], vec![result]);
    call.origin = Some(molt_ir::OpKind::CallGuarded);
    call.callee = Some(molt_ir::FunctionId(1));
    call.set_proven_target(true);
    let block = caller.blocks.get_mut(&centry).unwrap();
    block.ops.push(call);
    block.terminator = Terminator::Return {
        values: vec![result],
    };

    let (mut caller, sites) = sites_in(caller, &["clos"]);
    assert_eq!(sites.len(), 1);
    assert!(
        splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default()).is_some()
    );
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("closure splice invalid: {e:?}"));
    let ops: Vec<&TirOp> = caller.blocks.values().flat_map(|b| &b.ops).collect();
    assert!(
        ops.iter().all(|op| op.opcode != OpCode::Call),
        "the call is gone"
    );
    let load = ops
        .iter()
        .find(|op| op.opcode == OpCode::FunctionClosure)
        .expect("the environment comes from the callee's function object");
    assert_eq!(
        load.operands,
        [function],
        "the caller's parameter keeps the function alive through the activation"
    );
    let view = load.results[0];
    assert!(
        ops.iter().any(|op| {
            op.operands == [view] && op.origin == Some(molt_ir::OpKind::BindingAlias)
        }),
        "the environment binds through an owned alias of the closure tuple"
    );
}

/// A direct internal call of a closure carries no environment operand, so
/// the splice refuses it rather than bind the environment to an argument.
#[test]
fn a_closure_callee_refuses_a_call_without_a_function_object() {
    let mut callee = TirFunction::new(
        "clos".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    callee.param_names = vec![crate::MOLT_CLOSURE_PARAM_NAME.into(), "x".into()];
    let entry = callee.entry_block;
    let x = callee.blocks[&entry].args[1].id;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![x] };
    let mut caller = TirFunction::new(
        "g".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let centry = caller.entry_block;
    let (a, b) = (
        caller.blocks[&centry].args[0].id,
        caller.blocks[&centry].args[1].id,
    );
    let result = caller.fresh_value();
    let mut call = TirOp::new(OpCode::Call, vec![a, b], vec![result]);
    call.callee = Some(molt_ir::FunctionId(1));
    let block = caller.blocks.get_mut(&centry).unwrap();
    block.ops.push(call);
    block.terminator = Terminator::Return {
        values: vec![result],
    };
    let (mut caller, sites) = sites_in(caller, &["clos"]);
    assert_eq!(sites.len(), 1);
    assert!(
        splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default()).is_none()
    );
}

#[test]
fn refused_closure_splices_preserve_the_complete_caller() {
    #[derive(Debug)]
    enum Refusal {
        Arity,
        ArgumentRetain,
        PreparedDrops,
        MissingReturnValue,
    }

    for refusal in [
        Refusal::Arity,
        Refusal::ArgumentRetain,
        Refusal::PreparedDrops,
        Refusal::MissingReturnValue,
    ] {
        let mut callee = TirFunction::new(
            "clos".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        callee.param_names = vec![crate::MOLT_CLOSURE_PARAM_NAME.into(), "x".into()];
        let entry = callee.entry_block;
        let x = callee.blocks[&entry].args[1].id;
        callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
            values: if matches!(refusal, Refusal::MissingReturnValue) {
                vec![]
            } else {
                vec![x]
            },
        };
        if matches!(refusal, Refusal::PreparedDrops) {
            callee.attrs.insert(
                crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR.into(),
                AttrValue::Bool(true),
            );
        }

        let mut caller = TirFunction::new(
            "caller".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let entry = caller.entry_block;
        let function = caller.blocks[&entry].args[0].id;
        let argument = caller.blocks[&entry].args[1].id;
        let result = caller.fresh_value();
        caller.value_types.insert(result, TirType::DynBox);
        let operands = if matches!(refusal, Refusal::Arity) {
            vec![function]
        } else {
            vec![function, argument]
        };
        let mut call = TirOp::new(OpCode::Call, operands, vec![result]);
        call.origin = Some(molt_ir::OpKind::CallGuarded);
        call.callee = Some(molt_ir::FunctionId(1));
        call.set_proven_target(true);
        let block = caller.blocks.get_mut(&entry).unwrap();
        if matches!(refusal, Refusal::ArgumentRetain) {
            block
                .ops
                .push(TirOp::new(OpCode::IncRef, vec![argument], vec![]));
        }
        block.ops.push(call);
        block.terminator = Terminator::Return {
            values: vec![result],
        };
        let (mut caller, sites) = sites_in(caller, &["clos"]);
        assert_eq!(sites.len(), 1);
        // The serialized function includes its SSA allocator, type facts,
        // attributes and CFG, rather than only the surviving call instruction.
        let before = rmp_serde::to_vec_named(&caller).unwrap();
        assert!(
            splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default())
                .is_none(),
            "the {refusal:?} control must exercise a refusal"
        );
        assert_eq!(
            rmp_serde::to_vec_named(&caller).unwrap(),
            before,
            "{refusal:?} must preserve every field of the caller"
        );
    }
}

#[test]
fn closure_argument_custody_skips_the_environment_parameter() {
    let mut callee = TirFunction::new(
        "clos".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    callee.param_names = vec![crate::MOLT_CLOSURE_PARAM_NAME.into(), "x".into()];
    callee.set_parameter_custody(&[
        molt_ir::ParameterCustody::Borrowed,
        molt_ir::ParameterCustody::Transferred,
    ]);
    let entry = callee.entry_block;
    let argument = callee.blocks[&entry].args[1].id;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
        values: vec![argument],
    };

    let mut caller = TirFunction::new(
        "caller".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = caller.entry_block;
    let function = caller.blocks[&entry].args[0].id;
    let argument = caller.blocks[&entry].args[1].id;
    let result = caller.fresh_value();
    caller.value_types.insert(result, TirType::DynBox);
    let mut call = TirOp::new(OpCode::Call, vec![function, argument], vec![result]);
    call.origin = Some(molt_ir::OpKind::CallGuarded);
    call.callee = Some(molt_ir::FunctionId(1));
    call.set_proven_target(true);
    let block = caller.blocks.get_mut(&entry).unwrap();
    block.ops.push(call);
    block.terminator = Terminator::Return {
        values: vec![result],
    };
    let (mut caller, sites) = sites_in(caller, &["clos"]);
    assert_eq!(sites.len(), 1);
    assert!(
        splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default()).is_some()
    );
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("closure custody splice invalid: {e:?}"));
    // The caller parameter outlives the call, but the callee's second
    // physical parameter adopts a reference. Reading custody from the
    // borrowed environment parameter would incorrectly bind it directly.
    let binding = caller.blocks[&entry]
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Copy && op.operands == [argument])
        .expect("the transferred argument has its own binding");
    let owned = binding.results[0];
    assert!(
        caller
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|op| { op.opcode == OpCode::DelBoundary && op.operands.contains(&owned) }),
        "the activation must clear its adopted argument reference"
    );
}

#[test]
fn an_inlined_closure_keeps_its_temporary_callable_until_each_activation_exit() {
    // The closure returns its environment on the normal path, and propagates
    // a pending exception on the other. Returning the environment exercises
    // result capture before the activation's owners are cleared.
    let mut callee = observation_callee_with_type("clos", 7, TirType::DynBox);
    callee.param_names = vec![crate::MOLT_CLOSURE_PARAM_NAME.into()];
    let mut caller = caller_calling_obs_with_label_and_type("caller", "clos", 90, TirType::DynBox);
    let entry = caller.entry_block;
    let block = caller.blocks.get_mut(&entry).unwrap();
    let function = block.ops[0].results[0];
    // This is an OWNED temporary function, not a borrowed parameter or a
    // static cell whose independent owner would mask early destruction.
    block.ops[0] = TirOp::new(OpCode::Call, vec![], vec![function]);
    block.ops[0].attrs.insert(
        "s_value".into(),
        AttrValue::Str("fixture_make_closure".into()),
    );
    block.ops[1].origin = Some(molt_ir::OpKind::CallGuarded);
    block.ops[1].set_proven_target(true);
    let (mut caller, sites) = sites_in(caller, &["clos"]);
    assert_eq!(sites.len(), 1);
    assert!(
        splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default()).is_some()
    );

    crate::tir::passes::drop_insertion::run(
        &mut caller,
        &mut crate::tir::analysis::AnalysisManager::new(),
    );
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("closure lifetime after terminal drops: {e:?}"));
    let prefix = &caller.blocks[&entry].ops;
    let load = prefix
        .iter()
        .position(|op| op.opcode == OpCode::FunctionClosure)
        .unwrap();
    let function_owner = prefix[load].operands[0];
    assert!(
        prefix[load..]
            .iter()
            .all(|op| { op.opcode != OpCode::DecRef || !op.operands.contains(&function_owner) }),
        "the function must outlive environment acquisition and the inlined body"
    );
    let view = prefix[load].results[0];
    let environment = prefix
        .iter()
        .find(|op| op.origin == Some(molt_ir::OpKind::BindingAlias) && op.operands == [view])
        .unwrap()
        .results[0];
    let mut exits = 0;
    for block in caller.blocks.values() {
        let releases: Vec<_> = block
            .ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef && op.operands.contains(&function_owner))
            .collect();
        if releases.is_empty() {
            continue;
        }
        exits += 1;
        assert_eq!(
            releases.len(),
            1,
            "one function reference per activation exit"
        );
        let release = releases[0].0;
        assert!(
            block.ops[..release]
                .iter()
                .any(|op| { op.opcode == OpCode::DecRef && op.operands.contains(&environment) }),
            "callee cleanup must finish before its function can be destroyed"
        );
    }
    assert_eq!(
        exits, 2,
        "normal and implicit exceptional exits release the function"
    );
}

#[test]
fn a_nonclosure_splice_keeps_its_captured_callable_through_both_exits() {
    let callee = observation_callee_with_type("target", 7, TirType::DynBox);
    let mut caller =
        caller_calling_obs_with_label_and_type("caller", "target", 90, TirType::DynBox);
    let entry = caller.entry_block;
    let function = caller.fresh_value();
    caller.value_types.insert(function, TirType::DynBox);
    let block = caller.blocks.get_mut(&entry).unwrap();
    let mut producer = TirOp::new(OpCode::Call, vec![], vec![function]);
    producer.attrs.insert(
        "s_value".into(),
        AttrValue::Str("fixture_make_function".into()),
    );
    block.ops.insert(0, producer);
    let call = &mut block.ops[2];
    call.origin = Some(molt_ir::OpKind::CallGuarded);
    call.set_proven_target(true);
    call.operands.insert(0, function);
    let (mut caller, sites) = sites_in(caller, &["target"]);
    assert_eq!(sites.len(), 1);
    splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default()).unwrap();

    let owner = caller.blocks[&entry]
        .ops
        .iter()
        .find(|op| op.origin == Some(molt_ir::OpKind::BindingAlias) && op.operands == [function])
        .expect("the captured function has its own activation owner")
        .results[0];
    crate::tir::passes::drop_insertion::run(
        &mut caller,
        &mut crate::tir::analysis::AnalysisManager::new(),
    );
    crate::tir::verify::verify_function(&caller).unwrap();
    let releases: Vec<_> = caller
        .blocks
        .values()
        .filter(|block| {
            block
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::DecRef && op.operands == [owner])
        })
        .map(|block| block.id)
        .collect();
    assert_eq!(releases.len(), 2);
    assert!(!releases.contains(&entry));
}

#[test]
fn a_spliced_call_operand_cannot_witness_an_earlier_borrow() {
    use super::super::activation::Binding;

    let callee = observation_callee_with_type("target", 7, TirType::DynBox);
    let mut caller =
        caller_calling_obs_with_label_and_type("caller", "target", 90, TirType::DynBox);
    let entry = caller.entry_block;
    let later = caller.fresh_block();
    let later_result = caller.fresh_value();
    caller.value_types.insert(later_result, TirType::DynBox);
    let prefix = caller.blocks.get_mut(&entry).unwrap();
    let object = prefix.ops[0].results[0];
    let mut producer = TirOp::new(OpCode::Call, vec![], vec![object]);
    producer.attrs.insert(
        "s_value".into(),
        AttrValue::Str("fixture_make_object".into()),
    );
    prefix.ops[0] = producer;
    let mut later_call = prefix.ops[1].clone();
    later_call.results[0] = later_result;
    prefix.terminator = Terminator::Branch {
        target: later,
        args: vec![],
    };
    caller.blocks.insert(
        later,
        TirBlock {
            id: later,
            args: vec![],
            ops: vec![later_call],
            terminator: Terminator::Return {
                values: vec![later_result],
            },
        },
    );
    let (caller, sites) = sites_in(caller, &["target"]);
    assert_eq!(sites.len(), 2);
    let first = sites.iter().find(|site| site.block == entry).unwrap();

    let mut activations = Activations::default();
    activations.enter_caller(&sites);
    assert_eq!(
        activations.bindings(&caller, &callee, first, &[object], 0),
        vec![Binding::Owned],
        "the later splice may remove the only cross-block witness"
    );

    activations.enter_caller([first]);
    assert_eq!(
        activations.bindings(&caller, &callee, first, &[object], 0),
        vec![Binding::Direct],
        "a call that stays provides the cross-block witness"
    );
}

#[test]
fn fresh_sequence_moves_one_reference_to_the_existing_frame_clear() {
    use molt_ir::ParameterCustody;
    // Each test uses the same ordinary leaf and the same checked call. Change
    // only the independent evidence for the argument credit or its visibility.
    #[derive(Clone, Copy, Debug)]
    enum Argument {
        List,
        Tuple,
        UnknownElement,
        Published,
        Escaping,
        Transferred,
        Observer,
    }
    for shape in [
        Argument::List,
        Argument::Tuple,
        Argument::UnknownElement,
        Argument::Published,
        Argument::Escaping,
        Argument::Transferred,
        Argument::Observer,
    ] {
        let mut callee = TirFunction::new(
            "predicate".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let none = callee.fresh_value();
        let result = callee.fresh_value();
        let entry = callee.blocks.get_mut(&callee.entry_block).unwrap();
        entry
            .ops
            .push(TirOp::new(OpCode::ConstNone, vec![], vec![none]));
        entry
            .ops
            .push(TirOp::new(OpCode::Is, vec![ValueId(0), none], vec![result]));
        if matches!(shape, Argument::Observer) {
            entry.ops.push(TirOp::new(OpCode::Call, vec![], vec![]));
        }
        entry.terminator = Terminator::Return {
            values: vec![if matches!(shape, Argument::Escaping) {
                ValueId(0)
            } else {
                result
            }],
        };
        if matches!(shape, Argument::Transferred) {
            callee.set_parameter_custody(&[ParameterCustody::Transferred]);
        }
        let mut caller = TirFunction::new(
            "caller".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let item = caller.fresh_value();
        let object = caller.fresh_value();
        let output = caller.fresh_value();
        let entry = caller.blocks.get_mut(&caller.entry_block).unwrap();
        let mut literal = TirOp::new(OpCode::ConstInt, vec![], vec![item]);
        literal.attrs.insert("value".into(), AttrValue::Int(9));
        entry.ops.push(literal);
        let element = if matches!(shape, Argument::UnknownElement) {
            ValueId(0)
        } else {
            item
        };
        let kind = if matches!(shape, Argument::Tuple) {
            OpCode::BuildTuple
        } else {
            OpCode::BuildList
        };
        entry
            .ops
            .push(TirOp::new(kind, vec![element], vec![object]));
        if matches!(shape, Argument::Published) {
            entry.ops.push(TirOp::new(
                OpCode::StoreIndex,
                vec![ValueId(0), item, object],
                vec![],
            ));
        }
        let mut call = TirOp::new(OpCode::Call, vec![object], vec![output]);
        call.attrs
            .insert("s_value".into(), AttrValue::Str("predicate".into()));
        if matches!(shape, Argument::Transferred) {
            call.set_argument_custody(&[ParameterCustody::Transferred]);
        }
        entry.ops.push(call);
        entry.terminator = Terminator::Return {
            values: vec![output],
        };
        let (mut caller, sites) = sites_in(caller, &["predicate"]);
        assert!(
            splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default())
                .is_some()
        );
        let ops = caller.blocks.values().flat_map(|block| &block.ops);
        let retained: Vec<_> = ops
            .clone()
            .filter(|op| {
                op.origin == Some(molt_ir::OpKind::BindingAlias) && op.operands == [object]
            })
            .collect();
        let moved = matches!(shape, Argument::List | Argument::Tuple);
        assert_eq!(retained.len(), usize::from(!moved), "{shape:?}");
        let target = retained.first().map_or(object, |op| op.results[0]);
        assert_eq!(
            ops.filter(|op| op.opcode == OpCode::DelBoundary && op.operands == [target])
                .count(),
            1,
            "{shape:?}: one original frame clear"
        );

        if moved {
            for tti in [
                TargetInfo::native_release_fast(),
                TargetInfo::llvm_release_fast(),
            ] {
                let mut finalized = caller.clone();
                crate::tir::pass_manager::build_drop_pipeline(tti).run(&mut finalized);
                crate::tir::verify::verify_function(&finalized).unwrap();
                let ops = finalized.blocks.values().flat_map(|block| &block.ops);
                assert!(
                    ops.clone()
                        .any(|op| op.opcode == kind && op.results == [object])
                );
                assert_eq!(
                    ops.clone()
                        .filter(|op| op.opcode == OpCode::IncRef && op.operands == [object])
                        .count(),
                    0
                );
                assert_eq!(
                    ops.filter(|op| op.opcode == OpCode::DecRef && op.operands == [object])
                        .count(),
                    1
                );
            }
        }
    }
}
