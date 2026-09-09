use super::*;

#[test]
fn constants_are_proven() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(42)),
        make_op(
            OpCode::ConstFloat,
            vec![],
            vec![ValueId(1)],
            float_attr(2.5),
        ),
        make_op(
            OpCode::ConstStr,
            vec![],
            vec![ValueId(2)],
            str_attr("hello"),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    refine_types(&mut func);
    let proven = extract_proven_map(&func);

    assert_eq!(proven.get(&ValueId(0)), Some(&TirType::I64));
    assert_eq!(proven.get(&ValueId(1)), Some(&TirType::F64));
    assert_eq!(proven.get(&ValueId(2)), Some(&TirType::Str));
}

// ---- Test: Arithmetic on proven values is proven ----
#[test]
fn arithmetic_on_proven_is_proven() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
        make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    refine_types(&mut func);
    let proven = extract_proven_map(&func);

    assert_eq!(proven.get(&ValueId(2)), Some(&TirType::I64));
}

// ---- Test: parse_guard_type handles various type strings ----
