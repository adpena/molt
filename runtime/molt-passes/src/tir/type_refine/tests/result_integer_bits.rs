use super::*;

const BINARY_OPS: [OpCode; 5] = [
    OpCode::BitAnd,
    OpCode::BitOr,
    OpCode::BitXor,
    OpCode::Shl,
    OpCode::Shr,
];

fn integer_constant(id: u32, ty: &TirType) -> TirOp {
    let (opcode, attrs) = match ty {
        TirType::I64 => (OpCode::ConstInt, int_attr(5)),
        TirType::Bool => (
            OpCode::ConstBool,
            AttrDict::from([("value".into(), AttrValue::Bool(true))]),
        ),
        TirType::BigInt => (
            OpCode::ConstBigInt,
            AttrDict::from([(
                "s_value".into(),
                AttrValue::Str("18446744073709551616".into()),
            )]),
        ),
        _ => panic!("not an integer fixture"),
    };
    make_op(opcode, vec![], vec![ValueId(id)], attrs)
}

#[test]
fn integer_bitwise_result_family_preserves_python_type_without_raw_storage_proof() {
    use crate::repr::Repr;

    for opcode in BINARY_OPS {
        for lhs in [TirType::Bool, TirType::I64, TirType::BigInt] {
            for rhs in [TirType::Bool, TirType::I64, TirType::BigInt] {
                let expected = if lhs == TirType::Bool
                    && rhs == TirType::Bool
                    && matches!(opcode, OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor)
                {
                    TirType::Bool
                } else {
                    TirType::I64
                };
                let mut func = single_block_func(
                    vec![
                        integer_constant(0, &lhs),
                        integer_constant(1, &rhs),
                        make_op(
                            opcode,
                            vec![ValueId(0), ValueId(1)],
                            vec![ValueId(2)],
                            AttrDict::new(),
                        ),
                        make_op(
                            OpCode::Copy,
                            vec![ValueId(2)],
                            vec![ValueId(3)],
                            AttrDict::new(),
                        ),
                    ],
                    4,
                );
                assert_eq!(
                    infer_result_types_with_attrs(opcode, &[lhs.clone(), rhs.clone()], None, 1),
                    vec![Some(expected.clone())],
                    "{opcode:?}, {lhs:?}, {rhs:?}",
                );
                for refined in [false, true] {
                    if refined {
                        refine_types(&mut func);
                    }
                    let exact = extract_exact_scalar_map(&func);
                    let reprs = crate::representation_facts::repr_by_value_for(&func, None);
                    for id in [ValueId(2), ValueId(3)] {
                        assert_eq!(exact[&id], expected, "{opcode:?}, {lhs:?}, {rhs:?}");
                        assert_eq!(
                            reprs[&id],
                            if expected == TirType::Bool {
                                Repr::Bool
                            } else {
                                Repr::MaybeBigInt
                            },
                            "{opcode:?}, {lhs:?}, {rhs:?}, refined={refined}",
                        );
                    }
                }
            }
        }
    }
    for ty in [TirType::Bool, TirType::I64, TirType::BigInt] {
        let func = single_block_func(
            vec![
                integer_constant(0, &ty),
                make_op(
                    OpCode::BitNot,
                    vec![ValueId(0)],
                    vec![ValueId(1)],
                    AttrDict::new(),
                ),
            ],
            2,
        );
        assert_eq!(extract_exact_scalar_map(&func)[&ValueId(1)], TirType::I64);
        assert_eq!(
            crate::representation_facts::repr_by_value_for(&func, None)[&ValueId(1)],
            Repr::MaybeBigInt,
        );
    }
}

#[test]
fn integer_bitwise_result_rules_reject_unknown_domains_and_invalid_shapes() {
    for opcode in BINARY_OPS.into_iter().chain([OpCode::BitNot]) {
        let arity = if opcode == OpCode::BitNot { 1 } else { 2 };
        for count in (0..=3).filter(|count| *count != arity) {
            let operands = vec![TirType::I64; count];
            assert_eq!(
                infer_result_types_with_attrs(opcode, &operands, None, 1),
                vec![None]
            );
        }
        let operands = vec![TirType::I64; arity];
        for result_count in [0, 2] {
            assert_eq!(
                infer_result_types_with_attrs(opcode, &operands, None, result_count),
                vec![None; result_count],
            );
        }
        for (unknown, expected) in [
            (TirType::DynBox, None),
            (TirType::Never, Some(TirType::Never)),
            (TirType::F64, None),
            (TirType::UserClass("m.IntSubclass".into()), None),
        ] {
            for position in 0..arity {
                let mut operands = operands.clone();
                operands[position] = unknown.clone();
                assert_eq!(
                    infer_result_types_with_attrs(opcode, &operands, None, 1),
                    vec![expected.clone()],
                    "{opcode:?}, {operands:?}",
                );
            }
        }
        // An int/bool annotation (or a persisted result hint) is not an exact
        // built-in producer and cannot authorize result or physical facts.
        for annotation in [TirType::I64, TirType::Bool, TirType::BigInt] {
            let mut func = single_block_func(
                vec![
                    integer_constant(1, &TirType::I64),
                    make_op(
                        opcode,
                        (0..arity as u32).map(ValueId).collect(),
                        vec![ValueId(2)],
                        AttrDict::from([("return_type".into(), AttrValue::Str("int".into()))]),
                    ),
                ],
                3,
            );
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .args
                .push(TirValue {
                    id: ValueId(0),
                    ty: annotation.clone(),
                });
            func.value_types.insert(ValueId(0), annotation);
            func.value_types.insert(ValueId(2), TirType::I64);
            for refined in [false, true] {
                if refined {
                    refine_types(&mut func);
                }
                assert!(!extract_exact_scalar_map(&func).contains_key(&ValueId(2)));
                let range = crate::representation_facts::value_range_for(&func);
                let repr = crate::representation_facts::repr_by_value_for(&func, Some(&range))
                    [&ValueId(2)];
                assert!(!repr.is_raw_i64_carrier() && !repr.is_bool_carrier());
            }
        }
    }
}

#[test]
fn shift_integer_result_provenance_does_not_elide_count_or_magnitude_guards() {
    use crate::repr::Repr;

    for opcode in [OpCode::Shl, OpCode::Shr] {
        for (lhs, count, raw) in [
            (5, 3, true),
            (0, 63, true),
            (0, 64, false),
            (0, 70, false),
            (1, -1, false),
        ] {
            let mut func = single_block_func(
                vec![
                    make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(lhs)),
                    make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(count)),
                    make_op(
                        opcode,
                        vec![ValueId(0), ValueId(1)],
                        vec![ValueId(2)],
                        AttrDict::new(),
                    ),
                    make_op(OpCode::CheckException, vec![], vec![], AttrDict::new()),
                ],
                3,
            );
            assert_eq!(extract_exact_scalar_map(&func)[&ValueId(2)], TirType::I64);
            let range = crate::representation_facts::value_range_for(&func);
            let repr =
                crate::representation_facts::repr_by_value_for(&func, Some(&range))[&ValueId(2)];
            assert_eq!(repr == Repr::RawI64Safe, raw, "{opcode:?}: {lhs}, {count}");
            if count < 0 {
                crate::tir::passes::check_exception_elim::run(&mut func);
                assert!(
                    func.blocks[&func.entry_block]
                        .ops
                        .iter()
                        .any(|op| op.opcode == OpCode::CheckException)
                );
                crate::tir::passes::dce::run(&mut func);
                assert!(
                    func.blocks[&func.entry_block]
                        .ops
                        .iter()
                        .any(|op| op.opcode == opcode)
                );
            }
        }
    }
    let func = single_block_func(
        vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(46)),
            make_op(
                OpCode::Shl,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ],
        3,
    );
    let range = crate::representation_facts::value_range_for(&func);
    assert_eq!(extract_exact_scalar_map(&func)[&ValueId(2)], TirType::I64);
    assert_eq!(
        crate::representation_facts::repr_by_value_for(&func, Some(&range))[&ValueId(2)],
        Repr::MaybeBigInt,
    );
}
