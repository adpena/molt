use std::collections::HashMap;

use super::blocks::{BlockId, Terminator, TirBlock};
use super::function::TirFunction;
use super::ops::{AttrDict, AttrValue, OpCode};
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
            if let Some(inferred) = infer_result_type(op.opcode, &operand_types, &op.attrs) {
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
    let ops_by_block: HashMap<BlockId, Vec<(OpCode, Vec<ValueId>, Vec<ValueId>, AttrDict)>> =
        block_order
            .iter()
            .map(|&bid| {
                let ops = func.blocks[&bid]
                    .ops
                    .iter()
                    .map(|op| {
                        (
                            op.opcode,
                            op.operands.clone(),
                            op.results.clone(),
                            op.attrs.clone(),
                        )
                    })
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

    // Side map: for iterator-producing values, tracks the element type
    // that downstream IterNext/ForIter/IterNextUnboxed should yield.
    // Populated by CallBuiltin("range") → I64, GetIter on List(T) → T, etc.
    let mut iter_element_types: HashMap<ValueId, TirType> = HashMap::new();

    // Fixpoint iteration.
    for _round in 0..MAX_ROUNDS {
        let mut changed = false;

        for &block_id in &block_order {
            let ops_snapshot = &ops_by_block[&block_id];

            for (opcode, operands, results, attrs) in ops_snapshot {
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

                let inferred = infer_result_type(*opcode, &operand_types, attrs);

                // IterNextUnboxed has two results: [0]=element, [1]=Bool.
                // Handle multi-result ops before the single-result fast path.
                if *opcode == OpCode::IterNextUnboxed && results.len() == 2 {
                    // results[1] is always the done-flag (Bool).
                    let flag_id = results[1];
                    let current_flag =
                        env.get(&flag_id).cloned().unwrap_or(TirType::DynBox);
                    if current_flag != TirType::Bool {
                        env.insert(flag_id, TirType::Bool);
                        changed = true;
                    }
                    // results[0] gets the element type from iter_element_types
                    // or the general inferred type.
                    let elem_id = results[0];
                    // Trace the iterator operand to see if we know its element type.
                    let elem_ty = operands
                        .first()
                        .and_then(|iter_val| iter_element_types.get(iter_val))
                        .cloned()
                        .or(inferred);
                    if let Some(ty) = elem_ty {
                        let current = env.get(&elem_id).cloned().unwrap_or(TirType::DynBox);
                        if ty != current {
                            env.insert(elem_id, ty);
                            changed = true;
                        }
                    }
                    continue;
                }

                // For ops with a single result (the common case).
                if results.len() == 1 {
                    let result_id = results[0];

                    // For GetIter: record the element type of the produced
                    // iterator in the side map so downstream IterNext/ForIter
                    // can resolve element types.
                    if *opcode == OpCode::GetIter {
                        if let Some(elem_ty) =
                            infer_iter_element_type(&operand_types, &iter_element_types, operands)
                        {
                            let prev = iter_element_types.get(&result_id);
                            if prev != Some(&elem_ty) {
                                iter_element_types.insert(result_id, elem_ty);
                                changed = true;
                            }
                        }
                    }

                    // For CallBuiltin("range"): record that the result,
                    // when iterated, yields I64.
                    if *opcode == OpCode::CallBuiltin {
                        if let Some(AttrValue::Str(name)) = attrs.get("name") {
                            if name == "range"
                                || name == "molt_range"
                                || name == "builtin_range"
                            {
                                let prev = iter_element_types.get(&result_id);
                                if prev != Some(&TirType::I64) {
                                    iter_element_types.insert(result_id, TirType::I64);
                                    changed = true;
                                }
                            }
                        }
                    }

                    // For ForIter/IterNext: resolve element type from the
                    // iterator source if available.
                    let final_ty = if matches!(
                        opcode,
                        OpCode::ForIter | OpCode::IterNext
                    ) {
                        operands
                            .first()
                            .and_then(|iter_val| iter_element_types.get(iter_val))
                            .cloned()
                            .or(inferred)
                    } else {
                        inferred
                    };

                    if let Some(new_ty) = final_ty {
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

/// Infer the element type yielded by an iterator produced by `GetIter`.
///
/// Given the operand types of a `GetIter` op, determine what element type
/// the resulting iterator yields.  Also consults the `iter_element_types`
/// side map for cases where the source value was itself annotated (e.g.,
/// a `CallBuiltin("range")` result).
fn infer_iter_element_type(
    operand_types: &[TirType],
    iter_element_types: &HashMap<ValueId, TirType>,
    operands: &[ValueId],
) -> Option<TirType> {
    // If the source value already has an element-type annotation (e.g.,
    // from CallBuiltin("range")), propagate it through GetIter.
    if let Some(src_id) = operands.first() {
        if let Some(elem_ty) = iter_element_types.get(src_id) {
            return Some(elem_ty.clone());
        }
    }

    // Structural: List(T) → T, Set(T) → T, Dict(K,V) → K, Tuple → DynBox,
    // Str → Str (iterating a string yields single-char strings).
    match operand_types.first() {
        Some(TirType::List(elem)) => Some(elem.as_ref().clone()),
        Some(TirType::Set(elem)) => Some(elem.as_ref().clone()),
        Some(TirType::Dict(key, _)) => Some(key.as_ref().clone()),
        Some(TirType::Str) => Some(TirType::Str),
        Some(TirType::Bytes) => Some(TirType::I64), // iterating bytes yields ints
        Some(TirType::Tuple(elems)) => {
            // If all tuple elements are the same type, the iterator yields that type.
            if let Some(first) = elems.first() {
                if elems.iter().all(|t| t == first) {
                    return Some(first.clone());
                }
            }
            None
        }
        _ => None,
    }
}

/// Infer the result type of an operation from its operand types and attributes.
/// Returns `None` if the result type cannot be determined (stays as-is).
fn infer_result_type(opcode: OpCode, operand_types: &[TirType], attrs: &AttrDict) -> Option<TirType> {
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

        // In-place arithmetic: same rules as their non-in-place counterparts.
        OpCode::InplaceAdd => match operand_types {
            [TirType::Str, TirType::Str] => Some(TirType::Str),
            _ => infer_numeric_arithmetic(operand_types),
        },
        OpCode::InplaceMul => match operand_types {
            [TirType::Str, TirType::I64] | [TirType::I64, TirType::Str] => Some(TirType::Str),
            _ => infer_numeric_arithmetic(operand_types),
        },
        OpCode::InplaceSub => infer_numeric_arithmetic(operand_types),

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

        // CallBuiltin: infer return type from builtin name.
        OpCode::CallBuiltin => {
            if let Some(AttrValue::Str(name)) = attrs.get("name") {
                match name.as_str() {
                    // Type-casting builtins
                    "int" | "molt_int" => Some(TirType::I64),
                    "float" | "molt_float" => Some(TirType::F64),
                    "str" | "molt_str" => Some(TirType::Str),
                    "bool" | "molt_bool" => Some(TirType::Bool),
                    "bytes" | "molt_bytes" => Some(TirType::Bytes),
                    // Numeric builtins that always return int
                    "len" | "molt_len" | "hash" | "id" | "ord" => Some(TirType::I64),
                    // Numeric builtins that propagate numeric type
                    "abs" => match operand_types.first() {
                        Some(TirType::I64) => Some(TirType::I64),
                        Some(TirType::F64) => Some(TirType::F64),
                        _ => None,
                    },
                    // chr returns a string
                    "chr" => Some(TirType::Str),
                    // repr returns a string
                    "repr" => Some(TirType::Str),
                    // type returns DynBox (it's a type object)
                    // range returns a range object (element type tracked separately)
                    // min/max: propagate numeric type if both args match
                    "min" | "max" => infer_numeric_arithmetic(operand_types),
                    // round: int if no ndigits, float if ndigits given
                    "round" => match operand_types.len() {
                        1 => Some(TirType::I64),
                        _ => Some(TirType::F64),
                    },
                    // isinstance/issubclass/callable/hasattr always return bool
                    "isinstance" | "issubclass" | "callable" | "hasattr" => Some(TirType::Bool),
                    // sum: if start is I64 and iterable elements are I64, result is I64.
                    // Conservative: return I64 if all operands are I64.
                    "sum" => {
                        if operand_types.iter().all(|t| matches!(t, TirType::I64)) && !operand_types.is_empty() {
                            Some(TirType::I64)
                        } else {
                            None
                        }
                    }
                    // sorted always returns a list
                    "sorted" => Some(TirType::List(Box::new(TirType::DynBox))),
                    // list/tuple/set/dict constructors
                    "list" | "molt_list" => Some(TirType::List(Box::new(TirType::DynBox))),
                    "tuple" | "molt_tuple" => Some(TirType::Tuple(vec![])),
                    "set" | "molt_set" => Some(TirType::Set(Box::new(TirType::DynBox))),
                    "dict" | "molt_dict" => Some(TirType::Dict(
                        Box::new(TirType::DynBox),
                        Box::new(TirType::DynBox),
                    )),
                    _ => None,
                }
            } else {
                None
            }
        }

        // Iteration: GetIter/ForIter/IterNext element types are resolved in
        // refine_types via iter_element_types side map (not here). But we can
        // return None here — the refine_types loop overrides with the side map.
        // IterNextUnboxed is handled specially in the refine_types loop (two results).

        // Index: extracting from a container
        OpCode::Index => match operand_types.first() {
            Some(TirType::List(elem)) => Some(elem.as_ref().clone()),
            Some(TirType::Str) => Some(TirType::Str),
            Some(TirType::Bytes) => Some(TirType::I64),
            Some(TirType::Dict(_, val)) => Some(val.as_ref().clone()),
            Some(TirType::Tuple(elems)) => {
                // If all elements same type, indexing yields that type.
                if let Some(first) = elems.first() {
                    if elems.iter().all(|t| t == first) {
                        return Some(first.clone());
                    }
                }
                None
            }
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
                float_attr(3.14),
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
                    float_attr(3.14),
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

    // ---- Test 8: CallBuiltin("range") → GetIter → ForIter yields I64 ----
    #[test]
    fn range_iter_yields_i64() {
        // Simulates: for i in range(10): ...
        // v0 = ConstInt(10)
        // v1 = CallBuiltin("range", v0)
        // v2 = GetIter(v1)
        // v3 = ForIter(v2)  → should be I64
        let mut range_attrs = AttrDict::new();
        range_attrs.insert("name".into(), AttrValue::Str("range".into()));

        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(10)),
            make_op(
                OpCode::CallBuiltin,
                vec![ValueId(0)],
                vec![ValueId(1)],
                range_attrs,
            ),
            make_op(
                OpCode::GetIter,
                vec![ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::ForIter,
                vec![ValueId(2)],
                vec![ValueId(3)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 4);
        let refined = refine_types(&mut func);
        // v0=I64 (ConstInt), v1=DynBox→? (range result), v2=DynBox (iterator),
        // v3=DynBox→I64 (ForIter element)
        // At minimum v0 and v3 should be refined.
        assert!(refined >= 2, "expected at least 2 refinements, got {}", refined);

        let env = extract_type_map(&func);
        assert_eq!(
            env.get(&ValueId(3)),
            Some(&TirType::I64),
            "ForIter on range should yield I64"
        );
    }

    // ---- Test 9: IterNext on range iterator yields I64 ----
    #[test]
    fn range_iter_next_yields_i64() {
        let mut range_attrs = AttrDict::new();
        range_attrs.insert("name".into(), AttrValue::Str("range".into()));

        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(5)),
            make_op(
                OpCode::CallBuiltin,
                vec![ValueId(0)],
                vec![ValueId(1)],
                range_attrs,
            ),
            make_op(
                OpCode::GetIter,
                vec![ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::IterNext,
                vec![ValueId(2)],
                vec![ValueId(3)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 4);
        refine_types(&mut func);

        let env = extract_type_map(&func);
        assert_eq!(
            env.get(&ValueId(3)),
            Some(&TirType::I64),
            "IterNext on range should yield I64"
        );
    }

    // ---- Test 10: CallBuiltin("len") → I64 ----
    #[test]
    fn callbuiltin_len_returns_i64() {
        let mut len_attrs = AttrDict::new();
        len_attrs.insert("name".into(), AttrValue::Str("len".into()));

        let ops = vec![make_op(
            OpCode::CallBuiltin,
            vec![],
            vec![ValueId(0)],
            len_attrs,
        )];
        let mut func = single_block_func(ops, 1);
        let refined = refine_types(&mut func);
        assert_eq!(refined, 1);
    }

    // ---- Test 11: CallBuiltin("int") → I64 ----
    #[test]
    fn callbuiltin_int_returns_i64() {
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str("int".into()));

        let ops = vec![make_op(
            OpCode::CallBuiltin,
            vec![],
            vec![ValueId(0)],
            attrs,
        )];
        let mut func = single_block_func(ops, 1);
        refine_types(&mut func);

        let env = extract_type_map(&func);
        assert_eq!(env.get(&ValueId(0)), Some(&TirType::I64));
    }

    // ---- Test 12: CallBuiltin("float") → F64 ----
    #[test]
    fn callbuiltin_float_returns_f64() {
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str("float".into()));

        let ops = vec![make_op(
            OpCode::CallBuiltin,
            vec![],
            vec![ValueId(0)],
            attrs,
        )];
        let mut func = single_block_func(ops, 1);
        refine_types(&mut func);

        let env = extract_type_map(&func);
        assert_eq!(env.get(&ValueId(0)), Some(&TirType::F64));
    }

    // ---- Test 13: InplaceAdd propagates I64 ----
    #[test]
    fn inplace_add_propagates_i64() {
        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
            make_op(
                OpCode::InplaceAdd,
                vec![ValueId(0), ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 3);
        let refined = refine_types(&mut func);
        assert_eq!(refined, 3);
    }

    // ---- Test 14: IterNextUnboxed done-flag is Bool ----
    #[test]
    fn iter_next_unboxed_done_flag_is_bool() {
        let mut range_attrs = AttrDict::new();
        range_attrs.insert("name".into(), AttrValue::Str("range".into()));

        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(10)),
            make_op(
                OpCode::CallBuiltin,
                vec![ValueId(0)],
                vec![ValueId(1)],
                range_attrs,
            ),
            make_op(
                OpCode::GetIter,
                vec![ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
            // IterNextUnboxed: results[0]=element, results[1]=done_flag
            make_op(
                OpCode::IterNextUnboxed,
                vec![ValueId(2)],
                vec![ValueId(3), ValueId(4)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 5);
        refine_types(&mut func);

        let env = extract_type_map(&func);
        assert_eq!(
            env.get(&ValueId(3)),
            Some(&TirType::I64),
            "IterNextUnboxed element from range should be I64"
        );
        assert_eq!(
            env.get(&ValueId(4)),
            Some(&TirType::Bool),
            "IterNextUnboxed done-flag should be Bool"
        );
    }

    // ---- Test 15: GetIter on List(I64) yields I64 elements ----
    #[test]
    fn getiter_list_i64_yields_i64() {
        // Block arg typed as List(I64), then GetIter → ForIter.
        let entry_id = BlockId(0);
        let block = TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::List(Box::new(TirType::I64)),
            }],
            ops: vec![
                make_op(
                    OpCode::GetIter,
                    vec![ValueId(0)],
                    vec![ValueId(1)],
                    AttrDict::new(),
                ),
                make_op(
                    OpCode::ForIter,
                    vec![ValueId(1)],
                    vec![ValueId(2)],
                    AttrDict::new(),
                ),
            ],
            terminator: Terminator::Return {
                values: vec![ValueId(2)],
            },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, block);
        let mut func = TirFunction {
            name: "list_iter_test".into(),
            param_names: vec!["lst".into()],
            param_types: vec![TirType::List(Box::new(TirType::I64))],
            return_type: TirType::I64,
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
        refine_types(&mut func);

        let env = extract_type_map(&func);
        assert_eq!(
            env.get(&ValueId(2)),
            Some(&TirType::I64),
            "ForIter on List(I64) should yield I64"
        );
    }

    // ---- Test 16: Index on List(I64) returns I64 ----
    #[test]
    fn index_list_i64_returns_i64() {
        let entry_id = BlockId(0);
        let block = TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::List(Box::new(TirType::I64)),
            }],
            ops: vec![
                make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(0)),
                make_op(
                    OpCode::Index,
                    vec![ValueId(0), ValueId(1)],
                    vec![ValueId(2)],
                    AttrDict::new(),
                ),
            ],
            terminator: Terminator::Return {
                values: vec![ValueId(2)],
            },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, block);
        let mut func = TirFunction {
            name: "index_test".into(),
            param_names: vec!["lst".into()],
            param_types: vec![TirType::List(Box::new(TirType::I64))],
            return_type: TirType::I64,
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
        refine_types(&mut func);

        let env = extract_type_map(&func);
        assert_eq!(
            env.get(&ValueId(2)),
            Some(&TirType::I64),
            "Index on List(I64) should return I64"
        );
    }

    // ---- Test 17: Sieve pattern — range + arithmetic all resolve to I64 ----
    #[test]
    fn sieve_pattern_all_i64() {
        // Simulates the critical sieve path:
        // v0 = ConstInt(100)         → I64
        // v1 = CallBuiltin("range")  → range object
        // v2 = GetIter(v1)           → iterator
        // v3 = ForIter(v2)           → I64 (loop var i)
        // v4 = ConstInt(1)           → I64
        // v5 = Add(v3, v4)           → I64 (i + 1)
        // v6 = Lt(v3, v0)            → Bool (i < 100)
        let mut range_attrs = AttrDict::new();
        range_attrs.insert("name".into(), AttrValue::Str("range".into()));

        let ops = vec![
            make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(100)),
            make_op(
                OpCode::CallBuiltin,
                vec![ValueId(0)],
                vec![ValueId(1)],
                range_attrs,
            ),
            make_op(
                OpCode::GetIter,
                vec![ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::ForIter,
                vec![ValueId(2)],
                vec![ValueId(3)],
                AttrDict::new(),
            ),
            make_op(OpCode::ConstInt, vec![], vec![ValueId(4)], int_attr(1)),
            make_op(
                OpCode::Add,
                vec![ValueId(3), ValueId(4)],
                vec![ValueId(5)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::Lt,
                vec![ValueId(3), ValueId(0)],
                vec![ValueId(6)],
                AttrDict::new(),
            ),
        ];
        let mut func = single_block_func(ops, 7);
        refine_types(&mut func);

        let env = extract_type_map(&func);
        // All critical values should be typed.
        assert_eq!(env.get(&ValueId(0)), Some(&TirType::I64), "const 100");
        assert_eq!(env.get(&ValueId(3)), Some(&TirType::I64), "loop var i");
        assert_eq!(env.get(&ValueId(4)), Some(&TirType::I64), "const 1");
        assert_eq!(env.get(&ValueId(5)), Some(&TirType::I64), "i + 1");
        assert_eq!(env.get(&ValueId(6)), Some(&TirType::Bool), "i < 100");
    }
}
