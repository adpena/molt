use super::*;

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
        store_index_fallback_import_name(&dict_store_plan, &dict_store, false),
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
        store_index_fallback_import_name(&store_plan, &transport_store, false),
        "molt_store_index"
    );
}

#[test]
fn native_generic_list_inlining_uses_tir_container_facts() {
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
        let nbc = crate::NanBoxConsts::new(&mut builder);
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

        let nbc = crate::NanBoxConsts::new(&mut builder);
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
        let nbc = crate::NanBoxConsts::new(&mut builder);

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
        let nbc = crate::NanBoxConsts::new(&mut builder);

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
    };

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(
        !plan.name_has_scalar_kind("result", ScalarKind::Int),
        "representation plan must keep generic runtime results boxed even when type_hint=int",
    );
}
