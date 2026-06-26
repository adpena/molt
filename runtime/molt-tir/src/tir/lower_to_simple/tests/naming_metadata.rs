use super::*;

#[test]
fn value_var_naming() {
    assert_eq!(value_var(ValueId(0)), "_v0");
    assert_eq!(value_var(ValueId(42)), "_v42");
}

#[test]
fn simple_value_names_use_entry_param_names() {
    let mut func = TirFunction::new(
        "params".into(),
        vec![TirType::I64, TirType::Bool],
        TirType::DynBox,
    );
    func.param_names = vec!["lhs".into(), "flag".into()];

    let names = SimpleValueNames::for_function(&func);

    assert_eq!(names.value_name(ValueId(0)), "lhs");
    assert_eq!(names.value_name(ValueId(1)), "flag");
    assert_eq!(names.value_name(ValueId(99)), "_v99");
}

#[test]
fn simple_value_names_record_block_arg_slots_without_shadowing_values() {
    let mut func = TirFunction::new("join".into(), vec![TirType::I64], TirType::I64);
    let join = func.fresh_block();
    let arg_id = func.fresh_value();
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: arg_id,
                ty: TirType::I64,
            }],
            ops: Vec::new(),
            terminator: Terminator::Unreachable,
        },
    );

    let names = SimpleValueNames::for_function(&func);

    assert_eq!(names.block_arg_slot(join, 0), "_bb1_arg0");
    assert_eq!(names.block_arg_slots(join, 1), vec!["_bb1_arg0"]);
    assert_eq!(
        names.value_name(arg_id),
        SimpleValueNames::canonical_value_name(arg_id),
        "block argument storage slots are separate from SSA value names",
    );
}

/// Regression: the TIR inliner mints fresh `ValueId`s, so a value's CANONICAL
/// name (`_v{id}`) can land on a string a DIFFERENT value already claimed via
/// an explicit `_simple_out` override (carried verbatim from the pre-lift
/// stream). Two distinct values must NOT resolve to the same SimpleIR name —
/// otherwise `rewrite_copy_aliases` conflates them (observed: a module-scope
/// guarded property merge read the cold slow path on its hot fast edge). The
/// override is authoritative; the colliding canonical value gets a fresh,
/// unique name.
#[test]
fn canonical_name_collision_with_override_is_resolved() {
    // Value A = ValueId(2), no override -> wants canonical "_v2".
    // Value B = ValueId(5), op carries `_simple_out: "_v2"` (a stale stream
    // name from before the re-lift renumbered ids). B keeps "_v2".
    let mut func = TirFunction::new("collide".into(), vec![], TirType::DynBox);
    let a = ValueId(2);
    let b = ValueId(5);

    let mut b_attrs = AttrDict::new();
    b_attrs.insert("_simple_out".into(), AttrValue::Str("_v2".into()));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    // A is a const result (canonical-named).
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![a],
        attrs: AttrDict::new(),
        source_span: None,
    });
    // B carries the explicit override "_v2".
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![b],
        attrs: b_attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![b] };

    let names = SimpleValueNames::for_function(&func);

    assert_eq!(
        names.value_name(b),
        "_v2",
        "the explicit `_simple_out` override is authoritative"
    );
    assert_ne!(
        names.value_name(a),
        names.value_name(b),
        "two distinct values must never resolve to the same SimpleIR name"
    );
    assert_ne!(
        names.value_name(a),
        "_v2",
        "the canonical-name value whose name collided with an override must \
             be renamed to a fresh, collision-free name"
    );
}

/// Verify that typed TIR does not re-emit integer transport hints.
#[test]
fn type_propagation_does_not_emit_fast_int_on_arithmetic() {
    use crate::tir::type_refine::{extract_type_map, refine_types};

    // Build: func @add_ints() -> I64
    //   %0 = const_int 10
    //   %1 = const_int 20
    //   %2 = add %0, %1
    //   return %2
    let mut func = TirFunction::new("add_ints".into(), vec![], TirType::I64);

    let v0 = ValueId(func.next_value);
    func.next_value += 1;
    let v1 = ValueId(func.next_value);
    func.next_value += 1;
    let v2 = ValueId(func.next_value);
    func.next_value += 1;

    let mut attrs0 = AttrDict::new();
    attrs0.insert("value".into(), AttrValue::Int(10));
    let mut attrs1 = AttrDict::new();
    attrs1.insert("value".into(), AttrValue::Int(20));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v0],
        attrs: attrs0,
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v1],
        attrs: attrs1,
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![v0, v1],
        results: vec![v2],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![v2] };

    // Run type refinement.
    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    // Verify the type map has I64 for all three values.
    assert_eq!(type_map.get(&v0), Some(&TirType::I64), "v0 should be I64");
    assert_eq!(type_map.get(&v1), Some(&TirType::I64), "v1 should be I64");
    assert_eq!(
        type_map.get(&v2),
        Some(&TirType::I64),
        "v2 should be I64 (add of two I64s)"
    );

    let ops = lower_to_simple_ir(&func);
    let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
    assert!(!add_ops.is_empty(), "expected an 'add' op in output");
    for add_op in &add_ops {
        assert!(
            add_op.fast_int.is_none(),
            "typed TIR must not emit fast_int transport hints: {:?}",
            add_op
        );
    }
}

/// Verify that typed TIR does not re-emit float transport hints.
#[test]
fn type_propagation_does_not_emit_fast_float_on_float_arithmetic() {
    use crate::tir::type_refine::refine_types;

    let mut func = TirFunction::new("add_floats".into(), vec![], TirType::F64);

    let v0 = ValueId(func.next_value);
    func.next_value += 1;
    let v1 = ValueId(func.next_value);
    func.next_value += 1;
    let v2 = ValueId(func.next_value);
    func.next_value += 1;

    let mut attrs0 = AttrDict::new();
    attrs0.insert("f_value".into(), AttrValue::Float(1.5));
    let mut attrs1 = AttrDict::new();
    attrs1.insert("f_value".into(), AttrValue::Float(2.5));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstFloat,
        operands: vec![],
        results: vec![v0],
        attrs: attrs0,
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstFloat,
        operands: vec![],
        results: vec![v1],
        attrs: attrs1,
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![v0, v1],
        results: vec![v2],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![v2] };

    refine_types(&mut func);
    let ops = lower_to_simple_ir(&func);

    let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
    assert!(!add_ops.is_empty());
    for add_op in &add_ops {
        assert!(
            add_op.fast_float.is_none(),
            "typed TIR must not emit fast_float transport hints: {:?}",
            add_op
        );
    }
}

/// Verify that typed TIR does not re-emit bool type hints.
#[test]
fn type_propagation_does_not_emit_type_hint_for_bool() {
    use crate::tir::type_refine::refine_types;

    let mut func = TirFunction::new("cmp".into(), vec![], TirType::Bool);

    let v0 = ValueId(func.next_value);
    func.next_value += 1;
    let v1 = ValueId(func.next_value);
    func.next_value += 1;
    let v2 = ValueId(func.next_value);
    func.next_value += 1;

    let mut attrs0 = AttrDict::new();
    attrs0.insert("value".into(), AttrValue::Int(1));
    let mut attrs1 = AttrDict::new();
    attrs1.insert("value".into(), AttrValue::Int(2));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v0],
        attrs: attrs0,
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v1],
        attrs: attrs1,
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Eq,
        operands: vec![v0, v1],
        results: vec![v2],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![v2] };

    refine_types(&mut func);
    let ops = lower_to_simple_ir(&func);

    let eq_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "eq").collect();
    assert!(!eq_ops.is_empty());
    for eq_op in &eq_ops {
        assert!(
            eq_op.type_hint.is_none(),
            "typed TIR must not emit bool type_hint metadata: {:?}",
            eq_op
        );
        assert!(
            eq_op.fast_float.is_none(),
            "bool op should not have fast_float"
        );
        assert!(
            eq_op.fast_int.is_none(),
            "comparison op should not carry fast_int metadata"
        );
    }
}

#[test]
fn type_propagation_does_not_emit_scalar_type_hint_for_call_result() {
    let mut func = TirFunction::new("call_result".into(), vec![], TirType::I64);

    let result = ValueId(func.next_value);
    func.next_value += 1;

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallMethod,
        operands: vec![],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let ops = lower_to_simple_ir(&func);

    let call_ops: Vec<&OpIR> = ops.iter().filter(|op| op.kind == "call_method").collect();
    assert!(!call_ops.is_empty());
    for call_op in &call_ops {
        assert!(
            call_op.type_hint.is_none(),
            "typed TIR must not backfill scalar type_hint metadata for opaque calls: {:?}",
            call_op
        );
    }
}

#[test]
fn tir_round_trip_preserves_method_ic_as_first_class_ops() {
    let func_ir = FunctionIR {
        name: "method_ic_roundtrip".into(),
        params: vec![
            "recv".into(),
            "arg".into(),
            "class".into(),
            "self_obj".into(),
        ],
        ops: vec![
            OpIR {
                kind: "call_method_ic".into(),
                args: Some(vec!["recv".into(), "arg".into()]),
                s_value: Some("f".into()),
                out: Some("r".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "call_super_method_ic".into(),
                args: Some(vec!["class".into(), "self_obj".into(), "arg".into()]),
                s_value: Some("g".into()),
                out: Some("s".into()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let tir_ops: Vec<&TirOp> = tir_func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .collect();
    let method_op = tir_ops
        .iter()
        .copied()
        .find(|op| op.opcode == OpCode::CallMethodIc)
        .expect("call_method_ic must lower to a first-class opcode");
    assert!(
        !method_op.attrs.contains_key("_original_kind"),
        "call_method_ic must not remain a Copy/_original_kind bridge"
    );
    assert_eq!(
        method_op.attrs.get("method"),
        Some(&AttrValue::Str("f".into()))
    );
    let super_op = tir_ops
        .iter()
        .copied()
        .find(|op| op.opcode == OpCode::CallSuperMethodIc)
        .expect("call_super_method_ic must lower to a first-class opcode");
    assert!(
        !super_op.attrs.contains_key("_original_kind"),
        "call_super_method_ic must not remain a Copy/_original_kind bridge"
    );
    assert_eq!(
        super_op.attrs.get("method"),
        Some(&AttrValue::Str("g".into()))
    );

    let round_tripped = lower_to_simple_ir(&tir_func);
    let method = round_tripped
        .iter()
        .find(|op| op.kind == "call_method_ic")
        .expect("round-trip should re-emit call_method_ic");
    assert_eq!(method.s_value.as_deref(), Some("f"));
    assert_eq!(
        method.args.as_ref().expect("call_method_ic args"),
        &vec!["recv".to_string(), "arg".to_string()]
    );
    assert_eq!(method.out.as_deref(), Some("r"));

    let super_method = round_tripped
        .iter()
        .find(|op| op.kind == "call_super_method_ic")
        .expect("round-trip should re-emit call_super_method_ic");
    assert_eq!(super_method.s_value.as_deref(), Some("g"));
    assert_eq!(
        super_method
            .args
            .as_ref()
            .expect("call_super_method_ic args"),
        &vec![
            "class".to_string(),
            "self_obj".to_string(),
            "arg".to_string()
        ]
    );
    assert_eq!(super_method.out.as_deref(), Some("s"));
}

/// Verify that no type map (empty) means no flags are set.
#[test]
fn empty_type_map_sets_no_flags() {
    let func = add_function();
    let ops = lower_to_simple_ir(&func);
    let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
    assert!(!add_ops.is_empty());
    for add_op in &add_ops {
        assert!(
            add_op.fast_int.is_none(),
            "empty type map should not set fast_int"
        );
        assert!(
            add_op.fast_float.is_none(),
            "empty type map should not set fast_float"
        );
        assert!(
            add_op.type_hint.is_none(),
            "empty type map should not set type_hint"
        );
    }
}

#[test]
fn tir_round_trip_preserves_guarded_field_set_offset() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    let func_ir = FunctionIR {
        name: "guarded_store".into(),
        params: vec![
            "obj".into(),
            "class_bits".into(),
            "expected".into(),
            "value".into(),
        ],
        ops: vec![OpIR {
            kind: "guarded_field_set".into(),
            args: Some(vec![
                "obj".into(),
                "class_bits".into(),
                "expected".into(),
                "value".into(),
            ]),
            s_value: Some("x".into()),
            value: Some(24),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let round_tripped = lower_to_simple_ir(&tir_func);
    let store_op = round_tripped
        .iter()
        .find(|op| op.kind == "guarded_field_set")
        .expect("expected guarded_field_set after TIR round-trip");

    assert_eq!(store_op.s_value.as_deref(), Some("x"));
    assert_eq!(store_op.value, Some(24));
}

#[test]
fn tir_round_trip_preserves_guarded_field_init_offset() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    let func_ir = FunctionIR {
        name: "guarded_init".into(),
        params: vec![
            "obj".into(),
            "class_bits".into(),
            "expected".into(),
            "value".into(),
        ],
        ops: vec![OpIR {
            kind: "guarded_field_init".into(),
            args: Some(vec![
                "obj".into(),
                "class_bits".into(),
                "expected".into(),
                "value".into(),
            ]),
            s_value: Some("x".into()),
            value: Some(24),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let round_tripped = lower_to_simple_ir(&tir_func);
    let init_op = round_tripped
        .iter()
        .find(|op| op.kind == "guarded_field_init")
        .expect("expected guarded_field_init after TIR round-trip");

    assert_eq!(init_op.s_value.as_deref(), Some("x"));
    assert_eq!(init_op.value, Some(24));
}

#[test]
fn tir_round_trip_preserves_guarded_field_get_offset() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    let func_ir = FunctionIR {
        name: "guarded_load".into(),
        params: vec!["obj".into(), "class_bits".into(), "expected".into()],
        ops: vec![OpIR {
            kind: "guarded_field_get".into(),
            args: Some(vec!["obj".into(), "class_bits".into(), "expected".into()]),
            s_value: Some("x".into()),
            value: Some(24),
            out: Some("loaded".into()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let round_tripped = lower_to_simple_ir(&tir_func);
    let load_op = round_tripped
        .iter()
        .find(|op| op.kind == "guarded_field_get")
        .expect("expected guarded_field_get after TIR round-trip");

    assert_eq!(load_op.s_value.as_deref(), Some("x"));
    assert_eq!(load_op.value, Some(24));
    assert!(
        load_op.out.is_some(),
        "guarded_field_get must preserve an output"
    );
}

#[test]
fn tir_round_trip_preserves_guarded_field_init_metadata() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    let func_ir = FunctionIR {
        name: "guarded_init".into(),
        params: vec![
            "obj".into(),
            "class_bits".into(),
            "expected".into(),
            "value".into(),
        ],
        ops: vec![OpIR {
            kind: "guarded_field_init".into(),
            args: Some(vec![
                "obj".into(),
                "class_bits".into(),
                "expected".into(),
                "value".into(),
            ]),
            s_value: Some("x".into()),
            value: Some(24),
            out: Some("init_result".into()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let round_tripped = lower_to_simple_ir(&tir_func);
    let init_op = round_tripped
        .iter()
        .find(|op| op.kind == "guarded_field_init")
        .expect("expected guarded_field_init after TIR round-trip");

    assert_eq!(init_op.s_value.as_deref(), Some("x"));
    assert_eq!(init_op.value, Some(24));
    assert_eq!(init_op.out.as_deref(), Some("init_result"));
}

#[test]
fn tir_round_trip_preserves_call_async_metadata() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    let func_ir = FunctionIR {
        name: "async_call".into(),
        params: vec!["delay".into(), "result".into()],
        ops: vec![OpIR {
            kind: "call_async".into(),
            args: Some(vec!["delay".into(), "result".into()]),
            s_value: Some("molt_async_sleep".into()),
            out: Some("future".into()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let round_tripped = lower_to_simple_ir(&tir_func);
    let call_op = round_tripped
        .iter()
        .find(|op| op.kind == "call_async")
        .expect("expected call_async after TIR round-trip");

    assert_eq!(call_op.s_value.as_deref(), Some("molt_async_sleep"));
    let args = call_op.args.as_ref().expect("call_async args");
    assert_eq!(args, &vec!["delay".to_string(), "result".to_string()]);
    assert!(
        call_op.out.is_some(),
        "call_async must preserve its future output"
    );
}

#[test]
fn tir_round_trip_preserves_typed_field_class_identity() {
    // The `class` field on typed-slot field ops (the S5-1.5 alias-region
    // authority) must survive the SimpleIR↔TIR roundtrip on both load and
    // store spellings; otherwise the alias oracle's `TypedField` region would
    // collapse to `GenericHeap` after the first roundtrip.
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    let func_ir = FunctionIR {
        name: "field_class".into(),
        params: vec![
            "obj".into(),
            "class_bits".into(),
            "expected".into(),
            "value".into(),
        ],
        ops: vec![
            OpIR {
                kind: "guarded_field_get".into(),
                args: Some(vec!["obj".into(), "class_bits".into(), "expected".into()]),
                s_value: Some("x".into()),
                value: Some(24),
                out: Some("loaded".into()),
                class_name: Some("Point".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "guarded_field_set".into(),
                args: Some(vec![
                    "obj".into(),
                    "class_bits".into(),
                    "expected".into(),
                    "value".into(),
                ]),
                s_value: Some("x".into()),
                value: Some(24),
                class_name: Some("Point".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "guarded_field_init".into(),
                args: Some(vec![
                    "obj".into(),
                    "class_bits".into(),
                    "expected".into(),
                    "value".into(),
                ]),
                s_value: Some("x".into()),
                value: Some(24),
                class_name: Some("Point".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "load".into(),
                args: Some(vec!["obj".into()]),
                value: Some(8),
                out: Some("plain".into()),
                class_name: Some("Line".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store".into(),
                args: Some(vec!["obj".into(), "value".into()]),
                value: Some(8),
                class_name: Some("Line".into()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let round_tripped = lower_to_simple_ir(&tir_func);
    for kind in [
        "guarded_field_get",
        "guarded_field_set",
        "guarded_field_init",
    ] {
        let op = round_tripped
            .iter()
            .find(|op| op.kind == kind)
            .unwrap_or_else(|| panic!("{kind} missing after roundtrip"));
        assert_eq!(
            op.class_name.as_deref(),
            Some("Point"),
            "{kind} must preserve its `class` identity"
        );
    }
    for kind in ["load", "store"] {
        let op = round_tripped
            .iter()
            .find(|op| op.kind == kind)
            .unwrap_or_else(|| panic!("{kind} missing after roundtrip"));
        assert_eq!(
            op.class_name.as_deref(),
            Some("Line"),
            "{kind} must preserve its `class` identity"
        );
    }
}

#[test]
fn tir_round_trip_preserves_fused_iter_next_output_names() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    let func_ir = FunctionIR {
        name: "iter_next_names".into(),
        params: vec!["items".into()],
        ops: vec![
            OpIR {
                kind: "iter".into(),
                args: Some(vec!["items".into()]),
                out: Some("iter_obj".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "iter_next".into(),
                args: Some(vec!["iter_obj".into()]),
                out: Some("pair".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(1),
                out: Some("done_index".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".into(),
                args: Some(vec!["pair".into(), "done_index".into()]),
                out: Some("done_flag".into()),
                fast_int: Some(true),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("value_index".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".into(),
                args: Some(vec!["pair".into(), "value_index".into()]),
                out: Some("next_value".into()),
                fast_int: Some(true),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let round_tripped = lower_to_simple_ir(&tir_func);
    let fused = round_tripped
        .iter()
        .find(|op| op.kind == "iter_next_unboxed")
        .expect("expected iter_next pattern to fuse");

    assert_eq!(fused.var.as_deref(), Some("next_value"));
    assert_eq!(fused.out.as_deref(), Some("done_flag"));

    let relowered = lower_to_tir(&FunctionIR {
        name: "roundtrip_iter_next_relower".into(),
        params: func_ir.params,
        ops: round_tripped,
        param_types: None,
        source_file: None,
        is_extern: false,
    });
    let relowered_op = relowered
        .blocks
        .values()
        .flat_map(|block| block.ops.iter())
        .find(|op| op.opcode == OpCode::IterNextUnboxed)
        .expect("round-tripped iter_next_unboxed must relower canonically");
    assert_eq!(relowered_op.operands.len(), 1);
    assert_eq!(relowered_op.results.len(), 2);
}

#[test]
fn tir_round_trip_preserves_method_guarded_field_set_sequence() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;
    use crate::tir::passes::run_pipeline;
    use crate::tir::type_refine::refine_types;

    let func_ir = FunctionIR {
        name: "method_trace__C_f".into(),
        params: vec!["self".into()],
        ops: vec![
            OpIR {
                kind: "exception_stack_enter".into(),
                out: Some("v88".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_depth".into(),
                out: Some("v89".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("self".into()),
                args: Some(vec!["self".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".into(),
                value: Some(3),
                col_offset: Some(8),
                end_col_offset: Some(18),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(1),
                out: Some("v90".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("C".into()),
                out: Some("v91".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("method_trace".into()),
                out: Some("v92".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_get".into(),
                args: Some(vec!["v92".into()]),
                out: Some("v93".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_attr".into(),
                args: Some(vec!["v93".into(), "v91".into()]),
                out: Some("v94".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(3),
                out: Some("v95".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "guarded_field_set".into(),
                args: Some(vec![
                    "self".into(),
                    "v94".into(),
                    "v95".into(),
                    "v90".into(),
                ]),
                s_value: Some("x".into()),
                value: Some(0),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("v96".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                var: Some("v96".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".into(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_set_depth".into(),
                args: Some(vec!["v89".into()]),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_exit".into(),
                args: Some(vec!["v88".into()]),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["i64".into()]),
        source_file: None,
        is_extern: false,
    };

    let mut tir_func = lower_to_tir(&func_ir);
    refine_types(&mut tir_func);
    run_pipeline(
        &mut tir_func,
        &crate::tir::target_info::TargetInfo::native_release_fast(),
    );
    refine_types(&mut tir_func);
    let round_tripped = lower_to_simple_ir(&tir_func);

    let cache_get_idx = round_tripped
        .iter()
        .position(|op| op.kind == "module_cache_get")
        .expect("module_cache_get must survive TIR roundtrip");
    let module_get_idx = round_tripped
        .iter()
        .position(|op| op.kind == "module_get_attr")
        .expect("module_get_attr must survive TIR roundtrip");
    let field_set_idx = round_tripped
        .iter()
        .position(|op| op.kind == "guarded_field_set")
        .expect("guarded_field_set must survive TIR roundtrip");

    assert!(
        cache_get_idx < module_get_idx && module_get_idx < field_set_idx,
        "method guarded field set path must preserve class lookup ordering: {round_tripped:?}"
    );

    let producer_by_out: std::collections::HashMap<String, &OpIR> = round_tripped
        .iter()
        .filter_map(|op| op.out.as_ref().map(|out| (out.clone(), op)))
        .collect();

    let cache_get = &round_tripped[cache_get_idx];
    let cache_arg = cache_get
        .args
        .as_ref()
        .and_then(|args| args.first())
        .expect("module_cache_get must keep module-name operand");
    // Follow through Copy/copy chains to find the original const_str
    // (GVN may deduplicate identical constants, replacing the second
    // with a copy of the first).
    let mut cache_arg_name = cache_arg.clone();
    for _ in 0..10 {
        let op = producer_by_out
            .get(&cache_arg_name)
            .expect("module_cache_get operand must come from an op");
        if op.kind == "const_str" {
            assert_eq!(op.s_value.as_deref(), Some("method_trace"));
            break;
        }
        if op.kind == "copy" || op.kind == "copy_var" {
            cache_arg_name = op
                .args
                .as_ref()
                .and_then(|a| a.first().cloned())
                .unwrap_or_else(|| cache_arg_name.clone());
        } else {
            panic!(
                "expected const_str or copy, got {} for module_cache_get operand",
                op.kind
            );
        }
    }

    let class_lookup = &round_tripped[module_get_idx];
    let class_lookup_args = class_lookup
        .args
        .as_ref()
        .expect("module_get_attr must keep operands");
    assert_eq!(class_lookup_args.len(), 2);
    assert_eq!(class_lookup_args[0], cache_get.out.clone().unwrap());
    let class_name_op = producer_by_out
        .get(&class_lookup_args[1])
        .expect("module_get_attr class-name operand must come from an op");
    assert_eq!(class_name_op.kind, "const_str");
    assert_eq!(class_name_op.s_value.as_deref(), Some("C"));

    let field_set = &round_tripped[field_set_idx];
    let field_set_args = field_set
        .args
        .as_ref()
        .expect("guarded_field_set must keep operands");
    assert_eq!(field_set_args.len(), 4);
    let self_value_op = producer_by_out
        .get(&field_set_args[0])
        .expect("guarded_field_set receiver must come from an op");
    assert_eq!(self_value_op.kind, "copy_var");
    assert_eq!(self_value_op.var.as_deref(), Some("self"));
    assert_eq!(field_set_args[1], class_lookup.out.clone().unwrap());
    let expected_version_op = producer_by_out
        .get(&field_set_args[2])
        .expect("guarded_field_set version operand must come from an op");
    assert_eq!(expected_version_op.kind, "const");
    assert_eq!(expected_version_op.value, Some(3));
    let stored_value_op = producer_by_out
        .get(&field_set_args[3])
        .expect("guarded_field_set value operand must come from an op");
    assert_eq!(stored_value_op.kind, "const");
    assert_eq!(stored_value_op.value, Some(1));
    assert_eq!(field_set.s_value.as_deref(), Some("x"));
    assert_eq!(field_set.value, Some(0));

    let set_depth_idx = round_tripped
        .iter()
        .position(|op| op.kind == "exception_stack_set_depth")
        .expect("handler cleanup must preserve exception_stack_set_depth");
    let exit_idx = round_tripped
        .iter()
        .position(|op| op.kind == "exception_stack_exit")
        .expect("handler cleanup must preserve exception_stack_exit");
    let set_depth_arg = round_tripped[set_depth_idx]
        .args
        .as_ref()
        .and_then(|args| args.first())
        .expect("exception_stack_set_depth must keep its operand");
    let exit_arg = round_tripped[exit_idx]
        .args
        .as_ref()
        .and_then(|args| args.first())
        .expect("exception_stack_exit must keep its operand");
    let set_depth_arg_op = producer_by_out
        .get(set_depth_arg)
        .expect("exception_stack_set_depth operand must come from a load_var");
    let exit_arg_op = producer_by_out
        .get(exit_arg)
        .expect("exception_stack_exit operand must come from a load_var");
    assert_eq!(set_depth_arg_op.kind, "load_var");
    assert_eq!(exit_arg_op.kind, "load_var");
}
