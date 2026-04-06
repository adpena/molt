use std::collections::HashMap;

use super::blocks::{BlockId, Terminator, TirBlock};
use super::function::TirFunction;
use super::ops::OpCode;
use super::types::TirType;
use super::values::ValueId;

/// Maximum number of fixpoint iterations before conservative fallback.
const MAX_ROUNDS: usize = 20;

/// Extract a map from every [`ValueId`] to its refined [`TirType`] in a
/// **post-refinement** TIR function.  Block argument types come from the
/// function directly (they were written back by [`refine_types`]); op result
/// types are re-inferred in a single forward pass (safe because refinement
/// has already converged).
pub fn extract_type_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let mut env: HashMap<ValueId, TirType> = HashMap::new();

    // Sorted block order for deterministic iteration.
    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    for &bid in &block_order {
        let block = &func.blocks[&bid];

        // Block arguments already carry refined types.
        for arg in &block.args {
            env.insert(arg.id, arg.ty.clone());
        }

        // Re-infer op result types from operand types (single pass — the
        // fixpoint has already converged so one pass is sufficient).
        for op in &block.ops {
            if op.results.is_empty() {
                continue;
            }
            let operand_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| env.get(id).cloned().unwrap_or(TirType::DynBox))
                .collect();
            if let Some(inferred) = infer_result_type(op.opcode, &operand_types) {
                for &result_id in &op.results {
                    env.insert(result_id, inferred.clone());
                }
            } else {
                // No inference possible — record DynBox so the map is complete.
                for &result_id in &op.results {
                    env.entry(result_id).or_insert(TirType::DynBox);
                }
            }
        }
    }

    env
}

/// Refine types in a TIR function.
/// Iterates to fixpoint (max 20 rounds, conservative fallback on timeout).
/// Returns the number of values refined from DynBox to concrete types.
pub fn refine_types(func: &mut TirFunction) -> usize {
    // Build the type environment from existing value types.
    let mut env: HashMap<ValueId, TirType> = HashMap::new();

    // Collect initial types from block args and op results.
    for block in func.blocks.values() {
        for arg in &block.args {
            env.insert(arg.id, arg.ty.clone());
        }
        for op in &block.ops {
            for &result_id in &op.results {
                // Check if we already have a type from the value declarations;
                // if not, start as DynBox.
                env.entry(result_id).or_insert(TirType::DynBox);
            }
        }
    }

    // Track which values started as DynBox so we can count refinements.
    let initially_dynbox: Vec<ValueId> = env
        .iter()
        .filter(|(_, ty)| matches!(ty, TirType::DynBox))
        .map(|(id, _)| *id)
        .collect();

    // Sorted block order for deterministic iteration.
    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    // Pre-compute: for each block, collect all incoming edges (predecessor
    // block → arg values). We accumulate across all blocks' terminators.
    // Key: target BlockId, Value: list of incoming arg value lists.
    let mut incoming_edges: HashMap<BlockId, Vec<Vec<ValueId>>> = HashMap::new();
    for block in func.blocks.values() {
        let edges = collect_branch_edges(block);
        for (target_id, arg_values) in edges {
            incoming_edges
                .entry(target_id)
                .or_default()
                .push(arg_values);
        }
    }

    // Pre-compute op snapshots once (ops don't change during refinement,
    // only the type environment does). Avoids O(ops × rounds) Vec allocations.
    let ops_by_block: HashMap<BlockId, Vec<(OpCode, Vec<ValueId>, Vec<ValueId>)>> = block_order
        .iter()
        .map(|&bid| {
            let ops = func.blocks[&bid]
                .ops
                .iter()
                .map(|op| (op.opcode, op.operands.clone(), op.results.clone()))
                .collect();
            (bid, ops)
        })
        .collect();

    // When exception handling is present, identify blocks that start with
    // StateBlockStart (exception handler entry points). Block arguments of
    // these blocks should stay DynBox — the exception may come from any
    // type context, so propagating a refined type would be unsound.
    let has_eh = func.has_exception_handling;
    let mut eh_handler_args: std::collections::HashSet<ValueId> = std::collections::HashSet::new();
    if has_eh {
        for block in func.blocks.values() {
            // A block whose first op is StateBlockStart or CheckException
            // is an exception handler — its args must stay DynBox.
            if let Some(first_op) = block.ops.first()
                && matches!(
                    first_op.opcode,
                    OpCode::StateBlockStart | OpCode::CheckException
                )
            {
                for arg in &block.args {
                    eh_handler_args.insert(arg.id);
                }
            }
        }
    }

    // Fixpoint iteration.
    for _round in 0..MAX_ROUNDS {
        let mut changed = false;

        for &block_id in &block_order {
            let ops_snapshot = &ops_by_block[&block_id];

            for (opcode, operands, results) in ops_snapshot {
                if results.is_empty() {
                    continue;
                }

                // Do not refine results of CheckException — the value
                // coming out of an exception check is dynamically typed.
                if has_eh && matches!(opcode, OpCode::CheckException) {
                    for &result_id in results {
                        if !matches!(env.get(&result_id), Some(TirType::DynBox)) {
                            env.insert(result_id, TirType::DynBox);
                            changed = true;
                        }
                    }
                    continue;
                }

                let operand_types: Vec<TirType> = operands
                    .iter()
                    .map(|id| env.get(id).cloned().unwrap_or(TirType::DynBox))
                    .collect();

                let inferred = infer_result_type(*opcode, &operand_types);

                // For ops with a single result (the common case).
                if results.len() == 1 {
                    let result_id = results[0];
                    if let Some(new_ty) = inferred {
                        let current = env.get(&result_id).cloned().unwrap_or(TirType::DynBox);
                        if new_ty != current {
                            env.insert(result_id, new_ty);
                            changed = true;
                        }
                    }
                }
            }

            // Recompute block argument types from all incoming edges.
            // Start from Never (bottom) and meet all incoming values.
            if let Some(edge_list) = incoming_edges.get(&block_id) {
                let arg_count = func.blocks[&block_id].args.len();
                for i in 0..arg_count {
                    let arg_id = func.blocks[&block_id].args[i].id;

                    // Exception handler block args must stay DynBox —
                    // the exception could come from any type context.
                    if eh_handler_args.contains(&arg_id) {
                        if !matches!(env.get(&arg_id), Some(TirType::DynBox)) {
                            env.insert(arg_id, TirType::DynBox);
                            changed = true;
                        }
                        continue;
                    }

                    let mut accumulated = TirType::Never;
                    for edge_args in edge_list {
                        if i < edge_args.len() {
                            let incoming_ty =
                                env.get(&edge_args[i]).cloned().unwrap_or(TirType::DynBox);
                            accumulated = accumulated.meet(&incoming_ty);
                        }
                    }
                    // Only update if we actually had incoming edges and computed
                    // something other than Never.
                    if !matches!(accumulated, TirType::Never) {
                        let current = env.get(&arg_id).cloned().unwrap_or(TirType::DynBox);
                        if accumulated != current {
                            env.insert(arg_id, accumulated);
                            changed = true;
                        }
                    }
                }
            }
        }

        if !changed {
            break;
        }
    }

    // Write refined types back into the function.
    for block in func.blocks.values_mut() {
        for arg in &mut block.args {
            if let Some(ty) = env.get(&arg.id) {
                arg.ty = ty.clone();
            }
        }
        for op in &mut block.ops {
            for &result_id in &op.results {
                // We don't have TirValue in ops directly — the type lives in
                // the env. But we need to propagate back to anywhere types are
                // stored. For now, the env is the authoritative source and
                // downstream passes can query it. However, since the task says
                // "mutates TirFunction in place", we store types on block args
                // (done above). Op result types aren't stored on TirOp (they
                // only have ValueId). So the block args are the mutation target.
                let _ = result_id; // suppress unused warning
            }
        }
    }

    // Count refinements: values that started as DynBox and are now concrete.
    initially_dynbox
        .iter()
        .filter(|id| {
            env.get(id)
                .map(|ty| !matches!(ty, TirType::DynBox))
                .unwrap_or(false)
        })
        .count()
}

/// Infer the result type of an operation from its operand types.
/// Returns `None` if the result type cannot be determined (stays as-is).
fn infer_result_type(opcode: OpCode, operand_types: &[TirType]) -> Option<TirType> {
    match opcode {
        // Constants — always produce a known type regardless of operands.
        OpCode::ConstInt => Some(TirType::I64),
        OpCode::ConstFloat => Some(TirType::F64),
        OpCode::ConstStr => Some(TirType::Str),
        OpCode::ConstBool => Some(TirType::Bool),
        OpCode::ConstNone => Some(TirType::None),
        OpCode::ConstBytes => Some(TirType::Bytes),

        // Add: numeric arithmetic + string concatenation + string/list repetition
        OpCode::Add => match operand_types {
            [TirType::Str, TirType::Str] => Some(TirType::Str), // "a" + "b"
            _ => infer_numeric_arithmetic(operand_types),
        },
        // Mul: numeric arithmetic + string/list repetition (str * int, int * str)
        OpCode::Mul => match operand_types {
            [TirType::Str, TirType::I64] | [TirType::I64, TirType::Str] => Some(TirType::Str),
            _ => infer_numeric_arithmetic(operand_types),
        },
        // Sub, Mod, Pow: numeric only (str-str is TypeError in Python)
        OpCode::Sub | OpCode::Mod | OpCode::Pow => infer_numeric_arithmetic(operand_types),
        OpCode::Div => {
            // Python: division always produces float unless both are DynBox.
            match operand_types {
                [TirType::I64, TirType::I64]
                | [TirType::F64, TirType::F64]
                | [TirType::I64, TirType::F64]
                | [TirType::F64, TirType::I64] => Some(TirType::F64),
                _ => infer_numeric_arithmetic(operand_types),
            }
        }
        OpCode::FloorDiv => infer_numeric_arithmetic(operand_types),

        // Unary Neg/Pos
        OpCode::Neg | OpCode::Pos => match operand_types {
            [TirType::I64] => Some(TirType::I64),
            [TirType::F64] => Some(TirType::F64),
            _ => None,
        },

        // Comparisons always produce Bool.
        OpCode::Eq
        | OpCode::Ne
        | OpCode::Lt
        | OpCode::Le
        | OpCode::Gt
        | OpCode::Ge
        | OpCode::Is
        | OpCode::IsNot
        | OpCode::In
        | OpCode::NotIn => Some(TirType::Bool),

        // Boolean ops
        OpCode::And | OpCode::Or => match operand_types {
            [TirType::Bool, TirType::Bool] => Some(TirType::Bool),
            _ => None,
        },
        OpCode::Not => Some(TirType::Bool),

        // Bitwise (I64 only)
        OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor | OpCode::Shl | OpCode::Shr => {
            match operand_types {
                [TirType::I64, TirType::I64] => Some(TirType::I64),
                _ => None,
            }
        }
        OpCode::BitNot => match operand_types {
            [TirType::I64] => Some(TirType::I64),
            _ => None,
        },

        // Containers
        OpCode::BuildList => Some(TirType::List(Box::new(TirType::DynBox))),
        OpCode::BuildDict => Some(TirType::Dict(
            Box::new(TirType::DynBox),
            Box::new(TirType::DynBox),
        )),
        OpCode::BuildSet => Some(TirType::Set(Box::new(TirType::DynBox))),
        OpCode::BuildTuple => Some(TirType::Tuple(operand_types.to_vec())),

        // Copy propagates type.
        OpCode::Copy => operand_types.first().cloned(),

        // Box/Unbox
        OpCode::BoxVal => operand_types
            .first()
            .map(|t| TirType::Box(Box::new(t.clone()))),
        OpCode::UnboxVal => match operand_types.first() {
            Some(TirType::Box(inner)) => Some(inner.as_ref().clone()),
            _ => None,
        },

        // Everything else: cannot infer, leave as-is.
        _ => None,
    }
}

/// Infer the result type of a numeric-only binary operation.
/// Does NOT handle string concatenation or repetition — those are handled
/// at the opcode level (Add for concat, Mul for repetition).
fn infer_numeric_arithmetic(operand_types: &[TirType]) -> Option<TirType> {
    match operand_types {
        [TirType::I64, TirType::I64] => Some(TirType::I64),
        [TirType::F64, TirType::F64] => Some(TirType::F64),
        // Python numeric promotion: int op float → float
        [TirType::I64, TirType::F64] | [TirType::F64, TirType::I64] => Some(TirType::F64),
        _ => None,
    }
}

/// Collect (target_block, arg_values) edges from a terminator.
fn collect_branch_edges(block: &TirBlock) -> Vec<(BlockId, Vec<ValueId>)> {
    match &block.terminator {
        Terminator::Branch { target, args } => {
            vec![(*target, args.clone())]
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            vec![
                (*then_block, then_args.clone()),
                (*else_block, else_args.clone()),
            ]
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut edges: Vec<(BlockId, Vec<ValueId>)> = cases
                .iter()
                .map(|(_, target, args)| (*target, args.clone()))
                .collect();
            edges.push((*default, default_args.clone()));
            edges
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

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
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
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
            make_op(
                OpCode::ConstFloat,
                vec![],
                vec![ValueId(1)],
                float_attr(PI),
            ),
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
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
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
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
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
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
        };
        let refined = refine_types(&mut func);
        assert_eq!(refined, 0);
    }
}
