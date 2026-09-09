use super::*;

#[test]
fn intrinsic_result_slots_preserve_exact_status_without_payload_or_purity_inference() {
    for opcode in [
        OpCode::CheckedAdd,
        OpCode::CheckedMul,
        OpCode::IterNextUnboxed,
    ] {
        let operands = if opcode == OpCode::IterNextUnboxed {
            vec![ValueId(0)]
        } else {
            vec![ValueId(0), ValueId(1)]
        };
        let func = single_block_func(
            vec![
                make_op(
                    opcode,
                    operands,
                    vec![ValueId(2), ValueId(3)],
                    AttrDict::new(),
                ),
                make_op(
                    OpCode::Copy,
                    vec![ValueId(3)],
                    vec![ValueId(4)],
                    AttrDict::new(),
                ),
                make_op(
                    OpCode::And,
                    vec![ValueId(3), ValueId(4)],
                    vec![ValueId(5)],
                    AttrDict::new(),
                ),
                make_op(
                    OpCode::Or,
                    vec![ValueId(3), ValueId(4)],
                    vec![ValueId(6)],
                    AttrDict::new(),
                ),
                make_op(
                    OpCode::ExceptionPending,
                    vec![],
                    vec![ValueId(7)],
                    AttrDict::new(),
                ),
            ],
            8,
        );
        let exact = extract_exact_scalar_map(&func);
        for id in 3..=7 {
            assert_eq!(
                exact.get(&ValueId(id)),
                Some(&TirType::Bool),
                "{opcode:?}, slot {id}"
            );
        }
        if opcode == OpCode::IterNextUnboxed {
            assert!(!exact.contains_key(&ValueId(2)));
        } else {
            assert_eq!(exact.get(&ValueId(2)), Some(&TirType::I64));
        }
        let effects =
            crate::tir::op_kinds_generated::opcode_effects_table(OpCode::ExceptionPending);
        assert!(!effects.consistent && !effects.effect_free);
        let proven = extract_proven_map(&func);
        assert_eq!(proven.get(&ValueId(3)), Some(&TirType::Bool));
        assert_eq!(proven.get(&ValueId(7)), Some(&TirType::Bool));
    }
}

#[test]
fn intrinsic_result_slots_reject_malformed_result_shapes_and_unknown_indices() {
    use crate::tir::op_kinds_generated::opcode_operand_independent_result_tir_type;
    for opcode in [
        OpCode::CheckedAdd,
        OpCode::CheckedMul,
        OpCode::IterNextUnboxed,
    ] {
        assert_eq!(opcode_operand_independent_result_tir_type(opcode, 2), None);
        for count in [1, 3] {
            let results: Vec<_> = (0..count).map(|index| ValueId(index as u32 + 2)).collect();
            let func = single_block_func(
                vec![make_op(
                    opcode,
                    vec![ValueId(0), ValueId(1)],
                    results.clone(),
                    AttrDict::new(),
                )],
                6,
            );
            let exact = extract_exact_scalar_map(&func);
            let proven = extract_proven_map(&func);
            for result in results {
                assert!(!exact.contains_key(&result));
                assert!(!proven.contains_key(&result));
            }
            assert_eq!(
                infer_result_types_with_attrs(opcode, &[], None, count),
                vec![None; count]
            );
        }
    }
    assert_eq!(
        infer_result_types_with_attrs(
            OpCode::IterNextUnboxed,
            &[TirType::Iterator(Box::new(TirType::Str))],
            None,
            2
        ),
        vec![Some(TirType::Str), Some(TirType::Bool)],
    );
}

#[test]
fn intrinsic_overflow_status_survives_loop_fanin_but_dynamic_edges_poison_exactness() {
    for dynamic_backedge in [false, true] {
        let mut func = single_block_func(
            vec![make_op(
                OpCode::CheckedAdd,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2), ValueId(3)],
                AttrDict::new(),
            )],
            7,
        );
        let header = BlockId(1);
        let exit = BlockId(2);
        func.next_block = 3;
        func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Branch {
            target: header,
            args: vec![ValueId(3)],
        };
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ValueId(4),
                    ty: TirType::Bool,
                }],
                ops: vec![make_op(
                    OpCode::Or,
                    vec![ValueId(4), ValueId(3)],
                    vec![ValueId(5)],
                    AttrDict::new(),
                )],
                terminator: Terminator::CondBranch {
                    cond: ValueId(3),
                    then_block: header,
                    then_args: vec![if dynamic_backedge {
                        ValueId(0)
                    } else {
                        ValueId(5)
                    }],
                    else_block: exit,
                    else_args: vec![ValueId(5)],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![TirValue {
                    id: ValueId(6),
                    ty: TirType::Bool,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![ValueId(6)],
                },
            },
        );
        let exact = extract_exact_scalar_map(&func);
        assert_eq!(exact.get(&ValueId(3)), Some(&TirType::Bool));
        for id in 4..=6 {
            assert_eq!(
                exact.get(&ValueId(id)),
                (!dynamic_backedge).then_some(&TirType::Bool)
            );
        }
    }
}

#[test]
fn exact_labelled_loop_facts_only_widen_for_admitted_exception_edges() {
    for has_exception_edge in [false, true] {
        let mut func = single_block_func(
            vec![
                make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(0)),
                make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(10)),
            ],
            6,
        );
        let header = BlockId(1);
        let exit = BlockId(2);
        func.next_block = 3;
        func.label_id_map.insert(header.0, 20);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![ValueId(0)],
        };
        if has_exception_edge {
            entry
                .ops
                .push(make_op(OpCode::TryStart, vec![], vec![], int_attr(20)));
            func.has_exception_handling = true;
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ValueId(2),
                    ty: TirType::I64,
                }],
                ops: vec![
                    make_op(OpCode::ConstInt, vec![], vec![ValueId(3)], int_attr(1)),
                    make_op(
                        OpCode::Add,
                        vec![ValueId(2), ValueId(3)],
                        vec![ValueId(4)],
                        AttrDict::new(),
                    ),
                    make_op(
                        OpCode::Lt,
                        vec![ValueId(4), ValueId(1)],
                        vec![ValueId(5)],
                        AttrDict::new(),
                    ),
                ],
                terminator: Terminator::CondBranch {
                    cond: ValueId(5),
                    then_block: header,
                    then_args: vec![ValueId(4)],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let exact = extract_exact_scalar_map(&func);
        let representations = crate::representation_facts::repr_by_value_for(&func, None);
        if has_exception_edge {
            assert!(!exact.contains_key(&ValueId(2)));
            assert!(!exact.contains_key(&ValueId(4)));
            assert!(!exact.contains_key(&ValueId(5)));
            assert_eq!(representations[&ValueId(5)], crate::repr::Repr::DynBox);
        } else {
            assert_eq!(exact.get(&ValueId(2)), Some(&TirType::I64));
            assert_eq!(exact.get(&ValueId(4)), Some(&TirType::I64));
            assert_eq!(exact.get(&ValueId(5)), Some(&TirType::Bool));
            assert_eq!(representations[&ValueId(5)], crate::repr::Repr::Bool);
        }
    }
}

// ---- Test 1: Constants resolve to concrete types ----
#[test]
fn constants_resolve_to_concrete_types() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(42)),
        make_op(OpCode::ConstFloat, vec![], vec![ValueId(1)], float_attr(PI)),
        make_op(
            OpCode::ConstStr,
            vec![],
            vec![ValueId(2)],
            str_attr("hello"),
        ),
        make_op(OpCode::ConstBool, vec![], vec![ValueId(3)], AttrDict::new()),
        make_op(OpCode::ConstNone, vec![], vec![ValueId(4)], AttrDict::new()),
        make_op(
            OpCode::ConstBytes,
            vec![],
            vec![ValueId(5)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 6);
    let refined = refine_types(&mut func);
    // All 6 values should be refined from DynBox to concrete types.
    assert_eq!(refined, 6);
}

// ---- Test 2: Arithmetic propagates types ----
#[test]
fn arithmetic_propagates_i64() {
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
    let refined = refine_types(&mut func);
    assert_eq!(refined, 3); // two consts + one add result
}

#[test]
fn module_get_attr_result_stays_dynbox() {
    let ops = vec![
        make_op(
            OpCode::ConstStr,
            vec![],
            vec![ValueId(0)],
            str_attr("module_name"),
        ),
        make_op(
            OpCode::ConstStr,
            vec![],
            vec![ValueId(1)],
            str_attr("Point"),
        ),
        make_op(
            OpCode::ModuleGetAttr,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    let refined = refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(refined, 2, "only the const_str operands refine to Str");
    assert_eq!(
        type_map.get(&ValueId(2)),
        Some(&TirType::DynBox),
        "module_get_attr result must not inherit the module operand type"
    );
}

#[test]
fn module_lookup_results_stay_dynbox() {
    for opcode in [
        OpCode::ModuleCacheGet,
        OpCode::ModuleGetGlobal,
        OpCode::ModuleGetName,
    ] {
        let operands = if opcode == OpCode::ModuleCacheGet {
            vec![ValueId(0)]
        } else {
            vec![ValueId(0), ValueId(1)]
        };
        let ops = vec![
            make_op(
                OpCode::ConstStr,
                vec![],
                vec![ValueId(0)],
                str_attr("module_name"),
            ),
            make_op(
                OpCode::ConstStr,
                vec![],
                vec![ValueId(1)],
                str_attr("answer"),
            ),
            make_op(opcode, operands, vec![ValueId(2)], AttrDict::new()),
        ];
        let mut func = single_block_func(ops, 3);
        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&ValueId(2)),
            Some(&TirType::DynBox),
            "{opcode:?} result must not inherit the module/name operand type"
        );
    }
}

// ---- Test 3: Mixed arithmetic promotes to F64 ----
#[test]
fn mixed_arithmetic_promotes_to_f64() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(
            OpCode::ConstFloat,
            vec![],
            vec![ValueId(1)],
            float_attr(2.0),
        ),
        make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    let refined = refine_types(&mut func);
    assert_eq!(refined, 3);
}

// ---- Test 4: Comparison produces Bool ----
#[test]
fn comparison_produces_bool() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
        make_op(
            OpCode::Eq,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    let refined = refine_types(&mut func);
    assert_eq!(refined, 3);
}

#[test]
fn rich_comparison_annotations_do_not_prove_scalar_results_or_effects() {
    for opcode in [
        OpCode::Eq,
        OpCode::Ne,
        OpCode::Lt,
        OpCode::Le,
        OpCode::Gt,
        OpCode::Ge,
    ] {
        let mut func = single_block_func(
            vec![
                make_op(
                    opcode,
                    vec![ValueId(0), ValueId(1)],
                    vec![ValueId(2)],
                    AttrDict::new(),
                ),
                make_op(
                    opcode,
                    vec![ValueId(0), ValueId(1)],
                    vec![ValueId(3)],
                    AttrDict::new(),
                ),
            ],
            4,
        );
        func.param_types = vec![TirType::I64, TirType::I64];
        func.value_types.extend([
            (ValueId(0), TirType::I64),
            (ValueId(1), TirType::I64),
            (ValueId(2), TirType::Bool),
            (ValueId(3), TirType::Bool),
        ]);
        assert!(extract_exact_scalar_map(&func).is_empty());
        refine_types(&mut func);
        assert_eq!(extract_type_map(&func)[&ValueId(2)], TirType::DynBox);
        let representations = crate::representation_facts::repr_by_value_for(&func, None);
        assert_eq!(representations[&ValueId(2)], crate::repr::Repr::DynBox);
        crate::tir::passes::dce::run(&mut func);
        assert_eq!(
            func.blocks[&BlockId(0)]
                .ops
                .iter()
                .filter(|op| op.opcode == opcode)
                .count(),
            2,
            "{opcode:?}: discarded rich comparisons can invoke callbacks"
        );
    }
}

#[test]
fn exact_scalar_provenance_survives_arithmetic_and_comparison_chains() {
    let func = single_block_func(
        vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
            make_op(
                OpCode::Add,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::Lt,
                vec![ValueId(2), ValueId(1)],
                vec![ValueId(3)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::Eq,
                vec![ValueId(3), ValueId(0)],
                vec![ValueId(4)],
                AttrDict::new(),
            ),
        ],
        5,
    );
    let exact = extract_exact_scalar_map(&func);
    assert_eq!(exact[&ValueId(2)], TirType::I64);
    assert_eq!(exact[&ValueId(3)], TirType::Bool);
    assert_eq!(exact[&ValueId(4)], TirType::Bool);
    let mut dead = func.clone();
    crate::tir::passes::dce::run(&mut dead);
    assert!(dead.blocks[&BlockId(0)].ops.is_empty());
}

#[test]
fn power_result_domain_does_not_invent_scalar_comparison_proofs() {
    for (base, exponent) in [
        (
            make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(0)],
                float_attr(-1.0),
            ),
            make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(1)],
                float_attr(0.5),
            ),
        ),
        (
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(-1)),
            make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(1)],
                float_attr(0.5),
            ),
        ),
        (
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(2)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(-1)),
        ),
    ] {
        let mut func = single_block_func(
            vec![
                base,
                exponent,
                make_op(
                    OpCode::Pow,
                    vec![ValueId(0), ValueId(1)],
                    vec![ValueId(2)],
                    AttrDict::new(),
                ),
                make_op(OpCode::CheckException, vec![], vec![], AttrDict::new()),
                make_op(
                    OpCode::Lt,
                    vec![ValueId(2), ValueId(0)],
                    vec![ValueId(3)],
                    AttrDict::new(),
                ),
                make_op(OpCode::CheckException, vec![], vec![], AttrDict::new()),
            ],
            4,
        );
        let exact = extract_exact_scalar_map(&func);
        assert!(!exact.contains_key(&ValueId(2)));
        assert!(!exact.contains_key(&ValueId(3)));
        refine_types(&mut func);
        let types = extract_type_map(&func);
        assert_eq!(types[&ValueId(2)], TirType::DynBox);
        assert_eq!(types[&ValueId(3)], TirType::DynBox);
        let reprs = crate::representation_facts::repr_by_value_for(&func, None);
        assert_eq!(reprs[&ValueId(2)], crate::repr::Repr::DynBox);
        assert_eq!(reprs[&ValueId(3)], crate::repr::Repr::DynBox);
        crate::tir::passes::check_exception_elim::run(&mut func);
        assert_eq!(
            func.blocks[&BlockId(0)]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::CheckException)
                .count(),
            2,
        );
        crate::tir::passes::dce::run(&mut func);
        assert!(
            func.blocks[&BlockId(0)]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::Lt)
        );
    }
}

#[test]
fn malformed_result_shapes_do_not_mint_exact_scalar_facts() {
    let malformed_constant = single_block_func(
        vec![make_op(
            OpCode::ConstInt,
            vec![],
            vec![ValueId(0), ValueId(1)],
            int_attr(1),
        )],
        2,
    );
    assert!(extract_exact_scalar_map(&malformed_constant).is_empty());
    for results in [vec![], vec![ValueId(2), ValueId(3)]] {
        let mut func = single_block_func(
            vec![
                make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
                make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
                make_op(
                    OpCode::Eq,
                    vec![ValueId(0), ValueId(1)],
                    results.clone(),
                    AttrDict::new(),
                ),
            ],
            4,
        );
        let exact = extract_exact_scalar_map(&func);
        assert!(results.iter().all(|value| !exact.contains_key(value)));
        let op = &func.blocks[&BlockId(0)].ops[2];
        let comparison =
            crate::tir::predicate_semantics::predicate_facts_for_op(op, &exact).unwrap();
        assert_eq!(comparison.result_type, TirType::DynBox);
        assert!(!comparison.effects.effect_free);
        assert!(!comparison.effects.nothrow);
        let reprs = crate::representation_facts::repr_by_value_for(&func, None);
        assert!(
            results
                .iter()
                .all(|value| reprs[value] == crate::repr::Repr::DynBox)
        );
        crate::tir::passes::dce::run(&mut func);
        assert!(
            func.blocks[&BlockId(0)]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::Eq)
        );
    }
}

#[test]
fn truth_and_containment_callbacks_survive_dead_predicate_chains() {
    for opcode in [OpCode::Bool, OpCode::Not, OpCode::In, OpCode::NotIn] {
        let operands = if matches!(opcode, OpCode::Bool | OpCode::Not) {
            vec![ValueId(0)]
        } else {
            vec![ValueId(0), ValueId(1)]
        };
        let mut func = single_block_func(
            vec![
                make_op(OpCode::ConstBool, vec![], vec![ValueId(2)], AttrDict::new()),
                make_op(opcode, operands, vec![ValueId(3)], AttrDict::new()),
                make_op(OpCode::CheckException, vec![], vec![], AttrDict::new()),
                make_op(
                    OpCode::Eq,
                    vec![ValueId(3), ValueId(2)],
                    vec![ValueId(4)],
                    AttrDict::new(),
                ),
            ],
            5,
        );
        let exact = extract_exact_scalar_map(&func);
        assert_eq!(exact[&ValueId(3)], TirType::Bool);
        assert_eq!(exact[&ValueId(4)], TirType::Bool);
        crate::tir::passes::check_exception_elim::run(&mut func);
        assert!(
            func.blocks[&BlockId(0)]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::CheckException)
        );
        crate::tir::passes::dce::run(&mut func);
        assert!(
            func.blocks[&BlockId(0)]
                .ops
                .iter()
                .any(|op| op.opcode == opcode)
        );
        assert!(
            !func.blocks[&BlockId(0)]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::Eq)
        );
    }
    for opcode in [OpCode::Bool, OpCode::Not] {
        let mut func = single_block_func(
            vec![
                make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
                make_op(opcode, vec![ValueId(0)], vec![ValueId(1)], AttrDict::new()),
            ],
            2,
        );
        crate::tir::passes::dce::run(&mut func);
        assert!(func.blocks[&BlockId(0)].ops.is_empty());
    }
}

#[test]
fn double_not_elimination_requires_exact_bool_input() {
    for exact_bool in [false, true] {
        let mut ops = vec![];
        if exact_bool {
            ops.push(make_op(
                OpCode::ConstBool,
                vec![],
                vec![ValueId(0)],
                AttrDict::new(),
            ));
        }
        ops.extend([
            make_op(
                OpCode::Not,
                vec![ValueId(0)],
                vec![ValueId(1)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::Not,
                vec![ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ]);
        let mut func = single_block_func(ops, 3);
        crate::tir::passes::canonicalize::run(&mut func);
        let result = func.blocks[&BlockId(0)]
            .ops
            .iter()
            .find(|op| op.results == vec![ValueId(2)])
            .unwrap();
        assert_eq!(result.opcode == OpCode::Copy, exact_bool);
    }
}

/// Locks in the contract that `InplaceAdd`/`InplaceSub`/`InplaceMul`
/// participate in numeric arithmetic inference identically to their
/// regular `Add`/`Sub`/`Mul` counterparts. Without this, an
/// accumulator pattern like `total += i` (lowered as `InplaceAdd`)
/// stays at DynBox even when both operands are I64, causing the
/// native backend to coerce to a float lane and silently miscompile
/// the integer accumulator (printed bits look like a denormal float).
#[test]
fn inplace_add_typed_to_i64_for_int_operands() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(10)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(20)),
        make_op(
            OpCode::InplaceAdd,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
        make_op(
            OpCode::InplaceSub,
            vec![ValueId(2), ValueId(1)],
            vec![ValueId(3)],
            AttrDict::new(),
        ),
        make_op(
            OpCode::InplaceMul,
            vec![ValueId(3), ValueId(0)],
            vec![ValueId(4)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 5);
    let _refined = refine_types(&mut func);

    // Re-extract the type map post-refinement to inspect op result
    // types (block args were already covered by other tests).
    let env = extract_type_map(&func);
    assert_eq!(
        env.get(&ValueId(2)),
        Some(&TirType::I64),
        "InplaceAdd of (I64, I64) must produce I64"
    );
    assert_eq!(
        env.get(&ValueId(3)),
        Some(&TirType::I64),
        "InplaceSub of (I64, I64) must produce I64"
    );
    assert_eq!(
        env.get(&ValueId(4)),
        Some(&TirType::I64),
        "InplaceMul of (I64, I64) must produce I64"
    );
}

/// Round-8 regression: a FRESH-VALUE scalar-conversion `Copy`
/// (`int_from_obj`/`float_from_obj`) is NOT a transparent type alias of its
/// operand — it mints a NEW raw-register value whose type the conversion
/// determines. `int(t)` with `t: float` lowers to `Copy[int_from_obj](t)`; the
/// old `Copy => operand_types.first()` rule type-aliased it to `t`'s `F64`,
/// flooding the downstream integer accumulator (`total += int(t)`) with a
/// spurious float carrier → native `def_var` repr mismatch / LIR-verifier
/// branch-repr divergence (`os._seconds_float_to_sec_nsec`). A TRANSPARENT
/// alias (`copy_var`/bare `Copy`) MUST still propagate the operand type.
#[test]
fn int_from_obj_copy_of_float_is_i64_not_aliased_to_operand() {
    let int_from_obj_attr = {
        let mut a = AttrDict::new();
        a.insert(
            "_original_kind".into(),
            AttrValue::Str("int_from_obj".into()),
        );
        a
    };
    let copy_var_attr = {
        let mut a = AttrDict::new();
        a.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
        a
    };
    let ops = vec![
        // t = <float> (a const float stands in for the float parameter).
        make_op(
            OpCode::ConstFloat,
            vec![],
            vec![ValueId(0)],
            AttrDict::new(),
        ),
        // sec = int(t)  →  Copy[int_from_obj](t). MUST type to I64, not F64.
        make_op(
            OpCode::Copy,
            vec![ValueId(0)],
            vec![ValueId(1)],
            int_from_obj_attr,
        ),
        // total = 0; total += sec  →  the integer accumulator that mis-typed.
        make_op(OpCode::ConstInt, vec![], vec![ValueId(2)], int_attr(0)),
        make_op(
            OpCode::InplaceAdd,
            vec![ValueId(2), ValueId(1)],
            vec![ValueId(3)],
            AttrDict::new(),
        ),
        // A TRANSPARENT alias of the float MUST keep the operand's F64 type.
        make_op(
            OpCode::Copy,
            vec![ValueId(0)],
            vec![ValueId(4)],
            copy_var_attr,
        ),
    ];
    let mut func = single_block_func(ops, 5);
    refine_types(&mut func);
    let env = extract_type_map(&func);
    assert_eq!(
        env.get(&ValueId(1)),
        Some(&TirType::I64),
        "Copy[int_from_obj](F64) must produce I64 (a fresh int), NOT alias the float operand"
    );
    assert_eq!(
        env.get(&ValueId(3)),
        Some(&TirType::I64),
        "InplaceAdd(I64 accumulator, int(t)) must stay I64 — the accumulator must not float-contaminate"
    );
    assert_eq!(
        env.get(&ValueId(4)),
        Some(&TirType::F64),
        "a TRANSPARENT-alias Copy (copy_var) must still propagate operand 0's F64 type"
    );
}

/// The `copy_kind_raw_carrier_type` source of truth: raw-carrier scalar
/// conversions map to their precise scalar; every other `Copy` kind (including
/// heap-producing fresh values and transparent aliases) returns `None` so the
/// caller keeps operand-0 propagation. Pins the narrow scope that keeps the
/// heap-value type lattice byte-identical to the pre-fix behavior.
#[test]
fn raw_carrier_type_is_scoped_to_scalar_conversions() {
    use crate::tir::passes::alias_analysis::copy_kind_raw_carrier_type;
    assert_eq!(
        copy_kind_raw_carrier_type(Some("int_from_obj")),
        Some(TirType::I64)
    );
    assert_eq!(
        copy_kind_raw_carrier_type(Some("int_from_str_of_obj")),
        Some(TirType::I64)
    );
    assert_eq!(
        copy_kind_raw_carrier_type(Some("float_from_obj")),
        Some(TirType::F64)
    );
    assert_eq!(
        copy_kind_raw_carrier_type(Some("contains")),
        Some(TirType::Bool)
    );
    // Heap-producing fresh values → None (operand-0 propagation / DynBox floor).
    assert_eq!(copy_kind_raw_carrier_type(Some("str_from_obj")), None);
    assert_eq!(copy_kind_raw_carrier_type(Some("list_new")), None);
    assert_eq!(copy_kind_raw_carrier_type(Some("tuple_new")), None);
    assert_eq!(copy_kind_raw_carrier_type(Some("enumerate")), None);
    // Transparent aliases / bare Copy / unknown → None.
    assert_eq!(copy_kind_raw_carrier_type(Some("copy_var")), None);
    assert_eq!(copy_kind_raw_carrier_type(Some("guard_tag")), None);
    assert_eq!(copy_kind_raw_carrier_type(None), None);
}

#[test]
fn exact_scalar_copies_use_shared_value_identity_not_annotation_hints() {
    for kind in [
        "copy",
        "copy_var",
        "store_var",
        "load_var",
        "identity_alias",
    ] {
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
        let func = single_block_func(
            vec![
                make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
                make_op(
                    OpCode::Copy,
                    vec![ValueId(0), ValueId(0)],
                    vec![ValueId(1)],
                    attrs.clone(),
                ),
                make_op(
                    OpCode::Copy,
                    vec![ValueId(0), ValueId(2)],
                    vec![ValueId(3)],
                    attrs,
                ),
            ],
            4,
        );
        let exact = extract_exact_scalar_map(&func);
        assert_eq!(exact.get(&ValueId(1)), Some(&TirType::I64), "{kind}");
        assert!(
            !exact.contains_key(&ValueId(3)),
            "mixed source {kind} must not transfer exactness"
        );
    }
    let mut attrs = AttrDict::new();
    attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("opaque_callback".into()),
    );
    let func = single_block_func(
        vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(OpCode::Copy, vec![ValueId(0)], vec![ValueId(1)], attrs),
        ],
        2,
    );
    assert!(!extract_exact_scalar_map(&func).contains_key(&ValueId(1)));
}

#[test]
fn dynamic_operand_selects_do_not_inherit_stale_bool_representations() {
    for opcode in [OpCode::And, OpCode::Or] {
        let mut func = single_block_func(
            vec![
                make_op(OpCode::ConstBool, vec![], vec![ValueId(1)], AttrDict::new()),
                make_op(
                    opcode,
                    vec![ValueId(0), ValueId(1)],
                    vec![ValueId(2)],
                    AttrDict::new(),
                ),
                make_op(OpCode::CheckException, vec![], vec![], AttrDict::new()),
                make_op(
                    opcode,
                    vec![ValueId(1), ValueId(1)],
                    vec![ValueId(3)],
                    AttrDict::new(),
                ),
            ],
            4,
        );
        func.param_types = vec![TirType::Bool];
        func.value_types.extend([
            (ValueId(0), TirType::Bool),
            (ValueId(2), TirType::Bool),
            (ValueId(3), TirType::Bool),
        ]);
        let exact = extract_exact_scalar_map(&func);
        assert!(!exact.contains_key(&ValueId(2)));
        assert_eq!(exact.get(&ValueId(3)), Some(&TirType::Bool));
        let before_refine = crate::representation_facts::repr_by_value_for(&func, None);
        assert_eq!(before_refine[&ValueId(2)], crate::repr::Repr::DynBox);
        assert_eq!(before_refine[&ValueId(3)], crate::repr::Repr::Bool);
        assert_eq!(extract_type_map(&func)[&ValueId(2)], TirType::DynBox);
        refine_types(&mut func);
        assert_eq!(func.value_types[&ValueId(2)], TirType::DynBox);
        let reprs = crate::representation_facts::repr_by_value_for(&func, None);
        assert_eq!(reprs[&ValueId(2)], crate::repr::Repr::DynBox);
        assert_eq!(reprs[&ValueId(3)], crate::repr::Repr::Bool);
        crate::tir::passes::check_exception_elim::run(&mut func);
        assert!(
            func.blocks[&BlockId(0)]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::CheckException)
        );
        crate::tir::passes::dce::run(&mut func);
        assert!(
            func.blocks[&BlockId(0)]
                .ops
                .iter()
                .any(|op| op.results.contains(&ValueId(2)))
        );
    }
}
