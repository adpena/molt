use super::*;

fn make_type_guard_op(operand: ValueId, result: ValueId, expected_type: &str) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("expected_type".into(), AttrValue::Str(expected_type.into()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::TypeGuard,
        operands: vec![operand],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

// ---- Test: TypeGuard result gets proven type ----
#[test]
fn typeguard_result_gets_proven_type() {
    // TypeGuard(%x, "int") -> %ok should type %ok as I64.
    let ops = vec![make_type_guard_op(ValueId(0), ValueId(1), "int")];
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![TirValue {
            id: ValueId(0),
            ty: TirType::DynBox,
        }],
        ops,
        terminator: Terminator::Return {
            values: vec![ValueId(1)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, block);
    let mut func = TirFunction {
        name: "guard_test".into(),
        param_names: vec!["x".into()],
        param_types: vec![TirType::DynBox],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry_id,
        next_value: 2,
        next_block: 1,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    let refined = refine_types(&mut func);
    assert!(refined >= 1, "TypeGuard should refine at least the result");

    let type_map = extract_type_map(&func);
    assert_eq!(
        type_map.get(&ValueId(1)),
        Some(&TirType::I64),
        "TypeGuard result should be I64"
    );
}

// ---- Test: Guard propagates to dominated blocks via CondBranch ----
#[test]
fn guard_propagates_to_dominated_blocks() {
    // bb0: %x = param(DynBox); %ok = TypeGuard(%x, "int"); CondBranch(%ok, bb1, bb2)
    // bb1 (success): Add(%x, %x) -> should know %x is I64
    // bb2 (fail): return
    let entry_id = BlockId(0);
    let success_id = BlockId(1);
    let fail_id = BlockId(2);

    let mut blocks = HashMap::new();

    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::DynBox,
            }],
            ops: vec![make_type_guard_op(ValueId(0), ValueId(1), "int")],
            terminator: Terminator::CondBranch {
                cond: ValueId(1),
                then_block: success_id,
                then_args: vec![],
                else_block: fail_id,
                else_args: vec![],
            },
        },
    );

    blocks.insert(
        success_id,
        TirBlock {
            id: success_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::Add,
                vec![ValueId(0), ValueId(0)],
                vec![ValueId(2)],
                AttrDict::new(),
            )],
            terminator: Terminator::Return {
                values: vec![ValueId(2)],
            },
        },
    );

    blocks.insert(
        fail_id,
        TirBlock {
            id: fail_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut func = TirFunction {
        name: "guard_prop_test".into(),
        param_names: vec!["x".into()],
        param_types: vec![TirType::DynBox],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry_id,
        next_value: 3,
        next_block: 3,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    refine_types(&mut func);
    let proven = extract_proven_map(&func);

    // The TypeGuard result should be proven I64.
    assert_eq!(
        proven.get(&ValueId(1)),
        Some(&TirType::I64),
        "TypeGuard result should be proven I64"
    );

    // The guarded value should be proven I64 (used in dominated block).
    assert_eq!(
        proven.get(&ValueId(0)),
        Some(&TirType::I64),
        "Guarded value should be proven I64 in dominated blocks"
    );
}

// ---- Test: Constants are always proven ----

#[test]
fn parse_guard_type_variants() {
    let cases = vec![
        ("int", TirType::I64),
        ("INT", TirType::I64),
        ("i64", TirType::I64),
        ("float", TirType::F64),
        ("f64", TirType::F64),
        ("bool", TirType::Bool),
        ("str", TirType::Str),
        ("string", TirType::Str),
        ("none", TirType::None),
        ("NoneType", TirType::None),
        ("bytes", TirType::Bytes),
        ("bigint", TirType::BigInt),
    ];
    for (input, expected) in cases {
        let mut attrs = AttrDict::new();
        attrs.insert("expected_type".into(), AttrValue::Str(input.into()));
        assert_eq!(
            parse_guard_type(&attrs),
            Some(expected),
            "parse_guard_type({:?}) mismatch",
            input
        );
    }
}

// ---- Test: parse_guard_type returns None for unknown types ----
#[test]
fn parse_guard_type_unknown() {
    let mut attrs = AttrDict::new();
    attrs.insert(
        "expected_type".into(),
        AttrValue::Str("SomeCustomClass".into()),
    );
    assert_eq!(parse_guard_type(&attrs), None);
}

// ---- Test: TypeGuard with "ty" attr (used by type_guard_hoist) ----
#[test]
fn typeguard_ty_attr_works() {
    let mut attrs = AttrDict::new();
    attrs.insert("ty".into(), AttrValue::Str("INT".into()));
    assert_eq!(parse_guard_type(&attrs), Some(TirType::I64));
}
