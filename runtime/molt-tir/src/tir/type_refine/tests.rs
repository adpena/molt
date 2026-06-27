use super::*;
use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use std::f64::consts::PI;

/// Helper: build a simple function with one block containing the given ops.
fn single_block_func(ops: Vec<TirOp>, next_value: u32) -> TirFunction {
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![],
        ops,
        terminator: Terminator::Return { values: vec![] },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, block);
    TirFunction {
        name: "test".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value,
        next_block: 1,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    }
}

fn make_op(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    attrs: AttrDict,
) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

fn int_attr(val: i64) -> AttrDict {
    let mut m = AttrDict::new();
    m.insert("value".into(), AttrValue::Int(val));
    m
}

fn float_attr(val: f64) -> AttrDict {
    let mut m = AttrDict::new();
    m.insert("value".into(), AttrValue::Float(val));
    m
}

fn str_attr(val: &str) -> AttrDict {
    let mut m = AttrDict::new();
    m.insert("value".into(), AttrValue::Str(val.into()));
    m
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
fn exception_label_forwarding_args_widen_downstream_merge_args() {
    let normal_value = ValueId(0);
    let handler_arg = ValueId(1);
    let merge_arg = ValueId(2);
    let entry = BlockId(0);
    let handler = BlockId(1);
    let merge = BlockId(2);

    let mut blocks = HashMap::new();
    blocks.insert(
        entry,
        TirBlock {
            id: entry,
            args: vec![],
            ops: vec![
                make_op(OpCode::TryStart, vec![], vec![], int_attr(10)),
                make_op(OpCode::ConstInt, vec![], vec![normal_value], int_attr(1)),
            ],
            terminator: Terminator::Branch {
                target: merge,
                args: vec![normal_value],
            },
        },
    );
    blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: merge,
                args: vec![handler_arg],
            },
        },
    );
    blocks.insert(
        merge,
        TirBlock {
            id: merge,
            args: vec![TirValue {
                id: merge_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![merge_arg],
            },
        },
    );
    let mut label_id_map = HashMap::new();
    label_id_map.insert(handler.0, 10);
    let mut func = TirFunction {
        name: "exception_label_forwarding_args_widen_downstream_merge_args".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry,
        next_value: 3,
        next_block: 3,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: true,
        label_id_map,
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&handler_arg), Some(&TirType::DynBox));
    assert_eq!(
        type_map.get(&merge_arg),
        Some(&TirType::DynBox),
        "merge block args must widen when an exception-label forwarding block can feed DynBox"
    );
    assert_eq!(
        func.blocks[&merge].args[0].ty,
        TirType::DynBox,
        "refine_types must write the widened merge type back into block args"
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

// ---- Test 5: Block argument meet ----
#[test]
fn block_arg_meet_same_types() {
    // Two predecessor blocks both pass I64 to a join block's arg.
    let entry_id = BlockId(0);
    let then_id = BlockId(1);
    let else_id = BlockId(2);
    let join_id = BlockId(3);

    let mut blocks = HashMap::new();

    // Entry: cond branch to then/else
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::Bool,
            }],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            },
        },
    );

    // Then: const int, branch to join
    blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstInt,
                vec![],
                vec![ValueId(1)],
                int_attr(10),
            )],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(1)],
            },
        },
    );

    // Else: const int, branch to join
    blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstInt,
                vec![],
                vec![ValueId(2)],
                int_attr(20),
            )],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(2)],
            },
        },
    );

    // Join: one block arg (starts as DynBox), return
    blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: ValueId(3),
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![ValueId(3)],
            },
        },
    );

    let mut func = TirFunction {
        name: "join_test".into(),
        param_names: vec!["p0".into()],
        param_types: vec![TirType::Bool],
        return_type: TirType::I64,
        blocks,
        entry_block: entry_id,
        next_value: 4,
        next_block: 4,
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

    // ValueId(1), ValueId(2) (const ints) and ValueId(3) (block arg) should
    // all be refined. ValueId(3) should be meet(I64, I64) = I64.
    assert!(refined >= 3);
    assert_eq!(func.blocks[&join_id].args[0].ty, TirType::I64);
}

#[test]
fn block_arg_meet_different_types_produces_union() {
    // One branch passes I64, another passes F64 → Union(I64, F64).
    let entry_id = BlockId(0);
    let then_id = BlockId(1);
    let else_id = BlockId(2);
    let join_id = BlockId(3);

    let mut blocks = HashMap::new();

    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::Bool,
            }],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            },
        },
    );

    blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstInt,
                vec![],
                vec![ValueId(1)],
                int_attr(10),
            )],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(1)],
            },
        },
    );

    blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(2)],
                float_attr(PI),
            )],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(2)],
            },
        },
    );

    blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: ValueId(3),
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![ValueId(3)],
            },
        },
    );

    let mut func = TirFunction {
        name: "union_test".into(),
        param_names: vec!["p0".into()],
        param_types: vec![TirType::Bool],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry_id,
        next_value: 4,
        next_block: 4,
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
    assert!(refined >= 3);

    let join_arg_ty = &func.blocks[&join_id].args[0].ty;
    // Union member order depends on HashMap iteration order; accept either.
    assert!(
        *join_arg_ty == TirType::Union(vec![TirType::I64, TirType::F64])
            || *join_arg_ty == TirType::Union(vec![TirType::F64, TirType::I64]),
        "expected Union(I64, F64) in any order, got {:?}",
        join_arg_ty
    );
}

// ---- Test 6: Fixpoint convergence ----
#[test]
fn fixpoint_converges() {
    // Chain: ConstInt → Add → Add — all should resolve in ≤2 rounds.
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
        make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
        make_op(
            OpCode::Add,
            vec![ValueId(2), ValueId(0)],
            vec![ValueId(3)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 4);
    let refined = refine_types(&mut func);
    assert_eq!(refined, 4);
}

// ---- Test 7: DynBox stays DynBox when operands are unknown ----
#[test]
fn dynbox_stays_dynbox_for_unknown_operands() {
    // Add(DynBox, DynBox) → DynBox (no refinement possible)
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![
            TirValue {
                id: ValueId(0),
                ty: TirType::DynBox,
            },
            TirValue {
                id: ValueId(1),
                ty: TirType::DynBox,
            },
        ],
        ops: vec![make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        )],
        terminator: Terminator::Return {
            values: vec![ValueId(2)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, block);
    let mut func = TirFunction {
        name: "dynbox_test".into(),
        param_names: vec!["p0".into(), "p1".into()],
        param_types: vec![TirType::DynBox, TirType::DynBox],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry_id,
        next_value: 3,
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
    assert_eq!(refined, 0);
}

#[test]
fn dynamic_transfer_widens_stale_precise_result_and_stays_idempotent() {
    let source = ValueId(0);
    let result = ValueId(1);
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
    let mut func = single_block_func(
        vec![make_op(OpCode::Copy, vec![source], vec![result], attrs)],
        2,
    );
    func.value_types.insert(source, TirType::DynBox);
    func.value_types.insert(result, TirType::Str);

    refine_types(&mut func);
    let once = extract_type_map(&func);
    assert_eq!(
        once.get(&result),
        Some(&TirType::DynBox),
        "a dynamic producer must widen stale precise facts to top"
    );

    refine_types(&mut func);
    let twice = extract_type_map(&func);
    assert_eq!(
        twice.get(&result),
        Some(&TirType::DynBox),
        "post-refinement extraction must not re-narrow widened top facts"
    );
}

#[test]
fn never_bottom_waits_for_late_dominator_and_drops_stale_result_fact() {
    let body_id = BlockId(0);
    let entry_id = BlockId(1);
    let source = ValueId(0);
    let alias = ValueId(1);

    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::Copy,
            vec![source],
            vec![alias],
            AttrDict::new(),
        )],
        terminator: Terminator::Return {
            values: vec![alias],
        },
    };
    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::ConstInt,
            vec![],
            vec![source],
            int_attr(41),
        )],
        terminator: Terminator::Branch {
            target: body_id,
            args: vec![],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(body_id, body);
    blocks.insert(entry_id, entry);

    let mut func = TirFunction {
        name: "never_bottom_late_dominator".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::I64,
        blocks,
        entry_block: entry_id,
        next_value: 2,
        next_block: 2,
        attrs: AttrDict::new(),
        value_types: HashMap::from([(alias, TirType::Str)]),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    refine_types(&mut func);
    let once = extract_type_map(&func);
    assert_eq!(
        once.get(&alias),
        Some(&TirType::I64),
        "bottom operands must wait for the dominating producer instead of \
         publishing the stale result fact or cementing DynBox"
    );
    assert_eq!(
        func.value_types.get(&alias),
        Some(&TirType::I64),
        "refine_types must publish the converged fact, not the stale input fact"
    );

    refine_types(&mut func);
    assert_eq!(
        extract_type_map(&func),
        once,
        "Never-bottom convergence must be idempotent after publication"
    );
}

#[test]
fn dense_check_exception_results_are_top_and_idempotent() {
    let ops: Vec<TirOp> = (0..1024)
        .map(|idx| {
            make_op(
                OpCode::CheckException,
                vec![],
                vec![ValueId(idx)],
                AttrDict::new(),
            )
        })
        .collect();
    let mut func = single_block_func(ops, 1024);
    func.name = "dense_exception_poll".into();
    func.has_exception_handling = true;

    refine_types(&mut func);
    let value_types_after_first = func.value_types.clone();
    assert_eq!(value_types_after_first.len(), 1024);
    assert!(
        value_types_after_first
            .values()
            .all(|ty| matches!(ty, TirType::DynBox)),
        "check_exception result facts are dynamic top"
    );

    refine_types(&mut func);
    assert_eq!(
        func.value_types, value_types_after_first,
        "dense exception refinement must be idempotent"
    );
}

/// Lock-in for the loop-induction-variable seeding contract.
///
/// CFG:
/// ```text
/// entry:  i_init = ConstInt(0); branch header(i_init)
/// header(i: ?):  cond = ConstBool(true); cond_branch body, exit
/// body:  one = ConstInt(1); i_next = Add(i, one); branch header(i_next)
/// exit:  return
/// ```
/// Without IV seeding, `i` ends up DynBox: the body sees `i: DynBox`
/// initially, infers `Add(DynBox, I64)` as no-type, the back-edge
/// brings DynBox, and `meet(I64, DynBox) = DynBox` widens the entry.
/// With IV seeding, `i` is initialized to I64 (the entry-edge type
/// alone, since the back-edge is excluded from the seed), the body
/// then infers `Add(I64, I64) = I64`, the back-edge confirms I64,
/// and the fixpoint converges to I64.
#[test]
fn loop_iv_block_arg_seeded_to_entry_type() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let body_id = BlockId(2);
    let exit_id = BlockId(3);

    let i_init = ValueId(0);
    let i = ValueId(1);
    let cond = ValueId(2);
    let one = ValueId(3);
    let i_next = ValueId(4);

    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_init],
        },
    };
    let header = TirBlock {
        id: header_id,
        args: vec![TirValue {
            id: i,
            ty: TirType::DynBox, // intentionally pessimistic — the seeding fix narrows it
        }],
        ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
            let mut a = AttrDict::new();
            a.insert("value".into(), AttrValue::Bool(true));
            a
        })],
        terminator: Terminator::CondBranch {
            cond,
            then_block: body_id,
            then_args: vec![],
            else_block: exit_id,
            else_args: vec![],
        },
    };
    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![
            make_op(OpCode::ConstInt, vec![], vec![one], int_attr(1)),
            make_op(OpCode::Add, vec![i, one], vec![i_next], AttrDict::new()),
        ],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_next],
        },
    };
    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };

    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(header_id, header);
    blocks.insert(body_id, body);
    blocks.insert(exit_id, exit);

    let mut func = TirFunction {
        name: "iv_loop".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 5,
        next_block: 4,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    let _refined = refine_types(&mut func);

    let header_block = &func.blocks[&header_id];
    let i_arg_ty = header_block
        .args
        .iter()
        .find(|a| a.id == i)
        .map(|a| a.ty.clone())
        .expect("loop header arg `i` present");
    assert_eq!(
        i_arg_ty,
        TirType::I64,
        "loop induction variable seeded with entry-edge I64 must converge to I64, got {:?}",
        i_arg_ty
    );
}

#[test]
fn loop_iv_seed_widens_to_dynbox_when_backedge_is_dynamic() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let body_id = BlockId(2);
    let exit_id = BlockId(3);

    let i_init = ValueId(0);
    let i = ValueId(1);
    let cond = ValueId(2);
    let i_next = ValueId(3);

    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_init],
        },
    };
    let header = TirBlock {
        id: header_id,
        args: vec![TirValue {
            id: i,
            ty: TirType::DynBox,
        }],
        ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
            let mut a = AttrDict::new();
            a.insert("value".into(), AttrValue::Bool(true));
            a
        })],
        terminator: Terminator::CondBranch {
            cond,
            then_block: body_id,
            then_args: vec![],
            else_block: exit_id,
            else_args: vec![],
        },
    };
    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::Call,
            vec![i],
            vec![i_next],
            AttrDict::new(),
        )],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_next],
        },
    };
    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };

    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(header_id, header);
    blocks.insert(body_id, body);
    blocks.insert(exit_id, exit);

    let mut func = TirFunction {
        name: "iv_loop_dynamic_backedge".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 4,
        next_block: 4,
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
    let first = func.value_types.clone();
    assert_eq!(
        func.blocks[&header_id].args[0].ty,
        TirType::DynBox,
        "entry-edge I64 seed must widen when the reachable back-edge is dynamic"
    );
    assert_eq!(
        first.get(&i_next),
        Some(&TirType::DynBox),
        "dynamic back-edge producer must publish top, not stay at bottom"
    );

    refine_types(&mut func);
    assert_eq!(
        func.value_types, first,
        "loop-carried dynamic widening must be stable, not an oscillation fallback"
    );
}

#[test]
fn unreachable_loop_end_edge_does_not_widen_reachable_loop_arg() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let body_id = BlockId(2);
    let exit_id = BlockId(3);
    let dead_loop_end_id = BlockId(4);

    let i_init = ValueId(0);
    let i = ValueId(1);
    let cond = ValueId(2);
    let one = ValueId(3);
    let i_next = ValueId(4);
    let dead_none = ValueId(5);

    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: vec![make_op(OpCode::ConstInt, vec![], vec![i_init], int_attr(0))],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_init],
        },
    };
    let header = TirBlock {
        id: header_id,
        args: vec![TirValue {
            id: i,
            ty: TirType::DynBox,
        }],
        ops: vec![make_op(OpCode::ConstBool, vec![], vec![cond], {
            let mut a = AttrDict::new();
            a.insert("value".into(), AttrValue::Bool(true));
            a
        })],
        terminator: Terminator::CondBranch {
            cond,
            then_block: body_id,
            then_args: vec![],
            else_block: exit_id,
            else_args: vec![],
        },
    };
    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![
            make_op(OpCode::ConstInt, vec![], vec![one], int_attr(1)),
            make_op(OpCode::Add, vec![i, one], vec![i_next], AttrDict::new()),
        ],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![i_next],
        },
    };
    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
    let dead_loop_end = TirBlock {
        id: dead_loop_end_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::ConstNone,
            vec![],
            vec![dead_none],
            AttrDict::new(),
        )],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![dead_none],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(header_id, header);
    blocks.insert(body_id, body);
    blocks.insert(exit_id, exit);
    blocks.insert(dead_loop_end_id, dead_loop_end);

    let mut func = TirFunction {
        name: "unreachable_loop_end_meet".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 6,
        next_block: 5,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::from([(dead_loop_end_id, LoopRole::LoopEnd)]),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    refine_types(&mut func);

    let header_block = &func.blocks[&header_id];
    let i_arg_ty = header_block
        .args
        .iter()
        .find(|a| a.id == i)
        .map(|a| a.ty.clone())
        .expect("loop header arg `i` present");
    assert_eq!(
        i_arg_ty,
        TirType::I64,
        "unreachable loop-end incoming values must not widen reachable loop-carried types"
    );
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

// ---- Guard propagation tests ----

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

#[test]
fn list_index_refines_to_element_type() {
    let list = ValueId(0);
    let index = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![list, index],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types
        .insert(list, TirType::List(Box::new(TirType::Bool)));
    func.value_types.insert(index, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::Bool));
    assert_eq!(
        func.value_types.get(&item),
        Some(&TirType::Bool),
        "refine_types must persist list element facts for backend plans"
    );
}

#[test]
fn list_index_with_non_integer_index_stays_dynbox() {
    let list = ValueId(0);
    let index = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![list, index],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types
        .insert(list, TirType::List(Box::new(TirType::Bool)));
    func.value_types.insert(index, TirType::Str);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::DynBox));
}

#[test]
fn str_index_refines_to_str_for_integer_indices() {
    for index_ty in [TirType::I64, TirType::Bool] {
        let value = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![value, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(value, TirType::Str);
        func.value_types.insert(index, index_ty.clone());

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&item),
            Some(&TirType::Str),
            "str indexed by {index_ty:?} should refine to Str"
        );
    }
}

#[test]
fn bytes_index_refines_to_i64_for_integer_indices() {
    for index_ty in [TirType::I64, TirType::Bool] {
        let value = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![value, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(value, TirType::Bytes);
        func.value_types.insert(index, index_ty.clone());

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&item),
            Some(&TirType::I64),
            "bytes indexed by {index_ty:?} should refine to I64"
        );
    }
}

#[test]
fn immutable_sequence_index_with_non_integer_index_stays_dynbox() {
    for value_ty in [TirType::Str, TirType::Bytes] {
        let value = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![value, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(value, value_ty.clone());
        func.value_types.insert(index, TirType::Str);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&item),
            Some(&TirType::DynBox),
            "{value_ty:?} indexed by Str must stay conservative"
        );
    }
}

#[test]
fn tuple_index_refines_homogeneous_element_type() {
    let tuple = ValueId(0);
    let index = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![tuple, index],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types
        .insert(tuple, TirType::Tuple(vec![TirType::Str, TirType::Str]));
    func.value_types.insert(index, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::Str));
}

#[test]
fn tuple_index_refines_to_element_join_for_mixed_tuple() {
    let tuple = ValueId(0);
    let index = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![tuple, index],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(
        tuple,
        TirType::Tuple(vec![TirType::I64, TirType::Str, TirType::I64]),
    );
    func.value_types.insert(index, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&item),
        Some(&TirType::Union(vec![TirType::I64, TirType::Str]))
    );
}

#[test]
fn dict_index_refines_matching_key_to_value_type() {
    let dict = ValueId(0);
    let key = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![dict, key],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(
        dict,
        TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
    );
    func.value_types.insert(key, TirType::Str);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::I64));
}

#[test]
fn dict_index_with_nonmatching_key_stays_dynbox() {
    let dict = ValueId(0);
    let key = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![dict, key],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(
        dict,
        TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
    );
    func.value_types.insert(key, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::DynBox));
}

#[test]
fn builtin_len_return_refines_to_i64_without_transport_hint() {
    let list = ValueId(0);
    let result = ValueId(1);
    let mut attrs = AttrDict::new();
    attrs.insert("name".into(), AttrValue::Str("len".into()));
    let ops = vec![make_op(
        OpCode::CallBuiltin,
        vec![list],
        vec![result],
        attrs,
    )];
    let mut func = single_block_func(ops, 2);
    func.value_types
        .insert(list, TirType::List(Box::new(TirType::DynBox)));

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&result), Some(&TirType::I64));
}

#[test]
fn builtin_predicate_returns_refine_to_bool() {
    for name in ["bool", "hasattr", "isinstance", "issubclass"] {
        let value = ValueId(0);
        let result = ValueId(1);
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str(name.into()));
        let ops = vec![make_op(
            OpCode::CallBuiltin,
            vec![value],
            vec![result],
            attrs,
        )];
        let mut func = single_block_func(ops, 2);
        func.value_types.insert(value, TirType::DynBox);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&result),
            Some(&TirType::Bool),
            "call_builtin {name} should refine to Bool"
        );
    }
}

#[test]
fn builtin_ord_and_chr_return_types_refine() {
    let value = ValueId(0);
    let ord_result = ValueId(1);
    let chr_result = ValueId(2);
    let mut ord_attrs = AttrDict::new();
    ord_attrs.insert("name".into(), AttrValue::Str("ord".into()));
    let mut chr_attrs = AttrDict::new();
    chr_attrs.insert("name".into(), AttrValue::Str("chr".into()));
    let ops = vec![
        make_op(
            OpCode::CallBuiltin,
            vec![value],
            vec![ord_result],
            ord_attrs,
        ),
        make_op(
            OpCode::CallBuiltin,
            vec![ord_result],
            vec![chr_result],
            chr_attrs,
        ),
    ];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(value, TirType::Str);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&ord_result), Some(&TirType::I64));
    assert_eq!(type_map.get(&chr_result), Some(&TirType::Str));
}

#[test]
fn ord_at_return_type_refines_to_i64() {
    let text = ValueId(0);
    let index = ValueId(1);
    let result = ValueId(2);
    let ops = vec![make_op(
        OpCode::OrdAt,
        vec![text, index],
        vec![result],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(text, TirType::Str);
    func.value_types.insert(index, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&result), Some(&TirType::I64));
}

#[test]
fn unknown_builtin_return_stays_dynbox() {
    let value = ValueId(0);
    let result = ValueId(1);
    let mut attrs = AttrDict::new();
    attrs.insert("name".into(), AttrValue::Str("dynamic_builtin".into()));
    let ops = vec![make_op(
        OpCode::CallBuiltin,
        vec![value],
        vec![result],
        attrs,
    )];
    let mut func = single_block_func(ops, 2);
    func.value_types.insert(value, TirType::DynBox);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&result), Some(&TirType::DynBox));
}

#[test]
fn iter_next_unboxed_done_flag_refines_to_bool() {
    let iter = ValueId(0);
    let elem = ValueId(1);
    let done = ValueId(2);
    let ops = vec![make_op(
        OpCode::IterNextUnboxed,
        vec![iter],
        vec![elem, done],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(iter, TirType::DynBox);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&elem),
        Some(&TirType::DynBox),
        "iterator element stays conservative until iterator element provenance is represented"
    );
    assert_eq!(type_map.get(&done), Some(&TirType::Bool));
    assert_eq!(
        func.value_types.get(&done),
        Some(&TirType::Bool),
        "refine_types must persist multi-result done-flag facts"
    );
}

#[test]
fn get_iter_refines_known_iterable_element_types() {
    let cases = [
        (
            TirType::List(Box::new(TirType::I64)),
            TirType::Iterator(Box::new(TirType::I64)),
        ),
        (
            TirType::Set(Box::new(TirType::Str)),
            TirType::Iterator(Box::new(TirType::Str)),
        ),
        (
            TirType::Tuple(vec![TirType::I64, TirType::Str]),
            TirType::Iterator(Box::new(TirType::Union(vec![TirType::I64, TirType::Str]))),
        ),
        (
            TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
            TirType::Iterator(Box::new(TirType::Str)),
        ),
        (TirType::Str, TirType::Iterator(Box::new(TirType::Str))),
        (TirType::Bytes, TirType::Iterator(Box::new(TirType::I64))),
    ];

    for (iterable_ty, expected_iter_ty) in cases {
        let iterable = ValueId(0);
        let iter = ValueId(1);
        let ops = vec![make_op(
            OpCode::GetIter,
            vec![iterable],
            vec![iter],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 2);
        func.value_types.insert(iterable, iterable_ty.clone());

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&iter),
            Some(&expected_iter_ty),
            "GetIter({iterable_ty:?}) should refine to {expected_iter_ty:?}"
        );
    }
}

#[test]
fn iterator_consumers_refine_element_types() {
    let iter = ValueId(0);
    let iter_next_elem = ValueId(1);
    let unboxed_elem = ValueId(2);
    let done = ValueId(3);
    let for_iter_elem = ValueId(4);
    let ops = vec![
        make_op(
            OpCode::IterNext,
            vec![iter],
            vec![iter_next_elem],
            AttrDict::new(),
        ),
        make_op(
            OpCode::IterNextUnboxed,
            vec![iter],
            vec![unboxed_elem, done],
            AttrDict::new(),
        ),
        make_op(
            OpCode::ForIter,
            vec![iter],
            vec![for_iter_elem],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 5);
    func.value_types
        .insert(iter, TirType::Iterator(Box::new(TirType::I64)));

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&iter_next_elem), Some(&TirType::I64));
    assert_eq!(type_map.get(&unboxed_elem), Some(&TirType::I64));
    assert_eq!(type_map.get(&done), Some(&TirType::Bool));
    assert_eq!(type_map.get(&for_iter_elem), Some(&TirType::I64));
}

#[test]
fn iter_next_unboxed_done_flag_not_proven_without_proven_iterator() {
    let iter = ValueId(0);
    let elem = ValueId(1);
    let done = ValueId(2);
    let ops = vec![make_op(
        OpCode::IterNextUnboxed,
        vec![iter],
        vec![elem, done],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(iter, TirType::DynBox);

    refine_types(&mut func);
    let proven = extract_proven_map(&func);

    assert_eq!(proven.get(&elem), None);
    assert_eq!(
        proven.get(&done),
        None,
        "done flag type is inferred but not proven unless the iterator operand is proven"
    );
}

// ---- Test: parse_guard_type handles various type strings ----
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

// ---- Test: parse_return_type_str routes through TirType::from_type_hint ----
/// Pin the contract that `parse_return_type_str` uses the
/// centralized `TirType::from_type_hint` helper, so any future
/// hint added there (e.g. richer `Func:<sig>` parsing) is
/// automatically picked up by the type-refine seeding path.
/// Builtin scalars + None / NoneType keep their existing
/// behavior; containers + BigInt + user classes are newly
/// refined (previously returned None and stayed DynBox).
#[test]
fn parse_return_type_str_uses_centralized_helper() {
    // Existing builtin-scalar contracts (preserved).
    assert_eq!(parse_return_type_str("int"), Some(TirType::I64));
    assert_eq!(parse_return_type_str("float"), Some(TirType::F64));
    assert_eq!(parse_return_type_str("bool"), Some(TirType::Bool));
    assert_eq!(parse_return_type_str("str"), Some(TirType::Str));
    assert_eq!(parse_return_type_str("bytes"), Some(TirType::Bytes));
    assert_eq!(parse_return_type_str("None"), Some(TirType::None));
    assert_eq!(parse_return_type_str("NoneType"), Some(TirType::None));

    // Newly refined container/special types.
    assert_eq!(
        parse_return_type_str("list"),
        Some(TirType::List(Box::new(TirType::DynBox))),
        "method returning `list` must seed type-refine with \
         List(DynBox), not DynBox — otherwise lane inference \
         never sees the container type"
    );
    assert_eq!(
        parse_return_type_str("dict"),
        Some(TirType::Dict(
            Box::new(TirType::DynBox),
            Box::new(TirType::DynBox)
        ))
    );
    assert_eq!(
        parse_return_type_str("set"),
        Some(TirType::Set(Box::new(TirType::DynBox)))
    );
    assert_eq!(
        parse_return_type_str("tuple"),
        Some(TirType::Tuple(Vec::new()))
    );
    assert_eq!(parse_return_type_str("BigInt"), Some(TirType::BigInt));

    // User-class refinement: the live use of TirType::UserClass
    // through the type-refine seeding path.
    assert_eq!(
        parse_return_type_str("Point"),
        Some(TirType::UserClass("Point".into())),
        "method returning a user class must propagate UserClass \
         through type-refine — enables direct dispatch / \
         escape analysis precision on the result of factory \
         methods"
    );
    assert_eq!(
        parse_return_type_str("MyDataClass"),
        Some(TirType::UserClass("MyDataClass".into()))
    );

    // Structured compound containers refine through the same helper.
    assert_eq!(
        parse_return_type_str("list[int]"),
        Some(TirType::List(Box::new(TirType::I64)))
    );
    assert_eq!(
        parse_return_type_str("dict[str, list[float]]"),
        Some(TirType::Dict(
            Box::new(TirType::Str),
            Box::new(TirType::List(Box::new(TirType::F64)))
        ))
    );

    // Dynamic / malformed / unknown hints fall through to None so the
    // caller's operand-based inference takes over (rather than
    // forcing DynBox).
    assert_eq!(parse_return_type_str("Any"), None);
    assert_eq!(parse_return_type_str("Unknown"), None);
    assert_eq!(parse_return_type_str(""), None);
    assert_eq!(parse_return_type_str("Func:foo"), None);
    assert_eq!(parse_return_type_str("BoundMethod:list:append"), None);
    assert_eq!(parse_return_type_str("list[]"), None);
    assert_eq!(parse_return_type_str("list[Any]"), None);
}

#[test]
fn object_new_bound_type_hint_is_structural_class_result_type() {
    let result = ValueId(0);
    let mut attrs = AttrDict::new();
    attrs.insert("_type_hint".into(), AttrValue::Str("Point".into()));
    let mut func = single_block_func(
        vec![make_op(OpCode::ObjectNewBound, vec![], vec![result], attrs)],
        1,
    );
    func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&result),
        Some(&TirType::UserClass("Point".into())),
        "object_new_bound _type_hint is the structural class-id contract, not legacy scalar transport",
    );
}

#[test]
fn legacy_type_hint_does_not_refine_call_return_type() {
    let result = ValueId(0);
    let mut attrs = AttrDict::new();
    attrs.insert("_type_hint".into(), AttrValue::Str("int".into()));
    let mut func = single_block_func(
        vec![make_op(OpCode::CallMethod, vec![], vec![result], attrs)],
        1,
    );
    func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&result),
        Some(&TirType::DynBox),
        "legacy SimpleIR `_type_hint` must remain semantic transport metadata, not call-return proof",
    );
}

#[test]
fn structural_return_type_refines_call_return_type() {
    let result = ValueId(0);
    let mut attrs = AttrDict::new();
    attrs.insert("return_type".into(), AttrValue::Str("int".into()));
    attrs.insert("_type_hint".into(), AttrValue::Str("str".into()));
    let mut func = single_block_func(
        vec![make_op(OpCode::CallMethod, vec![], vec![result], attrs)],
        1,
    );
    func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&result),
        Some(&TirType::I64),
        "explicit structural return_type remains the call-return refinement contract",
    );
}
