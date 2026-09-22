use super::*;
use cranelift_codegen::ir::{ExternalName, InstructionData};

#[test]
fn heap_literal_results_retain_independently_and_unique_anchors_release_once() {
    use super::super::fc::const_literals::{
        LiteralFailureExit, handle_const_literal_op, prepare_heap_literals,
    };

    // Unproven scalar facts require the boxed lane, including full-i64 consts.
    let plan = ScalarRepresentationPlan::default();
    let mut ops = Vec::new();
    for index in 0..2 {
        for kind in [
            "const_str",
            "const_bytes",
            "const_bigint",
            "const",
            "const_int",
            "load_const",
        ] {
            ops.push(OpIR {
                kind: kind.into(),
                out: Some(format!("{kind}_{index}")),
                s_value: Some(if kind == "const_bigint" {
                    i64::MAX.to_string()
                } else {
                    "non-interned literal".into()
                }),
                bytes: (kind == "const_bytes").then(|| vec![0, 255, 128]),
                value: matches!(kind, "const" | "const_int" | "load_const").then_some(i64::MAX),
                ..OpIR::default()
            });
        }
    }
    let input = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "literal_owners".into(),
        params: vec![],
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };
    let mut backend = SimpleBackend::new();
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut function = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut context = FunctionBuilderContext::new();
    let (retain, release, materializers);
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let exit = builder.create_block();
        builder.append_block_param(exit, types::I64);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let mut vars = BTreeMap::new();
        for op in &input.ops {
            let out = op.out.as_ref().unwrap();
            for name in [out.clone(), format!("{out}_ptr"), format!("{out}_len")] {
                vars.insert(name, builder.declare_var(types::I64));
            }
        }
        let mut refs = BTreeMap::new();
        retain = import_func_ref(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut refs,
            "molt_inc_ref_obj",
            &[types::I64],
            &[],
        );
        release = import_func_ref(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut refs,
            "molt_dec_ref_obj",
            &[types::I64],
            &[],
        );
        let (hoists, materialization) = prepare_heap_literals(&input, &mut builder, &plan);
        for runtime_func in [
            "molt_string_from_bytes",
            "molt_bytes_from_bytes",
            "molt_bigint_from_str",
        ] {
            assert!(
                !backend.import_ids.contains_key(runtime_func),
                "literal preparation must not import or emit constructors"
            );
        }
        materialization.materialize(
            &hoists,
            &mut backend.module,
            &mut backend.import_ids,
            &mut backend.data_pool,
            &mut backend.next_data_id,
            &mut builder,
            &vars,
            LiteralFailureExit {
                block: exit,
                returns_value: true,
            },
        );
        materializers = [
            "molt_string_from_bytes",
            "molt_bytes_from_bytes",
            "molt_bigint_from_str",
        ]
        .map(|name| backend.import_ids[name].0);
        for (index, op) in input.ops.iter().enumerate() {
            handle_const_literal_op(
                op,
                &mut backend.module,
                &mut backend.import_ids,
                &mut builder,
                &vars,
                &plan,
                &hoists,
                retain,
            );
            // Explicit IR drops can release every earlier output, even when
            // another operation later produces the identical immutable value.
            if index + 1 != input.ops.len() {
                let value = builder.use_var(vars[op.out.as_ref().unwrap()]);
                builder.ins().call(release, &[value]);
            }
        }
        let result = builder.use_var(vars[input.ops.last().unwrap().out.as_ref().unwrap()]);
        jump_block(&mut builder, exit, &[result]);
        switch_to_block_materialized(&mut builder, exit);
        builder.seal_block(exit);
        hoists.release_anchors(&mut builder, release);
        let result = builder.block_params(exit)[0];
        builder.ins().return_(&[result]);
        builder.finalize();
    }
    verify_function(&function, &settings::Flags::new(settings::builder()))
        .expect("owned literal lowering must verify");
    let entry = function.layout.entry_block().unwrap();
    let initialized_anchors = function
        .layout
        .block_insts(entry)
        .take_while(|&inst| !matches!(function.dfg.insts[inst], InstructionData::Call { .. }))
        .filter(|&inst| {
            function.dfg.insts[inst].opcode() == cranelift_codegen::ir::Opcode::StackStore
        })
        .count();
    assert_eq!(
        initialized_anchors, 3,
        "all anchors precede every failure edge"
    );
    let calls: Vec<_> = function
        .layout
        .blocks()
        .flat_map(|block| function.layout.block_insts(block))
        .filter_map(|inst| match function.dfg.insts[inst] {
            InstructionData::Call { func_ref, .. } => {
                Some((func_ref, function.dfg.inst_args(inst).to_vec()))
            }
            _ => None,
        })
        .collect();
    for materializer in materializers {
        assert_eq!(
            calls
                .iter()
                .filter(|(callee, _)| {
                    let ExternalName::User(name) = function.dfg.ext_funcs[*callee].name else {
                        return false;
                    };
                    let name = &function.params.user_named_funcs()[name];
                    name.namespace == 0 && name.index == materializer.as_u32()
                })
                .count(),
            1,
            "one constructor per unique payload, regardless of local FuncRef aliases"
        );
    }
    assert_eq!(
        calls.iter().filter(|(callee, _)| *callee == retain).count(),
        input.ops.len()
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(callee, _)| *callee == release)
            .count(),
        input.ops.len() - 1 + 3
    );
    for (index, (_, args)) in calls
        .iter()
        .enumerate()
        .filter(|(_, (callee, _))| *callee == retain)
        .take(input.ops.len() - 1)
    {
        assert_eq!(
            calls[index + 1],
            (release, args.clone()),
            "each drop consumes its own result"
        );
    }
    assert!(
        calls[calls.len() - 3..]
            .iter()
            .all(|(callee, _)| *callee == release)
    );
}

#[test]
fn integer_literal_aliases_share_raw_materialization_and_discard_contracts() {
    use super::super::fc::const_literals::{
        collect_loop_entry_const_defs, op_uses_heap_literal_data_segment,
    };
    use crate::native_backend::simple_backend::tests::{
        compile_selected_functions_direct, emit_direct_object,
    };
    for kind in ["const", "const_int", "load_const"] {
        for value in [i64::MIN, 0, i64::MAX] {
            let input = FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "integer_literal_contract".into(),
                params: vec![],
                param_types: None,
                ops: vec![
                    OpIR {
                        kind: kind.into(),
                        value: Some(value),
                        out: Some("value".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: kind.into(),
                        value: Some(i64::MAX),
                        out: None,
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: kind.into(),
                        value: Some(i64::MAX),
                        out: Some("none".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".into(),
                        args: Some(vec!["value".into()]),
                        ..OpIR::default()
                    },
                ],
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            };
            let plan = native_representation_plan_for_test(&input);
            let constants = crate::build_const_int_map(&input);
            assert_eq!(constants, BTreeMap::from([("value".into(), value)]));
            let entry = collect_loop_entry_const_defs(&input, &plan, &constants);
            assert_eq!(
                entry.get("value").copied(),
                Some(if plan.is_raw_int_carrier_name("value") {
                    value
                } else {
                    molt_codegen_abi::box_int_bits(value)
                })
            );
            assert!(!entry.contains_key("none"));
            for discarded in &input.ops[1..3] {
                assert!(
                    !op_uses_heap_literal_data_segment(discarded),
                    "{kind}: discarded integers have no frame anchors"
                );
            }
            let backend =
                compile_selected_functions_direct(vec![input], &["integer_literal_contract"]);
            let function = &backend
                .deferred_defines
                .iter()
                .find(|deferred| deferred.name == "integer_literal_contract")
                .unwrap()
                .func;
            verify_function(function, &settings::Flags::new(settings::builder()))
                .unwrap_or_else(|errors| panic!("{kind}: {errors}\n{}", function.display()));
            assert!(
                !backend.import_ids.contains_key("molt_bigint_from_str"),
                "raw literals and discarded outputs need no boxed-literal constructor: {kind}"
            );
            assert!(
                native_object_symbols(&emit_direct_object(backend))
                    .defined
                    .contains("integer_literal_contract")
            );
        }
    }
}
