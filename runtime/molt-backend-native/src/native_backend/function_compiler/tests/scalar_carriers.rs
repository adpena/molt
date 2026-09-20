use super::*;

#[test]
fn captured_scalar_transport_preserves_raw_outputs_and_shares_one_box() {
    use MergeRebindStorageKind::{BoxedI64, RawBool, RawF64, RawI64};
    for source_storage in [RawI64, RawF64, RawBool, BoxedI64] {
        for homes in [
            [source_storage, source_storage],
            [source_storage, BoxedI64],
            [BoxedI64, source_storage],
            [BoxedI64, BoxedI64],
        ] {
            let mut backend = SimpleBackend::new();
            let mut sig = Signature::new(CallConv::SystemV);
            for home in homes {
                sig.returns
                    .push(AbiParam::new(merge_rebind_storage_clif_type(home)));
            }
            let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
            let mut context = FunctionBuilderContext::new();
            let mut refs = BTreeMap::new();
            {
                let mut builder = FunctionBuilder::new(&mut func, &mut context);
                let entry = builder.create_block();
                builder.switch_to_block(entry);
                builder.seal_block(entry);
                let original = if source_storage == RawF64 {
                    builder.ins().f64const(-0.0)
                } else {
                    builder.ins().iconst(
                        types::I64,
                        if source_storage == RawBool {
                            1
                        } else {
                            1_i64 << 62
                        },
                    )
                };
                let mut incoming = CapturedScalarTransport::new(original, source_storage);
                let mut sealed = BTreeSet::from([entry]);
                let values = homes.map(|home| {
                    incoming.value_for_storage(
                        &mut backend.module,
                        &mut backend.import_ids,
                        &mut builder,
                        &mut refs,
                        &mut sealed,
                        &crate::NanBoxConsts::new(),
                        home,
                    )
                });
                for (home, value) in homes.into_iter().zip(values) {
                    if home == source_storage {
                        assert_eq!(
                            value, original,
                            "same-representation transfer must preserve original bits"
                        );
                    }
                }
                if homes[0] == homes[1] {
                    assert_eq!(
                        values[0], values[1],
                        "sibling boxed outputs must share identity"
                    );
                }
                if homes.contains(&BoxedI64) {
                    assert_eq!(incoming.take_boxed_owner(), source_storage != BoxedI64);
                    assert!(
                        !incoming.take_boxed_owner(),
                        "a materialization supplies exactly one credit"
                    );
                }
                builder.ins().return_(&values);
                builder.finalize();
            }
            verify_function(&func, &settings::Flags::new(settings::builder())).unwrap();
            let box_calls = refs.get("molt_int_from_i64").map_or(0, |box_fn| {
                func.layout.blocks().flat_map(|block| func.layout.block_insts(block)).filter(|inst| {
                    matches!(func.dfg.insts[*inst], cranelift_codegen::ir::InstructionData::Call { func_ref, .. } if func_ref == *box_fn)
                }).count()
            });
            assert_eq!(
                box_calls,
                usize::from(source_storage == RawI64 && homes.contains(&BoxedI64))
            );
        }
    }
}

fn lower_unary_numeric_carrier_fixture(
    kind: &str,
    source: &str,
    out: &str,
) -> (String, BTreeSet<String>) {
    let plan = scalar_transport_plan_for_boxed_transport_homes();
    lower_unary_numeric_carrier_with_plan(
        kind,
        source,
        out,
        &plan,
        if source == "bool_home" { 1 } else { 7 },
    )
}

fn lower_unary_numeric_carrier_with_plan(
    kind: &str,
    source: &str,
    out: &str,
    plan: &ScalarRepresentationPlan,
    source_bits: u64,
) -> (String, BTreeSet<String>) {
    let mut backend = SimpleBackend::new();
    let mut sig = Signature::new(CallConv::SystemV);
    let home_type = |name: &str| {
        if plan.is_float_unboxed(name) {
            types::F64
        } else {
            types::I64
        }
    };
    sig.returns.push(AbiParam::new(home_type(out)));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut func, &mut context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let mut vars = BTreeMap::new();
        for name in [source, out] {
            if !vars.contains_key(name) {
                vars.insert(name.to_string(), builder.declare_var(home_type(name)));
            }
        }
        let raw = if plan.is_float_unboxed(source) {
            builder.ins().f64const(f64::from_bits(source_bits))
        } else {
            builder.ins().iconst(types::I64, source_bits as i64)
        };
        builder.def_var(vars[source], raw);
        let mut refs = BTreeMap::new();
        let inc_ref = import_func_ref(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut refs,
            "molt_inc_ref_obj",
            &[types::I64],
            &[],
        );
        let op = OpIR {
            kind: kind.to_string(),
            args: Some(vec![source.to_string()]),
            out: Some(out.to_string()),
            ..OpIR::default()
        };
        let mut sealed = BTreeSet::from([entry]);
        super::super::fc::unary_logic::handle_unary_logic_op(
            &op,
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut refs,
            &mut sealed,
            &vars,
            plan,
            inc_ref,
            true,
            &crate::NanBoxConsts::new(),
        );
        if plan.is_full_deopt_int_name(source) && matches!(kind, "neg" | "abs") {
            let box_fn = refs
                .get("molt_int_from_i64")
                .expect("full-width unary input must use overflow-safe boxing");
            let mut exact_box_calls = 0;
            for block in builder.func.layout.blocks() {
                for inst in builder.func.layout.block_insts(block) {
                    if let cranelift_codegen::ir::InstructionData::Call { func_ref, .. } =
                        builder.func.dfg.insts[inst]
                        && func_ref == *box_fn
                    {
                        assert_eq!(
                            builder.func.dfg.inst_args(inst),
                            &[raw],
                            "full-width boxing must consume the original value, not an int47 payload"
                        );
                        exact_box_calls += 1;
                    }
                    if builder.func.dfg.insts[inst].opcode() == cranelift_codegen::ir::Opcode::Isub
                    {
                        assert!(
                            !builder.func.dfg.inst_args(inst).contains(&raw),
                            "unchecked unary negation must not consume a full-width source"
                        );
                    }
                }
            }
            assert_eq!(
                exact_box_calls, 1,
                "one overflow-safe input materialization must own full-width unary dispatch"
            );
        }
        let result = builder.use_var(vars[out]);
        builder.ins().return_(&[result]);
        builder.finalize();
    }
    verify_function(&func, &settings::Flags::new(settings::builder()))
        .expect("unary numeric carrier CFG must verify");
    (
        func.display().to_string(),
        backend
            .import_ids
            .keys()
            .map(|name| name.to_string())
            .collect(),
    )
}

#[test]
fn unary_neg_and_abs_preserve_full_width_checked_arithmetic_inputs() {
    for (lhs, rhs) in [
        (-(1_i64 << 32), 1_i64 << 31),
        (-(1_i64 << 31), 1_i64 << 31),
        (1_i64 << 31, 1_i64 << 31),
    ] {
        let value = lhs.checked_mul(rhs).unwrap();
        let plan = representation_plan_for_ops(&[
            OpIR {
                kind: "const_int".into(),
                out: Some("lhs".into()),
                value: Some(lhs),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_int".into(),
                out: Some("rhs".into()),
                value: Some(rhs),
                ..OpIR::default()
            },
            OpIR {
                kind: "checked_mul".into(),
                args: Some(vec!["lhs".into(), "rhs".into()]),
                var: Some("full_home".into()),
                out: Some("overflow".into()),
                ..OpIR::default()
            },
        ]);
        assert!(
            plan.is_full_deopt_int_name("full_home"),
            "checked product {value} must expose the full-width carrier"
        );
        assert!(!plan.is_inline_safe_int_name("full_home"));
        for (kind, runtime) in [("neg", "molt_neg"), ("abs", "molt_abs_builtin")] {
            let (clif, imports) = lower_unary_numeric_carrier_with_plan(
                kind,
                "full_home",
                "boxed_result",
                &plan,
                value as u64,
            );
            assert!(
                imports.contains(runtime),
                "{kind}({value}) requires BigInt-correct runtime dispatch: {clif}"
            );
            assert!(
                imports.contains("molt_int_from_i64"),
                "{kind}({value}) must not truncate its input: {clif}"
            );
        }
    }
}

#[test]
fn unary_numeric_float_results_preserve_physical_f64_homes_and_zero_sign() {
    let plan = scalar_transport_plan_for_float_home();
    for kind in ["neg", "pos"] {
        for value in [1.25_f64, 0.0_f64, -0.0_f64] {
            let (clif, imports) = lower_unary_numeric_carrier_with_plan(
                kind,
                "float_home",
                "float_home",
                &plan,
                value.to_bits(),
            );
            assert!(
                !imports.iter().any(|name| matches!(
                    name.as_str(),
                    "molt_neg" | "molt_pos" | "molt_float_from_obj"
                )),
                "{kind} must retain its physical F64 result through the numeric sink: {clif}"
            );
            assert_eq!(
                clif.matches("fneg").count(),
                usize::from(kind == "neg"),
                "unary float sign behavior: {clif}"
            );
            assert!(
                !clif.contains("fsub"),
                "subtraction from zero is not IEEE unary negation: {clif}"
            );
        }
    }
}

#[test]
fn unary_numeric_raw_operands_cannot_define_boxed_results_directly() {
    for (kind, runtime) in [
        ("neg", "molt_neg"),
        ("pos", "molt_pos"),
        ("abs", "molt_abs_builtin"),
        ("invert", "molt_invert"),
    ] {
        let (clif, imports) = lower_unary_numeric_carrier_fixture(kind, "int_home", "boxed_result");
        assert!(
            imports.contains(runtime),
            "{kind} must use boxed result transport: {clif}"
        );
        assert!(
            clif.contains("call"),
            "{kind} must retain the boxed runtime path: {clif}"
        );
    }
}

#[test]
fn unary_numeric_bool_operands_unbox_runtime_results_for_integer_homes() {
    for (kind, runtime) in [
        ("neg", "molt_neg"),
        ("pos", "molt_pos"),
        ("abs", "molt_abs_builtin"),
        ("invert", "molt_invert"),
    ] {
        let (clif, imports) = lower_unary_numeric_carrier_fixture(kind, "bool_home", "int_home");
        assert!(
            imports.contains(runtime),
            "{kind} must preserve Bool runtime semantics: {clif}"
        );
        assert!(
            clif.contains("sshr"),
            "{kind} must extract the signed integer payload before storing into its raw home: {clif}"
        );
    }
}

#[test]
fn unary_numeric_proven_raw_integer_results_avoid_runtime_calls() {
    for kind in ["neg", "pos", "abs", "invert"] {
        let (clif, imports) = lower_unary_numeric_carrier_fixture(kind, "int_home", "int_home");
        assert!(
            !imports.iter().any(|name| matches!(
                name.as_str(),
                "molt_neg" | "molt_pos" | "molt_abs_builtin" | "molt_invert"
            )),
            "{kind} must preserve proven raw operations: {clif}"
        );
    }
}

#[test]
fn native_container_dispatch_uses_tir_container_facts() {
    let dict_index = OpIR {
        kind: "index".to_string(),
        args: Some(vec!["mapping".to_string(), "key".to_string()]),
        out: Some("item".to_string()),
        ..OpIR::default()
    };
    let dict_plan = representation_plan_for_typed_ops(
        &["mapping", "key"],
        Some(vec!["dict[str, int]", "str"]),
        std::slice::from_ref(&dict_index),
    );
    assert_eq!(
        index_fallback_import_name(&dict_plan, &dict_index, false),
        "molt_dict_getitem"
    );

    let tuple_index = OpIR {
        kind: "index".to_string(),
        args: Some(vec!["items".to_string(), "idx".to_string()]),
        out: Some("item".to_string()),
        ..OpIR::default()
    };
    let tuple_plan = representation_plan_for_typed_ops(
        &["items", "idx"],
        Some(vec!["tuple[int, str]", "int"]),
        std::slice::from_ref(&tuple_index),
    );
    assert_eq!(
        index_fallback_import_name(&tuple_plan, &tuple_index, false),
        "molt_tuple_getitem"
    );

    let dict_store = OpIR {
        kind: "store_index".to_string(),
        args: Some(vec![
            "mapping".to_string(),
            "key".to_string(),
            "value".to_string(),
        ]),
        ..OpIR::default()
    };
    let dict_store_plan = representation_plan_for_typed_ops(
        &["mapping", "key", "value"],
        Some(vec!["dict[str, int]", "str", "int"]),
        std::slice::from_ref(&dict_store),
    );
    assert_eq!(
        store_index_fallback_import_name(&dict_store_plan, &dict_store),
        "molt_dict_setitem"
    );
}

#[test]
fn native_container_dispatch_ignores_transport_only_container_type() {
    let mut transport_index = OpIR {
        kind: "index".to_string(),
        args: Some(vec!["items".to_string(), "idx".to_string()]),
        out: Some("item".to_string()),
        ..OpIR::default()
    };
    transport_index.container_type = Some("tuple".to_string());
    let plan = representation_plan_for_typed_ops(
        &["items", "idx"],
        None,
        std::slice::from_ref(&transport_index),
    );

    assert_eq!(
        index_fallback_import_name(&plan, &transport_index, false),
        "molt_index"
    );
    assert!(
        !generic_list_int_lane_eligible(&plan, &transport_index, true),
        "transport-only container_type must not enable native generic-list inlining"
    );

    let mut transport_store = OpIR {
        kind: "store_index".to_string(),
        args: Some(vec![
            "mapping".to_string(),
            "key".to_string(),
            "value".to_string(),
        ]),
        ..OpIR::default()
    };
    transport_store.container_type = Some("dict".to_string());
    let store_plan = representation_plan_for_typed_ops(
        &["mapping", "key", "value"],
        None,
        std::slice::from_ref(&transport_store),
    );

    assert_eq!(
        store_index_fallback_import_name(&store_plan, &transport_store),
        "molt_store_index"
    );
}

#[test]
fn native_generic_list_reads_inline_but_stores_use_runtime_authority() {
    let list_index = OpIR {
        kind: "index".to_string(),
        args: Some(vec!["items".to_string(), "idx".to_string()]),
        out: Some("item".to_string()),
        ..OpIR::default()
    };
    let plan = representation_plan_for_typed_ops(
        &["items", "idx"],
        Some(vec!["list[int]", "int"]),
        std::slice::from_ref(&list_index),
    );

    assert!(generic_list_int_lane_eligible(&plan, &list_index, true));
    assert!(!generic_list_int_lane_eligible(&plan, &list_index, false));

    let list_store = OpIR {
        kind: "store_index".to_string(),
        args: Some(vec![
            "items".to_string(),
            "idx".to_string(),
            "value".to_string(),
        ]),
        ..OpIR::default()
    };
    let store_plan = representation_plan_for_typed_ops(
        &["items", "idx", "value"],
        Some(vec!["list[int]", "int", "int"]),
        std::slice::from_ref(&list_store),
    );
    assert_eq!(
        store_index_fallback_import_name(&store_plan, &list_store),
        "molt_store_index"
    );

    let list_int_new = OpIR {
        kind: "list_int_new".to_string(),
        out: Some("flat_items".to_string()),
        ..OpIR::default()
    };
    let flat_store = OpIR {
        kind: "store_index".to_string(),
        args: Some(vec![
            "flat_items".to_string(),
            "idx".to_string(),
            "value".to_string(),
        ]),
        out: Some("flat_result".to_string()),
        ..OpIR::default()
    };
    let flat_ops = [list_int_new, flat_store.clone()];
    let flat_plan =
        representation_plan_for_typed_ops(&["idx", "value"], Some(vec!["int", "int"]), &flat_ops);
    assert!(flat_plan.op_has_container_storage(1, &flat_store, ContainerStorageKind::FlatListInt,));
    assert_eq!(
        store_index_fallback_import_name(&flat_plan, &flat_store),
        "molt_store_index",
        "physical storage proof alone cannot prove that ABI publication never promoted the list",
    );
}

#[test]
fn raw_bool_boxing_accepts_i64_carrier() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut func, &mut context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let raw = builder.ins().iconst(types::I64, 1);
        let nbc = crate::NanBoxConsts::new();
        let boxed = box_raw_bool_value(&mut builder, raw, &nbc);
        builder.ins().return_(&[boxed]);
        builder.finalize();
    }

    let flags = settings::Flags::new(settings::builder());
    verify_function(&func, &flags).expect("raw bool boxing must verify with an i64 carrier");
}

#[test]
fn native_int_boxing_constants_materialized_at_site() {
    let mut backend = SimpleBackend::new();
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 1), sig);
    let mut context = FunctionBuilderContext::new();
    let int_mask_needle;
    let int_tag_needle;
    {
        let mut builder = FunctionBuilder::new(&mut func, &mut context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let nbc = crate::NanBoxConsts::new();
        int_mask_needle = format!("iconst.i64 {:#x}", nbc.int_mask);
        int_tag_needle = format!("iconst.i64 {:#x}", nbc.qnan_tag_int);

        let raw_zero = builder.ins().iconst(types::I64, 0);
        let mut import_refs = BTreeMap::new();
        let mut sealed_blocks = BTreeSet::from([entry]);
        let boxed = box_raw_i64_value_overflow_safe(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut import_refs,
            &mut sealed_blocks,
            raw_zero,
        );
        builder.ins().return_(&[boxed]);
        builder.finalize();
    }

    let flags = settings::Flags::new(settings::builder());
    verify_function(&func, &flags).expect("split raw-i64 escape boxing CFG must verify");
    let clif = func.display().to_string();
    let normalized_clif = clif.replace('_', "");
    assert!(
        normalized_clif.contains(&int_mask_needle),
        "raw-i64 escape boxing must materialize INT_MASK at the boxing site:\n{clif}"
    );
    assert!(
        normalized_clif.contains(&int_tag_needle),
        "raw-i64 escape boxing must materialize QNAN|TAG_INT at the boxing site:\n{clif}"
    );
}

#[test]
fn boxed_transport_defines_scalar_primary_homes() {
    let mut backend = SimpleBackend::new();
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut func, &mut context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let float_var = builder.declare_var(types::F64);
        let bool_var = builder.declare_var(types::I64);
        let int_var = builder.declare_var(types::I64);
        let mut vars = BTreeMap::new();
        vars.insert("float_home".to_string(), float_var);
        vars.insert("bool_home".to_string(), bool_var);
        vars.insert("int_home".to_string(), int_var);

        let representation_plan = scalar_transport_plan_for_boxed_transport_homes();
        let mut import_refs = BTreeMap::new();
        let nbc = crate::NanBoxConsts::new();

        let boxed_float = builder.ins().iconst(types::I64, 1.25f64.to_bits() as i64);
        def_var_from_boxed_transport(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut import_refs,
            &vars,
            &representation_plan,
            &nbc,
            "float_home",
            boxed_float,
        );

        let raw_bool = builder.ins().iconst(types::I64, 1);
        let boxed_bool = box_raw_bool_value(&mut builder, raw_bool, &nbc);
        def_var_from_boxed_transport(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut import_refs,
            &vars,
            &representation_plan,
            &nbc,
            "bool_home",
            boxed_bool,
        );

        let boxed_int = builder.ins().iconst(types::I64, nbc.qnan_tag_int | 7);
        def_var_from_boxed_transport(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut import_refs,
            &vars,
            &representation_plan,
            &nbc,
            "int_home",
            boxed_int,
        );

        let raw_int = builder.use_var(int_var);
        builder.ins().return_(&[raw_int]);
        builder.finalize();
    }

    let flags = settings::Flags::new(settings::builder());
    verify_function(&func, &flags)
        .expect("boxed transport must define scalar-primary homes with matching CLIF types");
}

#[test]
fn numeric_result_binding_converts_boxed_call_result_for_float_primary_home() {
    let mut backend = SimpleBackend::new();
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::F64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut func, &mut context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let float_var = builder.declare_var(types::F64);
        let mut vars = BTreeMap::new();
        vars.insert("float_home".to_string(), float_var);

        let representation_plan = scalar_transport_plan_for_float_home();
        let mut import_refs = BTreeMap::new();
        let nbc = crate::NanBoxConsts::new();

        let boxed_float = builder.ins().iconst(types::I64, 1.25f64.to_bits() as i64);
        def_var_from_numeric_result(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut import_refs,
            &vars,
            &representation_plan,
            &nbc,
            "float_home",
            boxed_float,
        );

        let raw_f64 = builder.use_var(float_var);
        builder.ins().return_(&[raw_f64]);
        builder.finalize();
    }

    let flags = settings::Flags::new(settings::builder());
    verify_function(&func, &flags)
        .expect("boxed call result must bind to float-primary homes as raw f64");
}

#[test]
fn raw_int_div_intmin_debug_guard_precedes_raw_sdiv() {
    // Regression for the finding-#17 UB class: the raw-i64 floordiv/mod fast
    // lanes emit a bare `sdiv`/`srem` on FULL-RANGE carrier values, and Cranelift
    // traps on `i64::MIN / -1` (it does not wrap). The lanes are only reached for
    // inline-47-proven operands (INT_MIN impossible) — a NON-LOCAL invariant — so
    // `emit_raw_int_div_intmin_debug_guard` makes that invariant locally CHECKED:
    // in debug builds it must emit a `trapnz` guarding `(i64::MIN, -1)` ahead of
    // the raw division; in release builds it must lower to nothing (no hot-path
    // cost). If a refactor drops the guard, this test fails.
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::user(0, 42), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut func, &mut context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let lhs_raw = builder.block_params(entry)[0];
        let rhs_raw = builder.block_params(entry)[1];
        emit_raw_int_div_intmin_debug_guard(&mut builder, lhs_raw, rhs_raw);
        // The guard must precede a raw division that would otherwise trap.
        let quot = builder.ins().sdiv(lhs_raw, rhs_raw);
        builder.ins().return_(&[quot]);
        builder.finalize();
    }

    let flags = settings::Flags::new(settings::builder());
    verify_function(&func, &flags)
        .expect("raw-int div INT_MIN/-1 debug guard must produce a verifiable CFG");
    let clif = func.display().to_string();
    if cfg!(debug_assertions) {
        assert!(
            clif.contains("trapnz"),
            "debug builds must emit a `trapnz` guarding INT_MIN/-1 ahead of the raw \
             sdiv/srem so the non-local inline-47 invariant is checked, not assumed:\n{clif}"
        );
    } else {
        assert!(
            !clif.contains("trapnz"),
            "release builds must not pay for the debug-only INT_MIN/-1 guard:\n{clif}"
        );
    }
}

#[test]
fn semantic_type_hint_does_not_create_native_scalar_lane_for_generic_ops() {
    let hinted_generic_op = OpIR {
        kind: "call_indirect".to_string(),
        args: Some(vec!["callable".to_string(), "args".to_string()]),
        out: Some("result".to_string()),
        type_hint: Some("int".to_string()),
        ..OpIR::default()
    };

    let func = FunctionIR {
        name: "hinted_generic".to_string(),
        params: vec!["callable".to_string(), "args".to_string()],
        ops: vec![hinted_generic_op],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };

    let plan = native_representation_plan_for_test(&func);

    assert!(
        !plan.name_has_scalar_kind("result", ScalarKind::Int),
        "representation plan must keep generic runtime results boxed even when type_hint=int",
    );
}
