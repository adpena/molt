use super::*;

const BINARY_CASES: [(OpCode, TirType); 5] = [
    (OpCode::BitAnd, TirType::Bool),
    (OpCode::BitOr, TirType::Bool),
    (OpCode::BitXor, TirType::Bool),
    (OpCode::Shl, TirType::I64),
    (OpCode::Shr, TirType::I64),
];

fn integer_constant(id: u32, ty: &TirType) -> TirOp {
    let (opcode, attrs) = match ty {
        TirType::I64 => (OpCode::ConstInt, int_attr(5)),
        TirType::Bool => (
            OpCode::ConstBool,
            AttrDict::from_iter([("value".into(), AttrValue::Bool(true))]),
        ),
        TirType::BigInt => (
            OpCode::ConstBigInt,
            AttrDict::from_iter([(
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

    for (opcode, bool_pair_result) in BINARY_CASES {
        for lhs in [TirType::Bool, TirType::I64, TirType::BigInt] {
            for rhs in [TirType::Bool, TirType::I64, TirType::BigInt] {
                let expected = if lhs == TirType::Bool && rhs == TirType::Bool {
                    bool_pair_result.clone()
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
                            AttrDict::default(),
                        ),
                        make_op(
                            OpCode::Copy,
                            vec![ValueId(2)],
                            vec![ValueId(3)],
                            AttrDict::default(),
                        ),
                    ],
                    4,
                );
                assert_eq!(
                    infer_result_types_with_attrs(
                        opcode,
                        None,
                        &[lhs.clone(), rhs.clone()],
                        None,
                        1
                    ),
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
                    AttrDict::default(),
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
    for opcode in BINARY_CASES
        .into_iter()
        .map(|(opcode, _)| opcode)
        .chain([OpCode::BitNot])
    {
        let arity = if opcode == OpCode::BitNot { 1 } else { 2 };
        for count in (0..=3).filter(|count| *count != arity) {
            let operands = vec![TirType::I64; count];
            assert_eq!(
                infer_result_types_with_attrs(opcode, None, &operands, None, 1),
                vec![None]
            );
        }
        let operands = vec![TirType::I64; arity];
        for result_count in [0, 2] {
            assert_eq!(
                infer_result_types_with_attrs(opcode, None, &operands, None, result_count),
                vec![None; result_count],
            );
        }
        for (unknown, expected) in [
            (TirType::DynBox, None),
            (TirType::Never, Some(TirType::Never)),
            (TirType::F64, None),
            (
                TirType::Class(molt_ir::ClassId::of_definition("class:m:m.IntSubclass")),
                None,
            ),
        ] {
            for position in 0..arity {
                let mut operands = operands.clone();
                operands[position] = unknown.clone();
                assert_eq!(
                    infer_result_types_with_attrs(opcode, None, &operands, None, 1),
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
                        AttrDict::from_iter([("return_type".into(), AttrValue::Str("int".into()))]),
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
                        AttrDict::default(),
                    ),
                    make_op(OpCode::CheckException, vec![], vec![], AttrDict::default()),
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
                AttrDict::default(),
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

fn checked_runtime_integer(runtime: molt_ir::runtime_fn_generated::RuntimeFn) -> TirFunction {
    let big = ValueId(0);
    let result = ValueId(1);
    let mut call = make_op(
        OpCode::Call,
        vec![big; runtime.params()],
        vec![result],
        Default::default(),
    );
    call.runtime = Some(runtime);
    call.origin = Some(molt_ir::OpKind::Call);
    let mut func = single_block_func(
        vec![
            integer_constant(0, &TirType::BigInt),
            call,
            make_op(OpCode::CheckException, vec![], vec![], int_attr(70)),
            make_op(
                OpCode::BitOr,
                vec![big, result],
                vec![ValueId(2)],
                Default::default(),
            ),
        ],
        3,
    );
    func.return_abi = molt_ir::FunctionReturnAbi::Value;
    func.return_type = TirType::DynBox;
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Return {
        values: vec![ValueId(2)],
    };
    let handler = BlockId(1);
    func.has_exception_handling = true;
    func.next_block = 2;
    func.label_id_map.insert(handler.0, 70);
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func
}

#[test]
fn checked_runtime_integer_success_flows_to_arithmetic_and_keeps_promoted_storage() {
    use molt_ir::runtime_fn_generated::RuntimeFn;
    for runtime in [
        RuntimeFn::OperatorIndex,
        RuntimeFn::Buffer2dGet,
        RuntimeFn::Buffer2dRows,
        RuntimeFn::Buffer2dCols,
    ] {
        let func = checked_runtime_integer(runtime);
        let types = extract_exact_scalar_map(&func);
        let ranges = crate::representation_facts::value_range_for(&func);
        let reprs = crate::representation_facts::repr_by_value_for(&func, Some(&ranges));
        for id in [ValueId(1), ValueId(2)] {
            assert_eq!(types.get(&id), Some(&TirType::I64), "{runtime:?}");
            assert_eq!(reprs[&id], crate::repr::Repr::MaybeBigInt);
        }
    }
}

#[test]
fn checked_runtime_integer_requires_the_actual_target_check_and_only_successful_uses() {
    use molt_ir::runtime_fn_generated::RuntimeFn;
    for change in [
        "handler use",
        "late check",
        "wrong label",
        "wrong arity",
        "callable object",
        "other target",
        "compiled target",
    ] {
        let mut func = checked_runtime_integer(RuntimeFn::OperatorIndex);
        let body = func.blocks.get_mut(&func.entry_block).unwrap();
        match change {
            "handler use" => {
                func.blocks.get_mut(&BlockId(1)).unwrap().terminator = Terminator::Return {
                    values: vec![ValueId(1)],
                }
            }
            "late check" => body.ops.swap(2, 3),
            "wrong label" => body.ops[2].attrs = int_attr(71),
            "wrong arity" => body.ops[1].operands.clear(),
            "callable object" => body.ops[1].origin = Some(molt_ir::OpKind::BuiltinFunc),
            "other target" => body.ops[1].runtime = Some(RuntimeFn::SysIntern),
            "compiled target" => body.ops[1].callee = Some(molt_ir::FunctionId(0)),
            _ => unreachable!(),
        }
        let types = extract_exact_scalar_map(&func);
        assert!(!types.contains_key(&ValueId(1)), "{change}");
        assert!(!types.contains_key(&ValueId(2)), "{change}");
    }
}

#[test]
fn compiled_return_evidence_requires_successful_uses_in_checked_bodies() {
    let result = ValueId(1);
    for opcode in [OpCode::Call, OpCode::DirectCall] {
        for mode in ["success", "handler", "before check", "no observer"] {
            let mut func =
                checked_runtime_integer(molt_ir::runtime_fn_generated::RuntimeFn::OperatorIndex);
            let ops = &mut func.blocks.get_mut(&func.entry_block).unwrap().ops;
            let mut call = TirOp::new(opcode, vec![ValueId(0)], vec![result]);
            call.callee = Some(molt_ir::FunctionId(0));
            assert!(call.mark_exact_result_proof(&TirType::F64));
            ops[1] = call;
            if mode == "before check" {
                ops.swap(2, 3);
            } else if mode == "no observer" {
                ops.remove(2);
            }
            if opcode == OpCode::DirectCall {
                ops.insert(2, TirOp::new(OpCode::RecursionRelease, vec![], vec![]));
            }
            if mode == "handler" {
                func.blocks.get_mut(&BlockId(1)).unwrap().terminator = Terminator::Return {
                    values: vec![result],
                };
            }
            let expected = if mode == "success" {
                TirType::F64
            } else {
                TirType::DynBox
            };
            assert_eq!(
                solve_exact_scalars(&func, ExactSeeds::default())[&result],
                expected,
                "{opcode:?} {mode}"
            );
            if opcode == OpCode::Call {
                func.blocks.get_mut(&func.entry_block).unwrap().ops[1].clear_exact_result_proof();
                let sites = HashMap::from_iter([(result, 0)]);
                let seeds = ExactSeeds {
                    call_results: Some(ExactCallResults {
                        sites: &sites,
                        types: &[TirType::F64],
                    }),
                    ..ExactSeeds::default()
                };
                assert_eq!(
                    solve_exact_scalars(&func, seeds)[&result],
                    expected,
                    "seeded {mode}"
                );
            }
        }
    }
}

#[test]
fn checked_compiled_success_keeps_physical_handler_values_and_specializes_normal_arithmetic() {
    use crate::tir::passes::{alias_analysis, copy_prop, value_identity::ScalarFacts};
    let word = ValueId(1);
    let handler = BlockId(1);
    let sum = ValueId(2);
    let mut func = checked_runtime_integer(molt_ir::runtime_fn_generated::RuntimeFn::OperatorIndex);
    let mut call = TirOp::new(OpCode::Call, vec![ValueId(0)], vec![word]);
    call.callee = Some(molt_ir::FunctionId(0));
    call.mark_exact_result_proof(&TirType::I64);
    func.blocks.get_mut(&func.entry_block).unwrap().ops[1] = call;
    let handler_arg = func.fresh_value();
    let landing = func.blocks.get_mut(&handler).unwrap();
    landing.args.push(TirValue {
        id: handler_arg,
        ty: TirType::DynBox,
    });
    landing.terminator = Terminator::Return {
        values: vec![handler_arg],
    };
    func.blocks.get_mut(&func.entry_block).unwrap().ops[2].operands = vec![word];

    assert!(checked_successful_call_results(&func).is_empty());
    assert!(project_checked_call_results(&mut func, |_| true));
    assert!(!project_checked_call_results(&mut func, |_| true));
    copy_prop::run(&mut func);
    let ops = &func.blocks[&func.entry_block].ops;
    let success = ops[4].operands[1];
    assert_ne!(word, success);
    assert_eq!(ops[2].operands, [word]);
    let facts = solve_exact_scalars(&func, ExactSeeds::default());
    assert_eq!(facts[&word], TirType::DynBox);
    assert_eq!(facts[&handler_arg], TirType::DynBox);
    assert_eq!(facts[&success], TirType::I64);
    assert_eq!(facts[&sum], TirType::I64);
    let ownership = alias_analysis::build_alias_union_find(&func);
    assert_eq!(ownership.root(success), ownership.root(word));
    assert!(!ScalarFacts::new(&func).forwarding().contains_key(&success));

    let sites = HashMap::from_iter([(word, 0)]);
    func.blocks.get_mut(&func.entry_block).unwrap().ops[1].clear_exact_result_proof();
    let facts = solve_exact_scalars(
        &func,
        ExactSeeds {
            call_results: Some(ExactCallResults {
                sites: &sites,
                types: &[TirType::F64],
            }),
            ..Default::default()
        },
    );
    assert_eq!(facts[&word], TirType::DynBox);
    assert_eq!(facts[&success], TirType::F64);
}

#[test]
fn marked_copy_and_malformed_observers_cannot_prove_success() {
    let word = ValueId(1);
    let handler = BlockId(1);
    let mut base = checked_runtime_integer(molt_ir::runtime_fn_generated::RuntimeFn::OperatorIndex);
    let mut call = TirOp::new(OpCode::Call, vec![], vec![word]);
    call.callee = Some(molt_ir::FunctionId(0));
    call.mark_exact_result_proof(&TirType::Bool);
    base.blocks.get_mut(&base.entry_block).unwrap().ops[1] = call;
    let forged = base.fresh_value();
    let landing = base.blocks.get_mut(&handler).unwrap();
    landing.ops.push(TirOp::checked_call_result(word, forged));
    landing.terminator = Terminator::Return {
        values: vec![forged],
    };
    assert!(project_checked_call_results(&mut base, |_| true));
    assert_eq!(
        solve_exact_scalars(&base, ExactSeeds::default())[&forged],
        TirType::DynBox
    );
    for mode in [
        "poll",
        "missing block",
        "duplicate label",
        "payload",
        "result",
    ] {
        let mut func = base.clone();
        let body = func.blocks.get_mut(&func.entry_block).unwrap();
        let success = body.ops[3].results[0];
        match mode {
            "poll" => body.ops[2].origin = Some(molt_ir::OpKind::AsyncWorkPoll),
            "missing block" => {
                func.blocks.remove(&handler);
            }
            "duplicate label" => {
                func.label_id_map.insert(99, 70);
            }
            "payload" => body.ops[2].operands.push(word),
            "result" => body.ops[2].results.push(ValueId(55)),
            _ => unreachable!(),
        }
        assert!(
            !checked_successful_call_results(&func).contains(&word),
            "{mode}"
        );
        assert_eq!(
            solve_exact_scalars(&func, ExactSeeds::default())[&success],
            TirType::DynBox,
            "{mode}"
        );
    }
}

#[test]
fn bound_invocation_identity_is_not_a_physical_use_or_an_extra_handler_argument() {
    use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;
    let word = ValueId(1);
    let mut func = checked_runtime_integer(molt_ir::runtime_fn_generated::RuntimeFn::OperatorIndex);
    let ops = &mut func.blocks.get_mut(&func.entry_block).unwrap().ops;
    let mut call = TirOp::new(OpCode::Call, vec![], vec![word]);
    call.callee = Some(molt_ir::FunctionId(0));
    ops[1] = call;
    ops[2].set_exception_edge_kind(ExceptionEdgeKind::Unwind);
    assert!(bind_checked_invocations(&mut func));
    assert!(!bind_checked_invocations(&mut func));
    let check = &func.blocks[&func.entry_block].ops[2];
    assert_eq!(check.checked_invocation_source(), Some(word));
    assert!(check.exception_payload().is_empty());
    assert!(checked_successful_call_results(&func).contains(&word));

    let mut scheduled = func.clone();
    let local = scheduled.fresh_value();
    scheduled
        .blocks
        .get_mut(&scheduled.entry_block)
        .unwrap()
        .ops
        .insert(2, TirOp::new(OpCode::Copy, vec![ValueId(0)], vec![local]));
    assert!(checked_successful_call_results(&scheduled).contains(&word));
    let ops = &mut scheduled
        .blocks
        .get_mut(&scheduled.entry_block)
        .unwrap()
        .ops;
    ops[3].operands[0] = local;
    assert!(!checked_successful_call_results(&scheduled).contains(&word));

    let arg = func.fresh_value();
    func.blocks
        .get_mut(&BlockId(1))
        .unwrap()
        .args
        .push(TirValue {
            id: arg,
            ty: TirType::DynBox,
        });
    // A handler can independently receive exactly the same SSA value.
    func.blocks.get_mut(&func.entry_block).unwrap().ops[2]
        .operands
        .insert(0, word);
    assert!(!checked_successful_call_results(&func).contains(&word));
    assert!(project_checked_call_results(&mut func, |_| true));
    let body = &func.blocks[&func.entry_block];
    assert_eq!(body.ops[2].exception_payload(), &[word]);
    assert_eq!(body.ops[3].checked_call_result_source(), Some(word));
    assert_ne!(body.ops[3].results[0], word);
}
