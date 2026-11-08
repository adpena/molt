use super::super::seal_block_once;
use super::*;
use cranelift_frontend::Variable;
use molt_ir::OpKind;

fn native_ssa_facts(
    ops: &[OpIR],
    parameters: &[&str],
    stateful: bool,
) -> (NativeSsaValues, crate::tir::cfg_liveness::SimpleNameTable) {
    let function = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "direct_ssa_control".into(),
        params: parameters.iter().map(|name| (*name).into()).collect(),
        ops: ops.to_vec(),
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    let cfg = crate::tir::cfg::CFG::build(ops);
    let names =
        crate::tir::cfg_liveness::analyze_simple_cfg_liveness_facts_with_cfg(ops, &cfg).names;
    let values = NativeSsaValues::for_function(&function, &cfg, stateful, &names);
    (values, names)
}

fn ssa_constant(name: &str, value: i64) -> OpIR {
    OpIR {
        out: Some(name.into()),
        value: Some(value),
        ..OpIR::new(OpKind::ConstInt)
    }
}

fn ssa_label(kind: OpKind, value: i64) -> OpIR {
    OpIR {
        value: Some(value),
        ..OpIR::new(kind)
    }
}

#[test]
fn direct_label_transport_uses_captured_or_backing_value() {
    for capture in [false, true] {
        let ops = vec![
            ssa_constant("immutable", 37),
            OpIR {
                args: Some(vec!["immutable".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ];
        let (mut values, names) = native_ssa_facts(&ops, &[], false);
        let mut sig = Signature::new(CallConv::SystemV);
        sig.returns.push(AbiParam::new(types::I64));
        let mut function = Function::with_name_signature(UserFuncName::default(), sig);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            let entry = builder.create_block();
            let target = builder.create_block();
            builder.switch_to_block(entry);
            let var = builder.declare_var(types::I64);
            let expected = builder.ins().iconst(types::I64, 37);
            builder.def_var(var, expected);
            values.project_variables(
                &names,
                &BTreeMap::from([("immutable".into(), var)]),
                &ScalarRepresentationPlan::default(),
                &BTreeMap::new(),
            );
            let id = names.id("immutable").unwrap();
            let plan = BlockTransportPlan::from_live_ids(
                [id].into_iter(),
                &values,
                crate::tir::dominators::SimpleProgramPoint::Before(1),
            );
            assert!(plan.edge_args(&mut builder).is_empty());
            plan.append_block_params(&mut builder, target);
            assert!(builder.block_params(target).is_empty());
            builder.install_ssa_values(values);
            if capture {
                builder.capture_operation(0, &ops[0], &names);
            }
            builder.begin_op(1);
            builder.ins().jump(target, &[]);
            builder.switch_to_block(target);
            let before = builder.func.dfg.num_values();
            plan.bind_block_params(&mut builder, target);
            builder.end_op();
            if capture {
                assert_eq!(builder.func.dfg.num_values(), before);
                assert_eq!(builder.use_var(var), expected);
            } else {
                assert!(builder.canonical_ssa_value(id).is_none());
            }
            let actual = builder.use_var(var);
            builder.ins().return_(&[actual]);
            builder.seal_all_blocks();
            assert_eq!(builder.func.dfg.resolve_aliases(actual), expected);
            builder.finalize(crate::test_support::host_frontend_config());
        }
        verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
    }
}

#[test]
fn native_ssa_reads_preserve_sparse_reverse_variable_projection() {
    let ops = vec![
        ssa_constant("a", 37),
        ssa_constant("z", 41),
        OpIR {
            out: Some("sum".into()),
            args: Some(vec!["a".into(), "z".into()]),
            ..OpIR::new(OpKind::Add)
        },
        OpIR {
            args: Some(vec!["sum".into()]),
            ..OpIR::new(OpKind::Ret)
        },
    ];
    let (mut values, names) = native_ssa_facts(&ops, &[], false);
    let mut function =
        Function::with_name_signature(UserFuncName::default(), Signature::new(CallConv::SystemV));
    let mut context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut function, &mut context);
    let entry = builder.create_block();
    builder.switch_to_block(entry);
    // Name order visits the highest Variable first. A shrinking projection
    // loses a's binding when it later visits z.
    let z = builder.declare_var(types::I64);
    let internal = builder.declare_var(types::I64);
    let a = builder.declare_var(types::I64);
    values.project_variables(
        &names,
        &BTreeMap::from([("a".into(), a), ("z".into(), z)]),
        &ScalarRepresentationPlan::default(),
        &BTreeMap::new(),
    );
    assert_eq!(values.read(a, 2), None, "uncaptured definition");
    let a_value = builder.ins().iconst(types::I64, 37);
    let z_value = builder.ins().iconst(types::I64, 41);
    builder.def_var(a, a_value);
    builder.def_var(z, z_value);
    values.capture_operation(&mut builder, 0, &ops[0], &names);
    values.capture_operation(&mut builder, 1, &ops[1], &names);
    assert_eq!(values.read(a, 0), None, "before its definition");
    assert_eq!(values.read(z, 1), None, "definition is not yet available");
    assert_eq!(values.read(a, 2), Some(a_value));
    assert_eq!(values.read(z, 2), Some(z_value));
    assert_eq!(values.read(internal, 2), None, "internal Variable");
    assert_eq!(values.read(Variable::from_u32(127), 2), None);
    builder.ins().return_(&[]);
    builder.seal_all_blocks();
    builder.finalize(crate::test_support::host_frontend_config());
}

#[test]
fn live_id_transport_only_parameterizes_mutable_projected_carriers() {
    let ops = vec![
        ssa_constant("immutable", 37),
        OpIR {
            var: Some("mutable".into()),
            args: Some(vec!["immutable".into()]),
            ..OpIR::new(OpKind::StoreVar)
        },
        OpIR {
            out: Some("sum".into()),
            args: Some(vec!["immutable".into(), "mutable".into()]),
            ..OpIR::new(OpKind::Add)
        },
        OpIR {
            out: Some("result".into()),
            args: Some(vec!["sum".into(), "unprojected".into()]),
            ..OpIR::new(OpKind::Add)
        },
        OpIR {
            args: Some(vec!["result".into()]),
            ..OpIR::new(OpKind::Ret)
        },
    ];
    let (mut values, names) = native_ssa_facts(&ops, &["mutable", "unprojected"], false);
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns = vec![AbiParam::new(types::I64), AbiParam::new(types::I64)];
    let mut function = Function::with_name_signature(UserFuncName::default(), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let target = builder.create_block();
        builder.switch_to_block(entry);
        let immutable = builder.declare_var(types::I64);
        let mutable = builder.declare_var(types::I64);
        let expected = builder.ins().iconst(types::I64, 37);
        let carried = builder.ins().iconst(types::I64, 11);
        builder.def_var(immutable, expected);
        builder.def_var(mutable, carried);
        values.project_variables(
            &names,
            &BTreeMap::from([("immutable".into(), immutable), ("mutable".into(), mutable)]),
            &ScalarRepresentationPlan::default(),
            &BTreeMap::new(),
        );
        let plan = BlockTransportPlan::from_live_ids(
            ["immutable", "mutable", "unprojected"]
                .into_iter()
                .map(|name| names.id(name).unwrap()),
            &values,
            crate::tir::dominators::SimpleProgramPoint::Before(2),
        );
        builder.install_ssa_values(values);
        builder.capture_operation(0, &ops[0], &names);
        builder.begin_op(2);
        plan.append_block_params(&mut builder, target);
        assert_eq!(builder.block_params(target).len(), 1);
        let param = builder.block_params(target)[0];
        assert_eq!(builder.func.dfg.value_type(param), types::I64);
        let args = plan.edge_args(&mut builder);
        assert_eq!(args, vec![carried]);
        jump_block(&mut builder, target, &args);
        builder.switch_to_block(target);
        plan.bind_block_params(&mut builder, target);
        assert_eq!(builder.use_var(immutable), expected);
        assert_eq!(builder.use_var(mutable), param);
        builder.ins().return_(&[expected, param]);
        builder.seal_all_blocks();
        builder.finalize(crate::test_support::host_frontend_config());
    }
    verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
}

#[test]
fn native_ssa_handler_reads_observe_the_earliest_exception_transfer() {
    for early_exception in [false, true] {
        let mut ops = vec![ssa_constant("before", 7), ssa_label(OpKind::TryStart, 90)];
        if early_exception {
            ops.push(ssa_label(OpKind::CheckException, 90));
        }
        let inside = ops.len();
        ops.push(ssa_constant("inside", 37));
        ops.push(ssa_label(OpKind::CheckException, 90));
        ops.push(ssa_label(OpKind::TryEnd, 90));
        let normal_read = ops.len();
        ops.push(ssa_label(OpKind::Jump, 91));
        ops.push(ssa_label(OpKind::Label, 90));
        let handler_read = ops.len();
        ops.push(OpIR {
            args: Some(vec!["inside".into()]),
            ..OpIR::new(OpKind::Ret)
        });
        ops.push(ssa_label(OpKind::Label, 91));
        ops.push(OpIR {
            args: Some(vec!["before".into()]),
            ..OpIR::new(OpKind::Ret)
        });
        let (mut values, names) = native_ssa_facts(&ops, &[], false);
        let mut function = Function::with_name_signature(
            UserFuncName::default(),
            Signature::new(CallConv::SystemV),
        );
        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let before_var = builder.declare_var(types::I64);
        let inside_var = builder.declare_var(types::I64);
        values.project_variables(
            &names,
            &BTreeMap::from([("before".into(), before_var), ("inside".into(), inside_var)]),
            &ScalarRepresentationPlan::default(),
            &BTreeMap::new(),
        );
        let before_value = builder.ins().iconst(types::I64, 7);
        let inside_value = builder.ins().iconst(types::I64, 37);
        builder.def_var(before_var, before_value);
        builder.def_var(inside_var, inside_value);
        values.capture_operation(&mut builder, 0, &ops[0], &names);
        values.capture_operation(&mut builder, inside, &ops[inside], &names);
        // A handler reachable only by an exception edge still sees a value
        // defined before every observation. Removing those edges fails this.
        assert_eq!(values.read(before_var, handler_read), Some(before_value));
        assert_eq!(values.read(inside_var, normal_read), Some(inside_value));
        assert_eq!(
            values.read(inside_var, handler_read),
            (!early_exception).then_some(inside_value),
            "a failure before the try-body definition must not reuse it",
        );
        builder.ins().return_(&[]);
        builder.seal_all_blocks();
        builder.finalize(crate::test_support::host_frontend_config());
    }
}

#[test]
fn native_ssa_loop_carriers_are_mutable_but_neighbouring_values_are_direct() {
    let ops = vec![
        OpIR::new(OpKind::LoopStart),
        OpIR {
            out: Some("index".into()),
            ..ssa_label(OpKind::LoopIndexStart, 0)
        },
        ssa_constant("fresh", 37),
        OpIR {
            out: Some("next".into()),
            args: Some(vec!["carry".into(), "fresh".into()]),
            ..OpIR::new(OpKind::Add)
        },
        OpIR {
            var: Some("carry".into()),
            args: Some(vec!["next".into()]),
            ..OpIR::new(OpKind::StoreVar)
        },
        OpIR::new(OpKind::LoopEnd),
        OpIR {
            args: Some(vec!["carry".into()]),
            ..OpIR::new(OpKind::Ret)
        },
    ];
    assert!(
        crate::tir::cfg::CFG::build(&ops)
            .loop_depth
            .iter()
            .any(|&depth| depth > 0)
    );
    let (mut values, names) = native_ssa_facts(&ops, &["carry"], false);
    let mut function =
        Function::with_name_signature(UserFuncName::default(), Signature::new(CallConv::SystemV));
    let mut context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut function, &mut context);
    let entry = builder.create_block();
    builder.switch_to_block(entry);
    let carry = builder.declare_var(types::I64);
    let index = builder.declare_var(types::I64);
    let fresh = builder.declare_var(types::I64);
    values.project_variables(
        &names,
        &BTreeMap::from([
            ("carry".into(), carry),
            ("index".into(), index),
            ("fresh".into(), fresh),
        ]),
        &ScalarRepresentationPlan::default(),
        &BTreeMap::new(),
    );
    let zero = builder.ins().iconst(types::I64, 0);
    let expected = builder.ins().iconst(types::I64, 37);
    builder.def_var(carry, zero);
    builder.def_var(index, zero);
    builder.def_var(fresh, expected);
    values.capture_parameters(&mut builder, &["carry".into()], &names);
    values.capture_operation(&mut builder, 1, &ops[1], &names);
    values.capture_operation(&mut builder, 2, &ops[2], &names);
    assert_eq!(values.read(carry, 3), None, "loop-carried binding");
    assert_eq!(values.read(index, 3), None, "native backedge writes");
    assert_eq!(values.read(fresh, 3), Some(expected));
    builder.ins().return_(&[]);
    builder.seal_all_blocks();
    builder.finalize(crate::test_support::host_frontend_config());
}

#[test]
fn native_ssa_resume_reads_do_not_reuse_values_from_an_earlier_invocation() {
    let ops = vec![
        ssa_constant("invocation", 7),
        OpIR {
            state_targets: Some(vec![(1, 10)]),
            ..OpIR::new(OpKind::StateSwitch)
        },
        ssa_constant("saved", 37),
        OpIR {
            args: Some(vec!["saved".into()]),
            ..ssa_label(OpKind::StateYield, 1)
        },
        ssa_label(OpKind::StateLabel, 10),
        ssa_constant("resumed", 41),
        OpIR {
            args: Some(vec!["saved".into()]),
            ..OpIR::new(OpKind::Ret)
        },
    ];
    let (mut values, names) = native_ssa_facts(&ops, &[], true);
    let mut function =
        Function::with_name_signature(UserFuncName::default(), Signature::new(CallConv::SystemV));
    let mut context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut function, &mut context);
    let entry = builder.create_block();
    builder.switch_to_block(entry);
    let mut vars = BTreeMap::new();
    let mut emitted = BTreeMap::new();
    for name in ["invocation", "saved", "resumed"] {
        vars.insert(name.to_string(), builder.declare_var(types::I64));
    }
    values.project_variables(
        &names,
        &vars,
        &ScalarRepresentationPlan::default(),
        &BTreeMap::new(),
    );
    for index in [0, 2, 5] {
        let op = &ops[index];
        let name = op.out.as_ref().unwrap();
        let value = builder.ins().iconst(types::I64, op.value.unwrap());
        builder.def_var(vars[name], value);
        emitted.insert(name.clone(), value);
        values.capture_operation(&mut builder, index, op, &names);
    }
    assert_eq!(values.read(vars["saved"], 3), Some(emitted["saved"]));
    assert_eq!(
        values.read(vars["saved"], 6),
        None,
        "resume bypasses its producer"
    );
    assert_eq!(
        values.read(vars["invocation"], 6),
        Some(emitted["invocation"])
    );
    assert_eq!(values.read(vars["resumed"], 6), Some(emitted["resumed"]));
    builder.ins().return_(&[]);
    builder.seal_all_blocks();
    builder.finalize(crate::test_support::host_frontend_config());
}

#[cfg(debug_assertions)]
#[test]
fn captured_native_ssa_redefinitions_require_the_block_parameter_boundary() {
    let ops = vec![
        ssa_constant("immutable", 37),
        OpIR {
            args: Some(vec!["immutable".into()]),
            ..OpIR::new(OpKind::Ret)
        },
    ];
    let (mut values, names) = native_ssa_facts(&ops, &[], false);
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut function = Function::with_name_signature(UserFuncName::default(), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let target = builder.create_block();
        builder.switch_to_block(entry);
        let var = builder.declare_var(types::I64);
        let expected = builder.ins().iconst(types::I64, 37);
        let different = builder.ins().iconst(types::I64, 41);
        builder.def_var(var, expected);
        values.project_variables(
            &names,
            &BTreeMap::from([("immutable".into(), var)]),
            &ScalarRepresentationPlan::default(),
            &BTreeMap::new(),
        );
        builder.install_ssa_values(values);
        builder.capture_operation(0, &ops[0], &names);
        builder.begin_op(1);
        builder.def_var(var, expected);
        let unexpected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            builder.def_var(var, different)
        }));
        assert!(
            unexpected.is_err(),
            "an unregistered extra writer must be detected"
        );
        let param = builder.append_block_param(target, types::I64);
        builder.ins().jump(target, &[expected.into()]);
        builder.switch_to_block(target);
        builder.bind_var_to_block_param(var, target, 0);
        assert_eq!(builder.use_var(var), expected);
        builder.end_op();
        assert_eq!(
            builder.use_var(var),
            param,
            "epilogue reads leave the source operation"
        );
        builder.ins().return_(&[param]);
        builder.seal_all_blocks();
        builder.finalize(crate::test_support::host_frontend_config());
    }
    verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
}

#[test]
fn block_transport_plan_emits_typed_args_and_rebinds_values() {
    let sig = Signature::new(CallConv::SystemV);
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let word_var = builder.declare_var(types::I64);
    let float_var = builder.declare_var(types::F64);
    let entry = builder.create_block();
    let target = builder.create_block();
    let plan =
        BlockTransportPlan::for_test(vec![float_var, word_var], vec![types::F64, types::I64]);
    plan.append_block_params(&mut builder, target);

    switch_to_block_materialized(&mut builder, entry);
    let word = builder.ins().iconst(types::I64, 17);
    let float = builder.ins().f64const(3.5);
    builder.def_var(word_var, word);
    builder.def_var(float_var, float);
    let args = plan.edge_args(&mut builder);
    jump_block(&mut builder, target, &args);
    builder.seal_block(entry);

    switch_to_block_materialized(&mut builder, target);
    plan.bind_block_params(&mut builder, target);

    assert_eq!(builder.use_var(float_var), builder.block_params(target)[0]);
    assert_eq!(builder.use_var(word_var), builder.block_params(target)[1]);
}

#[test]
fn materialize_label_block_defines_unreached_forward_label() {
    let sig = Signature::new(CallConv::SystemV);
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let entry = builder.create_block();
    let later = builder.create_block();
    let detached_label = builder.create_block();

    switch_to_block_materialized(&mut builder, entry);
    builder.ins().jump(later, &[]);
    builder.seal_block(entry);

    let mut is_block_filled = true;
    materialize_label_block(&mut builder, detached_label, &mut is_block_filled, None);

    assert!(
        builder.func.layout.is_block_inserted(detached_label),
        "textual label must materialize its block even before any emitted predecessor reaches it",
    );
    assert_eq!(builder.current_block(), Some(detached_label));
    assert!(
        !is_block_filled,
        "materialized label block must be open for emission"
    );
}

#[test]
fn materialize_label_block_does_not_self_jump_current_resume_block() {
    let sig = Signature::new(CallConv::SystemV);
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let resume_block = builder.create_block();
    switch_to_block_materialized(&mut builder, resume_block);

    let mut is_block_filled = false;
    materialize_label_block(&mut builder, resume_block, &mut is_block_filled, None);

    assert_eq!(builder.current_block(), Some(resume_block));
    assert!(
        !is_block_filled,
        "state_label materialization must leave the current resume block open"
    );
    assert!(
        builder.func.layout.last_inst(resume_block).is_none(),
        "state_label materialization must not emit a self-jump predecessor"
    );
}

// â”€â”€ scan_loop_int_sum_reduction tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[test]
fn immutable_reads_do_not_construct_phis_in_unsealed_native_blocks() {
    let ops = vec![
        OpIR {
            out: Some("immutable".into()),
            value: Some(37),
            ..OpIR::new(OpKind::Const)
        },
        OpIR {
            args: Some(vec!["immutable".into()]),
            ..OpIR::new(OpKind::Ret)
        },
    ];
    let names = crate::tir::cfg_liveness::analyze_simple_cfg_liveness_facts(&ops).names;
    let mut values = NativeSsaValues::for_test(&[], &ops, &names);
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut function = Function::with_name_signature(UserFuncName::default(), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        let var = builder.declare_var(types::I64);
        let expected = builder.ins().iconst(types::I64, 37);
        builder.def_var(var, expected);
        values.project_variables(
            &names,
            &BTreeMap::from([("immutable".into(), var)]),
            &ScalarRepresentationPlan::default(),
            &BTreeMap::new(),
        );
        builder.install_ssa_values(values);
        builder.capture_operation(0, &ops[0], &names);
        builder.begin_op(1);
        let mut blocks = vec![entry];
        for _ in 0..128 {
            let next = builder.create_block();
            builder.ins().jump(next, &[]);
            builder.switch_to_block(next);
            blocks.push(next);
        }
        let before = builder.func.dfg.num_values();
        assert_eq!(builder.use_var(var), expected);
        assert_eq!(builder.func.dfg.num_values(), before);
        builder.ins().return_(&[expected]);
        for block in blocks {
            builder.seal_block(block);
        }
        builder.finalize(crate::test_support::host_frontend_config());
    }
    verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
}

#[test]
fn unique_dominating_definition_still_requires_earlier_emission() {
    let initializer = OpIR {
        out: Some("immutable".into()),
        ..OpIR::new(OpKind::Const)
    };
    let ops = vec![
        OpIR {
            value: Some(9),
            ..OpIR::new(OpKind::Jump)
        },
        OpIR {
            value: Some(7),
            ..OpIR::new(OpKind::Label)
        },
        OpIR {
            args: Some(vec!["immutable".into()]),
            ..OpIR::new(OpKind::Ret)
        },
        OpIR {
            value: Some(9),
            ..OpIR::new(OpKind::Label)
        },
        initializer,
        OpIR {
            value: Some(7),
            ..OpIR::new(OpKind::Jump)
        },
    ];
    let names = crate::tir::cfg_liveness::analyze_simple_cfg_liveness_facts(&ops).names;
    let values = NativeSsaValues::for_test(&[], &ops, &names);
    assert!(!values.can_bind_directly(
        names.id("immutable").unwrap(),
        crate::tir::dominators::SimpleProgramPoint::Before(1)
    ));
    assert!(values.can_bind_directly(
        names.id("immutable").unwrap(),
        crate::tir::dominators::SimpleProgramPoint::Before(5)
    ));
}

#[test]
fn internal_single_predecessor_chain_resolves_only_demanded_values() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut function = Function::with_name_signature(UserFuncName::default(), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let mut sealed = BTreeSet::new();
        let entry = builder.create_block();
        switch_to_block_materialized(&mut builder, entry);
        seal_block_once(&mut builder, &mut sealed, entry);
        let mut homes = Vec::new();
        for value in 0..128 {
            let home = builder.declare_var(types::I64);
            let value = builder.ins().iconst(types::I64, value);
            builder.def_var(home, value);
            homes.push((home, value));
        }
        for _ in 0..128 {
            let next = builder.create_block();
            builder.ins().jump(next, &[]);
            switch_to_block_materialized(&mut builder, next);
            seal_block_once(&mut builder, &mut sealed, next);
        }
        let (home, expected) = homes[37];
        assert_eq!(builder.use_var(home), expected);
        builder.ins().return_(&[expected]);
        builder.finalize(crate::test_support::host_frontend_config());
    }
    for block in function.layout.blocks() {
        assert!(function.dfg.block_params(block).is_empty());
    }
    verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
}

#[test]
fn internal_diamond_preserves_redefinitions_before_and_after_sealing() {
    for seal_before_use in [false, true] {
        let mut sig = Signature::new(CallConv::SystemV);
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let mut function = Function::with_name_signature(UserFuncName::default(), sig);
        let mut context = FunctionBuilderContext::new();
        let (left_edge, right_edge, join) = {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            let mut sealed = BTreeSet::new();
            let home = builder.declare_var(types::I64);
            let entry = builder.create_block();
            let left = builder.create_block();
            let right = builder.create_block();
            let join = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            switch_to_block_materialized(&mut builder, entry);
            seal_block_once(&mut builder, &mut sealed, entry);
            let condition = builder.block_params(entry)[0];
            let initial = builder.ins().iconst(types::I64, 11);
            builder.def_var(home, initial);
            builder.ins().brif(condition, left, &[], right, &[]);
            switch_to_block_materialized(&mut builder, left);
            seal_block_once(&mut builder, &mut sealed, left);
            let replaced = builder.ins().iconst(types::I64, 23);
            builder.def_var(home, replaced);
            let final_left = builder.ins().iconst(types::I64, 47);
            builder.def_var(home, final_left);
            let left_edge = builder.ins().jump(join, &[]);
            switch_to_block_materialized(&mut builder, right);
            seal_block_once(&mut builder, &mut sealed, right);
            assert_eq!(builder.use_var(home), initial);
            let right_edge = builder.ins().jump(join, &[]);
            switch_to_block_materialized(&mut builder, join);
            if seal_before_use {
                seal_block_once(&mut builder, &mut sealed, join);
            }
            let result = builder.use_var(home);
            if !seal_before_use {
                seal_block_once(&mut builder, &mut sealed, join);
            }
            assert_eq!(builder.block_params(join), &[result]);
            builder.ins().return_(&[result]);
            builder.finalize(crate::test_support::host_frontend_config());
            (left_edge, right_edge, join)
        };
        // Inspect each actual edge's constant origin, not SSA value numbering
        // or merely the existence of a join parameter. The left arm must carry
        // its final write (47); the right arm must retain the initial write (11).
        for (edge, expected) in [(left_edge, 47), (right_edge, 11)] {
            use cranelift_codegen::ir::{BlockArg, InstructionData, Opcode, ValueDef};
            let destinations = function.dfg.insts[edge]
                .branch_destination(&function.dfg.jump_tables, &function.dfg.exception_tables);
            assert_eq!(destinations.len(), 1);
            assert_eq!(destinations[0].block(&function.dfg.value_lists), join);
            let arguments: Vec<_> = destinations[0].args(&function.dfg.value_lists).collect();
            let [BlockArg::Value(incoming)] = arguments.as_slice() else {
                panic!("diamond edge must pass exactly one SSA value: {arguments:?}");
            };
            let incoming = function.dfg.resolve_aliases(*incoming);
            let ValueDef::Result(definition, _) = function.dfg.value_def(incoming) else {
                panic!("diamond edge must originate in its arm's constant");
            };
            let InstructionData::UnaryImm {
                opcode: Opcode::Iconst,
                imm,
            } = function.dfg.insts[definition]
            else {
                panic!("diamond edge must originate in an integer constant");
            };
            assert_eq!(imm.bits(), expected);
        }
        verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
    }
}

#[test]
fn seal_block_once_rejects_unmaterialized_block_without_recording_success() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut function = Function::with_name_signature(UserFuncName::default(), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let mut sealed = BTreeSet::new();
        let home = builder.declare_var(types::I64);
        let entry = builder.create_block();
        let next = builder.create_block();
        switch_to_block_materialized(&mut builder, entry);
        seal_block_once(&mut builder, &mut sealed, entry);
        let initial = builder.ins().iconst(types::I64, 11);
        builder.def_var(home, initial);
        builder.ins().jump(next, &[]);

        let before = sealed.clone();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            seal_block_once(&mut builder, &mut sealed, next);
        }))
        .expect_err("sealing before materialization must fail at Molt's boundary");
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .expect("sealing diagnostic must be text");
        assert!(message.contains("cannot seal unmaterialized block"));
        assert_eq!(sealed, before, "failed sealing must preserve the record");
        assert!(!builder.func.layout.is_block_inserted(next));

        // Materializing and retrying must perform the real seal. A false record
        // from the rejected attempt would leave use_var creating a parameter.
        switch_to_block_materialized(&mut builder, next);
        seal_block_once(&mut builder, &mut sealed, next);
        seal_block_once(&mut builder, &mut sealed, next);
        assert!(sealed.contains(&next));
        assert_eq!(builder.use_var(home), initial);
        assert!(builder.block_params(next).is_empty());
        builder.ins().return_(&[initial]);
        builder.finalize(crate::test_support::host_frontend_config());
    }
    verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
}
