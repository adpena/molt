//! Sparse Conditional Constant Propagation (SCCP).
//!
//! Propagates constants through the SSA graph, folds constant operations,
//! and eliminates branches with known-constant conditions.
//!
//! This is a simplified single-pass forward scan that folds obvious constants.
//! An iterative fixpoint version can replace it later.

use std::collections::{HashMap, HashSet};

use super::PassStats;
use super::effects;
use super::reachability::metadata_preserving_reachable_blocks;
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, OpCode};
use crate::tir::values::ValueId;

/// A value in the constant-propagation lattice.
#[derive(Debug, Clone, PartialEq)]
enum LatticeValue {
    /// Unknown — may still be constant (not yet visited).
    Top,
    /// Known constant value.
    Constant(ConstVal),
    /// Overdefined — definitely not constant.
    Bottom,
}

/// Concrete constant values carried through the lattice.
///
/// # NaN note
/// The derived `PartialEq` for `Float(f64)` uses `f64::eq`, which returns
/// `false` for NaN == NaN. In practice this only affects programs that fold
/// a constant NaN value -- an extremely rare case -- and the worst outcome is
/// a missed constant-fold (the lattice value stays Bottom rather than being
/// collapsed to a constant NaN). A future improvement would be to implement
/// `PartialEq` manually using `f64::to_bits()` for bit-exact NaN comparison.
#[derive(Debug, Clone, PartialEq)]
enum ConstVal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    None,
    /// Compile-time constant list (all elements are ConstVal).
    /// Capped at MAX_COMPOUND_ELEMENTS to avoid embedding huge data at compile time.
    List(Vec<ConstVal>),
    /// Compile-time constant dict (all keys and values are ConstVal).
    /// Capped at MAX_COMPOUND_ELEMENTS entries.
    Dict(Vec<(ConstVal, ConstVal)>),
    /// Compile-time range(start, stop, step). Not materialized as a list,
    /// but supports len() and iteration count propagation.
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
}

/// Maximum number of elements for compile-time compound value folding.
/// Prevents embedding excessively large data structures in the binary.
const MAX_COMPOUND_ELEMENTS: usize = 1000;

/// Returns `true` if the op may throw and thus should not be folded inside
/// a try region (its execution may transfer to an exception handler).
#[inline]
fn may_throw(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            | OpCode::Raise
            | OpCode::Index
            | OpCode::StoreIndex
            | OpCode::LoadAttr
            | OpCode::StoreAttr
            | OpCode::DelAttr
            | OpCode::DelIndex
            | OpCode::Import
            | OpCode::ImportFrom
            | OpCode::Div
            | OpCode::FloorDiv
            | OpCode::Mod
            | OpCode::GetIter
            | OpCode::IterNext
            | OpCode::IterNextUnboxed
            | OpCode::ForIter
            | OpCode::StateTransition
            | OpCode::StateYield
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            | OpCode::ClosureLoad
            | OpCode::ClosureStore
    )
}

/// Build a set of ValueIds that are results of ops inside try regions.
/// When `has_exception_handling` is true, we must not rewrite these ops
/// to constants because the op's execution may transfer control to a
/// handler, and removing the op would change observable behavior.
fn build_try_region_results(func: &TirFunction) -> HashSet<ValueId> {
    let mut result_set = HashSet::new();
    for block in func.blocks.values() {
        let mut try_depth: u32 = 0;
        for op in &block.ops {
            match op.opcode {
                OpCode::TryStart => try_depth += 1,
                OpCode::TryEnd => try_depth = try_depth.saturating_sub(1),
                _ => {}
            }
            if try_depth > 0 && may_throw(op.opcode) {
                for &r in &op.results {
                    result_set.insert(r);
                }
            }
        }
    }
    result_set
}

/// Run the SCCP pass on `func`, returning statistics.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "sccp",
        ..Default::default()
    };

    let has_eh = func.has_exception_handling;

    // Phase 1: Build the lattice from all existing ops.
    let mut lattice: HashMap<ValueId, LatticeValue> = HashMap::new();

    // Block arguments are Bottom (parameters / phi-like — not constant).
    for block in func.blocks.values() {
        for arg in &block.args {
            lattice.insert(arg.id, LatticeValue::Bottom);
        }
    }

    // When exception handling is present, mark results of potentially-throwing
    // ops inside try regions as Bottom (unfoldable) so SCCP never rewrites them.
    let try_region_results = if has_eh {
        build_try_region_results(func)
    } else {
        HashSet::new()
    };

    // Collect block ids for deterministic iteration (sorted).
    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    // First pass: seed constants from ConstInt/ConstFloat/ConstBool/ConstNone ops,
    // mark everything else as Top initially.
    // Results of potentially-throwing ops inside try regions are forced to Bottom.
    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            for &res in &op.results {
                // Loop-carried values (loop_index_start, loop_index_next, iter_next)
                // must not be folded — they change on each iteration.
                let original_kind = op
                    .attrs
                    .get("_original_kind")
                    .and_then(|v| {
                        if let AttrValue::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("");
                if matches!(
                    original_kind,
                    "loop_index_start" | "loop_index_next" | "iter_next"
                ) {
                    lattice.insert(res, LatticeValue::Bottom);
                    continue;
                }
                // If this result is inside a try region and may throw, force Bottom.
                if try_region_results.contains(&res) {
                    lattice.insert(res, LatticeValue::Bottom);
                    continue;
                }
                let val = match op.opcode {
                    OpCode::ConstInt => {
                        if let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                            LatticeValue::Constant(ConstVal::Int(*v))
                        } else {
                            LatticeValue::Bottom
                        }
                    }
                    OpCode::ConstFloat => {
                        if let Some(AttrValue::Float(v)) = op.attrs.get("f_value") {
                            LatticeValue::Constant(ConstVal::Float(*v))
                        } else {
                            LatticeValue::Bottom
                        }
                    }
                    OpCode::ConstBool => {
                        if let Some(AttrValue::Bool(v)) = op.attrs.get("value") {
                            LatticeValue::Constant(ConstVal::Bool(*v))
                        } else {
                            LatticeValue::Bottom
                        }
                    }
                    OpCode::ConstStr => {
                        if let Some(AttrValue::Str(v)) = op.attrs.get("s_value") {
                            LatticeValue::Constant(ConstVal::Str(v.clone()))
                        } else if let Some(AttrValue::Str(v)) = op.attrs.get("value") {
                            LatticeValue::Constant(ConstVal::Str(v.clone()))
                        } else {
                            LatticeValue::Bottom
                        }
                    }
                    OpCode::ConstNone => LatticeValue::Constant(ConstVal::None),
                    _ => LatticeValue::Top,
                };
                lattice.insert(res, val);
            }
        }
    }

    // Phase 2: Forward propagation — try to fold ops with all-constant operands.
    // Iterate until stable (bounded by number of values).
    let mut changed = true;
    while changed {
        changed = false;
        for &bid in &block_ids {
            let block = &func.blocks[&bid];
            for op in &block.ops {
                if op.results.is_empty() {
                    continue;
                }
                // Skip ops that are already resolved as Constant or Bottom.
                let result_id = op.results[0];
                match lattice.get(&result_id) {
                    Some(LatticeValue::Bottom) | Some(LatticeValue::Constant(_)) => continue,
                    _ => {}
                }

                // Gather operand lattice values.
                let operand_vals: Vec<Option<&ConstVal>> = op
                    .operands
                    .iter()
                    .map(|v| match lattice.get(v) {
                        Some(LatticeValue::Constant(c)) => Some(c),
                        _ => None,
                    })
                    .collect();

                // If any operand is Bottom, this result is Bottom.
                let any_bottom = op
                    .operands
                    .iter()
                    .any(|v| matches!(lattice.get(v), Some(LatticeValue::Bottom)));
                if any_bottom {
                    lattice.insert(result_id, LatticeValue::Bottom);
                    changed = true;
                    continue;
                }

                // If any operand is still Top, we can't fold yet.
                if operand_vals.iter().any(|v| v.is_none()) {
                    continue;
                }

                // All operands are Constant — try to evaluate.
                let folded = evaluate_op(op.opcode, &operand_vals)
                    .or_else(|| evaluate_builtin_call(op, &operand_vals))
                    .or_else(|| evaluate_method_call(op, &operand_vals));
                if let Some(result) = folded {
                    lattice.insert(result_id, LatticeValue::Constant(result));
                    changed = true;
                } else {
                    // Can't fold this opcode — mark Bottom.
                    lattice.insert(result_id, LatticeValue::Bottom);
                    changed = true;
                }
            }
        }
    }

    // Phase 3: Rewrite — replace constant-valued ops with ConstXxx ops.
    for &bid in &block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        for op in &mut block.ops {
            if op.results.is_empty() {
                continue;
            }
            let result_id = op.results[0];
            // Don't rewrite ops that are already constant constructors.
            match op.opcode {
                OpCode::ConstInt
                | OpCode::ConstFloat
                | OpCode::ConstStr
                | OpCode::ConstBool
                | OpCode::ConstNone => {
                    continue;
                }
                _ => {}
            }
            if let Some(LatticeValue::Constant(cv)) = lattice.get(&result_id) {
                match cv {
                    ConstVal::Int(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("value".into(), AttrValue::Int(*v));
                        op.opcode = OpCode::ConstInt;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::Float(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("f_value".into(), AttrValue::Float(*v));
                        op.opcode = OpCode::ConstFloat;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::Bool(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("value".into(), AttrValue::Bool(*v));
                        op.opcode = OpCode::ConstBool;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::Str(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("s_value".into(), AttrValue::Str(v.clone()));
                        op.opcode = OpCode::ConstStr;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::None => {
                        op.opcode = OpCode::ConstNone;
                        op.operands.clear();
                        op.attrs = AttrDict::new();
                        stats.values_changed += 1;
                    }
                    // Compound types (List, Dict, Range) stay in the lattice for
                    // downstream folding (e.g. len([1,2,3]) → 3) but cannot be
                    // rewritten to a single constant opcode since no ConstList/
                    // ConstDict/ConstRange opcodes exist in TIR.
                    ConstVal::List(_) | ConstVal::Dict(_) | ConstVal::Range { .. } => {}
                }
            }
        }
    }

    // Phase 4: Fold constant conditional branches to unconditional branches.
    // SAFETY: Never fold branches whose targets include a loop header —
    // the loop condition depends on runtime iteration state that SCCP's
    // forward-only lattice cannot model correctly.
    for &bid in &block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        let new_term = match &block.terminator {
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                // Skip if either branch target is a loop header — folding
                // these would eliminate loop bodies.
                let targets_loop = func
                    .loop_roles
                    .get(then_block)
                    .is_some_and(|r| *r == LoopRole::LoopHeader)
                    || func
                        .loop_roles
                        .get(else_block)
                        .is_some_and(|r| *r == LoopRole::LoopHeader);
                if targets_loop {
                    None
                } else {
                    match lattice.get(cond) {
                        Some(LatticeValue::Constant(ConstVal::Bool(true))) => {
                            Some(Terminator::Branch {
                                target: *then_block,
                                args: then_args.clone(),
                            })
                        }
                        Some(LatticeValue::Constant(ConstVal::Bool(false))) => {
                            Some(Terminator::Branch {
                                target: *else_block,
                                args: else_args.clone(),
                            })
                        }
                        // Python truthiness: nonzero int is truthy
                        Some(LatticeValue::Constant(ConstVal::Int(v))) => {
                            if *v != 0 {
                                Some(Terminator::Branch {
                                    target: *then_block,
                                    args: then_args.clone(),
                                })
                            } else {
                                Some(Terminator::Branch {
                                    target: *else_block,
                                    args: else_args.clone(),
                                })
                            }
                        }
                        Some(LatticeValue::Constant(ConstVal::None)) => Some(Terminator::Branch {
                            target: *else_block,
                            args: else_args.clone(),
                        }),
                        _ => None,
                    }
                } // close else { ... } for targets_loop guard
            }
            _ => None,
        };
        if let Some(term) = new_term {
            block.terminator = term;
            stats.ops_removed += 1; // count branch simplification
        }
    }

    // Phase 5: Eliminate blocks that became unreachable after branch folding.
    // When a CondBranch is folded to a Branch, one successor is no longer
    // reachable from the folded block. If that was the only path to the target,
    // the target and its transitive successors become dead. Leaving dead blocks
    // in the TIR is incorrect because their ops reference values whose
    // definitions may no longer dominate them (the dominance tree changed when
    // the CFG edge was removed). Removing dead blocks prevents downstream
    // verification from reporting false SSA dominance violations.
    if stats.ops_removed > 0 {
        let reachable = metadata_preserving_reachable_blocks(func);
        let dead_blocks: Vec<BlockId> = func
            .blocks
            .keys()
            .copied()
            .filter(|bid| !reachable.contains(bid))
            .collect();
        for bid in &dead_blocks {
            func.blocks.remove(bid);
            func.loop_roles.remove(bid);
            func.loop_pairs.remove(bid);
            func.loop_break_kinds.remove(bid);
            func.loop_cond_blocks.remove(bid);
            func.label_id_map.remove(&bid.0);
        }
        stats.ops_removed += dead_blocks.len();
    }

    stats
}

/// Compute len(range(start, stop, step)) using Python semantics.
fn range_len(start: i64, stop: i64, step: i64) -> i64 {
    if step > 0 {
        if start >= stop {
            0
        } else {
            (stop - start - 1) / step + 1
        }
    } else if step < 0 {
        if start <= stop {
            0
        } else {
            (start - stop - 1) / (-step) + 1
        }
    } else {
        0 // step == 0 is ValueError, but we guard against this at construction
    }
}

/// Try to evaluate a binary/unary op on constant operands.
fn evaluate_op(opcode: OpCode, operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    match opcode {
        // Binary arithmetic
        // Use checked arithmetic to avoid panic on overflow in debug / silent wrap in release.
        // On overflow, return None → value stays as Bottom (unfoldable), matching Python's BigInt.
        OpCode::Add => {
            // Try string concatenation first, then numeric addition.
            eval_str_concat(operands)
                .or_else(|| eval_list_concat(operands))
                .or_else(|| eval_binary(operands, |a, b| a.checked_add(b), |a, b| Some(a + b)))
        }
        OpCode::Sub => eval_binary(operands, |a, b| a.checked_sub(b), |a, b| Some(a - b)),
        OpCode::Mul => {
            // Try string/list repeat first, then numeric multiplication.
            eval_str_repeat(operands)
                .or_else(|| eval_list_repeat(operands))
                .or_else(|| eval_binary(operands, |a, b| a.checked_mul(b), |a, b| Some(a * b)))
        }
        OpCode::Div => eval_binary_div(operands),
        OpCode::FloorDiv => eval_binary_floordiv(operands),
        OpCode::Mod => eval_binary_mod(operands),
        OpCode::Pow => eval_binary_pow(operands),

        // Comparisons
        OpCode::Eq => eval_cmp(operands, |a, b| a == b, |a, b| a == b, |a, b| a == b),
        OpCode::Ne => eval_cmp(operands, |a, b| a != b, |a, b| a != b, |a, b| a != b),
        OpCode::Lt => eval_cmp(operands, |a, b| a < b, |a, b| a < b, |a, b| !a & b),
        OpCode::Le => eval_cmp(operands, |a, b| a <= b, |a, b| a <= b, |a, b| a <= b),
        OpCode::Gt => eval_cmp(operands, |a, b| a > b, |a, b| a > b, |a, b| a & !b),
        OpCode::Ge => eval_cmp(operands, |a, b| a >= b, |a, b| a >= b, |a, b| a >= b),

        // Unary
        OpCode::Neg => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => v.checked_neg().map(ConstVal::Int),
                ConstVal::Float(v) => Some(ConstVal::Float(-v)),
                _ => None,
            }
        }
        OpCode::Not => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Bool(v) => Some(ConstVal::Bool(!v)),
                _ => None,
            }
        }

        // Container construction with all-constant elements.
        OpCode::BuildList => eval_build_list(operands),
        OpCode::BuildDict => eval_build_dict(operands),
        OpCode::BuildTuple => eval_build_list(operands), // tuples fold to List for SCCP purposes

        _ => None,
    }
}

/// Fold string concatenation: "a" + "b" → "ab".
fn eval_str_concat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Str(x), ConstVal::Str(y)) => {
            let result = format!("{}{}", x, y);
            if result.len() <= MAX_COMPOUND_ELEMENTS {
                Some(ConstVal::Str(result))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Fold list concatenation: [1,2] + [3,4] → [1,2,3,4].
fn eval_list_concat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::List(x), ConstVal::List(y)) => {
            let total = x.len() + y.len();
            if total <= MAX_COMPOUND_ELEMENTS {
                let mut result = x.clone();
                result.extend(y.iter().cloned());
                Some(ConstVal::List(result))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Fold string repeat: "ab" * 3 → "ababab".
fn eval_str_repeat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Str(s), ConstVal::Int(n)) | (ConstVal::Int(n), ConstVal::Str(s)) => {
            if *n <= 0 {
                Some(ConstVal::Str(String::new()))
            } else {
                let count = *n as usize;
                let result_len = s.len().checked_mul(count)?;
                if result_len <= MAX_COMPOUND_ELEMENTS {
                    Some(ConstVal::Str(s.repeat(count)))
                } else {
                    None
                }
            }
        }
        _ => None,
    }
}

/// Fold list repeat: [1,2] * 3 → [1,2,1,2,1,2].
fn eval_list_repeat(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    let (list, n) = match (a, b) {
        (ConstVal::List(l), ConstVal::Int(n)) => (l, *n),
        (ConstVal::Int(n), ConstVal::List(l)) => (l, *n),
        _ => return None,
    };
    if n <= 0 {
        return Some(ConstVal::List(Vec::new()));
    }
    let count = n as usize;
    let total = list.len().checked_mul(count)?;
    if total > MAX_COMPOUND_ELEMENTS {
        return None;
    }
    let mut result = Vec::with_capacity(total);
    for _ in 0..count {
        result.extend(list.iter().cloned());
    }
    Some(ConstVal::List(result))
}

/// Fold BuildList with all-constant operands to ConstVal::List.
fn eval_build_list(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    if operands.len() > MAX_COMPOUND_ELEMENTS {
        return None;
    }
    let elements: Vec<ConstVal> = operands
        .iter()
        .map(|o| o.map(|v| (*v).clone()))
        .collect::<Option<Vec<_>>>()?;
    Some(ConstVal::List(elements))
}

/// Fold BuildDict with all-constant operands to ConstVal::Dict.
/// Dict operands are laid out as [k1, v1, k2, v2, ...].
fn eval_build_dict(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    if operands.len() % 2 != 0 {
        return None;
    }
    let n_entries = operands.len() / 2;
    if n_entries > MAX_COMPOUND_ELEMENTS {
        return None;
    }
    let mut entries = Vec::with_capacity(n_entries);
    for i in 0..n_entries {
        let k = operands[i * 2]?.clone();
        let v = operands[i * 2 + 1]?.clone();
        entries.push((k, v));
    }
    Some(ConstVal::Dict(entries))
}

/// Evaluate a binary arithmetic op on int or float operands.
/// Int operations use checked arithmetic — returns None on overflow
/// (matching Python's BigInt promotion behavior: we can't fold it, so leave it unfoldable).
fn eval_binary(
    operands: &[Option<&ConstVal>],
    int_op: impl Fn(i64, i64) -> Option<i64>,
    float_op: impl Fn(f64, f64) -> Option<f64>,
) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => int_op(*x, *y).map(ConstVal::Int),
        (ConstVal::Float(x), ConstVal::Float(y)) => float_op(*x, *y).map(ConstVal::Float),
        _ => None,
    }
}

fn eval_binary_div(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) if *y != 0 => {
            // Python `/` on ints returns float
            Some(ConstVal::Float(*x as f64 / *y as f64))
        }
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => Some(ConstVal::Float(*x / *y)),
        _ => None,
    }
}

fn eval_binary_floordiv(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) if *y != 0 => {
            // Python floor division: rounds towards negative infinity.
            // Rust's div_euclid rounds towards zero for negative divisors — WRONG.
            // Use explicit floor division: q = x/y, adjust if signs differ and not exact.
            let q = x / y;
            let r = x % y;
            let result = if r != 0 && ((*x ^ *y) < 0) { q - 1 } else { q };
            Some(ConstVal::Int(result))
        }
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => {
            Some(ConstVal::Float((*x / *y).floor()))
        }
        _ => None,
    }
}

fn eval_binary_mod(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) if *y != 0 => {
            // Python modulo: result has the sign of the divisor.
            // C/Rust rem_euclid always returns non-negative — WRONG for negative divisors.
            let r = *x % *y;
            let result = if r != 0 && ((r ^ *y) < 0) { r + *y } else { r };
            Some(ConstVal::Int(result))
        }
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => {
            // Python modulo semantics
            let r = *x % *y;
            let result = if r != 0.0 && r.signum() != y.signum() {
                r + *y
            } else {
                r
            };
            Some(ConstVal::Float(result))
        }
        _ => None,
    }
}

fn eval_binary_pow(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(base), ConstVal::Int(exp)) => {
            if *exp >= 0 && *exp <= 63 {
                // Safe small exponent — use checked pow to avoid overflow panic.
                base.checked_pow(*exp as u32).map(ConstVal::Int)
            } else {
                None
            }
        }
        (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.powf(*y))),
        _ => None,
    }
}

/// Evaluate a comparison op.
fn eval_cmp(
    operands: &[Option<&ConstVal>],
    int_cmp: impl Fn(i64, i64) -> bool,
    float_cmp: impl Fn(f64, f64) -> bool,
    bool_cmp: impl Fn(bool, bool) -> bool,
) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Bool(int_cmp(*x, *y))),
        (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Bool(float_cmp(*x, *y))),
        (ConstVal::Bool(x), ConstVal::Bool(y)) => Some(ConstVal::Bool(bool_cmp(*x, *y))),
        _ => None,
    }
}

/// Try to concrete-eval a `CallBuiltin` op when all operands are constant
/// and the callee is a known pure builtin.
fn evaluate_builtin_call(
    op: &crate::tir::ops::TirOp,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    if op.opcode != OpCode::CallBuiltin {
        return None;
    }
    let name = match op.attrs.get("name") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    let fx = effects::builtin_effects(name)?;
    if !fx.is_pure() {
        return None;
    }
    eval_concrete_builtin(name, operands)
}

/// Try to concrete-eval a `CallMethod` op when the receiver is a known
/// constant and the method is pure.
fn evaluate_method_call(
    op: &crate::tir::ops::TirOp,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    if op.opcode != OpCode::CallMethod {
        return None;
    }
    let method = match op.attrs.get("method") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    let receiver = operands.first().copied().flatten()?;
    let receiver_type = match receiver {
        ConstVal::Str(_) => "str",
        ConstVal::Int(_) => "int",
        ConstVal::Float(_) => "float",
        ConstVal::Bool(_) => "bool",
        ConstVal::List(_) => "list",
        ConstVal::Dict(_) => "dict",
        ConstVal::Range { .. } => return None,
        ConstVal::None => return None,
    };
    let fx = effects::method_effects(receiver_type, method)?;
    if !fx.is_pure() {
        return None;
    }
    eval_concrete_method(receiver_type, method, operands)
}

/// Concrete evaluation of known pure builtins.
fn eval_concrete_builtin(name: &str, operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    match name {
        "len" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Str(s) => Some(ConstVal::Int(s.len() as i64)),
                ConstVal::List(elems) => Some(ConstVal::Int(elems.len() as i64)),
                ConstVal::Dict(entries) => Some(ConstVal::Int(entries.len() as i64)),
                ConstVal::Range { start, stop, step } => {
                    // Python: len(range(start, stop, step))
                    let len = range_len(*start, *stop, *step);
                    Some(ConstVal::Int(len))
                }
                _ => None,
            }
        }
        "abs" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => v.checked_abs().map(ConstVal::Int),
                ConstVal::Float(v) => Some(ConstVal::Float(v.abs())),
                _ => None,
            }
        }
        "bool" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Bool(*v != 0)),
                ConstVal::Float(v) => Some(ConstVal::Bool(*v != 0.0)),
                ConstVal::Bool(v) => Some(ConstVal::Bool(*v)),
                ConstVal::Str(s) => Some(ConstVal::Bool(!s.is_empty())),
                ConstVal::None => Some(ConstVal::Bool(false)),
                ConstVal::List(elems) => Some(ConstVal::Bool(!elems.is_empty())),
                ConstVal::Dict(entries) => Some(ConstVal::Bool(!entries.is_empty())),
                ConstVal::Range { start, stop, step } => {
                    Some(ConstVal::Bool(range_len(*start, *stop, *step) > 0))
                }
            }
        }
        "int" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Int(*v)),
                ConstVal::Float(v) => Some(ConstVal::Int(*v as i64)),
                ConstVal::Bool(v) => Some(ConstVal::Int(if *v { 1 } else { 0 })),
                _ => None,
            }
        }
        "float" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Float(*v as f64)),
                ConstVal::Float(v) => Some(ConstVal::Float(*v)),
                ConstVal::Bool(v) => Some(ConstVal::Float(if *v { 1.0 } else { 0.0 })),
                _ => None,
            }
        }
        "str" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Str(v.to_string())),
                ConstVal::Float(v) => {
                    // Python: float → str preserves ".0" for whole numbers
                    let s = if v.fract() == 0.0 && v.is_finite() {
                        format!("{:.1}", v)
                    } else {
                        format!("{}", v)
                    };
                    Some(ConstVal::Str(s))
                }
                ConstVal::Bool(v) => {
                    Some(ConstVal::Str(if *v { "True" } else { "False" }.to_string()))
                }
                ConstVal::Str(s) => Some(ConstVal::Str(s.clone())),
                ConstVal::None => Some(ConstVal::Str("None".to_string())),
                _ => None, // compound types don't fold to str
            }
        }
        "repr" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => Some(ConstVal::Str(v.to_string())),
                ConstVal::Float(v) => {
                    let s = if v.fract() == 0.0 && v.is_finite() {
                        format!("{:.1}", v)
                    } else {
                        format!("{}", v)
                    };
                    Some(ConstVal::Str(s))
                }
                ConstVal::Bool(v) => {
                    Some(ConstVal::Str(if *v { "True" } else { "False" }.to_string()))
                }
                ConstVal::Str(s) => Some(ConstVal::Str(format!("'{}'", s))),
                ConstVal::None => Some(ConstVal::Str("None".to_string())),
                _ => None, // compound types don't fold to repr
            }
        }
        "chr" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                if *v >= 0 && *v <= 0x10FFFF {
                    char::from_u32(*v as u32).map(|c| ConstVal::Str(c.to_string()))
                } else {
                    None
                }
            } else {
                None
            }
        }
        "ord" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Str(s) = a {
                let mut chars = s.chars();
                let first = chars.next()?;
                if chars.next().is_none() {
                    Some(ConstVal::Int(first as i64))
                } else {
                    None
                }
            } else {
                None
            }
        }
        "hex" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0x{:x}", -v)
                } else {
                    format!("0x{:x}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        "oct" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0o{:o}", -v)
                } else {
                    format!("0o{:o}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        "bin" => {
            let a = operands.first().copied().flatten()?;
            if let ConstVal::Int(v) = a {
                let s = if *v < 0 {
                    format!("-0b{:b}", -v)
                } else {
                    format!("0b{:b}", v)
                };
                Some(ConstVal::Str(s))
            } else {
                None
            }
        }
        "range" => {
            // range(stop), range(start, stop), range(start, stop, step)
            match operands.len() {
                1 => {
                    let stop = match operands[0]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    Some(ConstVal::Range {
                        start: 0,
                        stop,
                        step: 1,
                    })
                }
                2 => {
                    let start = match operands[0]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    let stop = match operands[1]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    Some(ConstVal::Range {
                        start,
                        stop,
                        step: 1,
                    })
                }
                3 => {
                    let start = match operands[0]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    let stop = match operands[1]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    let step = match operands[2]? {
                        ConstVal::Int(v) => *v,
                        _ => return None,
                    };
                    if step == 0 {
                        return None; // ValueError in Python
                    }
                    Some(ConstVal::Range { start, stop, step })
                }
                _ => None,
            }
        }
        "sorted" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::List(elems) => {
                    // Only sort homogeneous int lists (Python raises TypeError
                    // on mixed types like int < str).
                    let mut ints = Vec::with_capacity(elems.len());
                    for e in elems {
                        match e {
                            ConstVal::Int(v) => ints.push(*v),
                            _ => return None,
                        }
                    }
                    ints.sort();
                    Some(ConstVal::List(ints.into_iter().map(ConstVal::Int).collect()))
                }
                _ => None,
            }
        }
        "sum" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::List(elems) => {
                    // sum([int, int, ...]) → int
                    let mut total: i64 = 0;
                    for elem in elems {
                        match elem {
                            ConstVal::Int(v) => {
                                total = total.checked_add(*v)?;
                            }
                            _ => return None,
                        }
                    }
                    Some(ConstVal::Int(total))
                }
                _ => None,
            }
        }
        "min" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Int(std::cmp::min(*x, *y))),
                (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.min(*y))),
                _ => None,
            }
        }
        "max" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Int(std::cmp::max(*x, *y))),
                (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.max(*y))),
                _ => None,
            }
        }
        "math.sqrt" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) if *v >= 0.0 => Some(ConstVal::Float(v.sqrt())),
                ConstVal::Int(v) if *v >= 0 => Some(ConstVal::Float((*v as f64).sqrt())),
                _ => None,
            }
        }
        "math.floor" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Int(v.floor() as i64)),
                ConstVal::Int(v) => Some(ConstVal::Int(*v)),
                _ => None,
            }
        }
        "math.ceil" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Int(v.ceil() as i64)),
                ConstVal::Int(v) => Some(ConstVal::Int(*v)),
                _ => None,
            }
        }
        "math.log" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) if *v > 0.0 => Some(ConstVal::Float(v.ln())),
                ConstVal::Int(v) if *v > 0 => Some(ConstVal::Float((*v as f64).ln())),
                _ => None,
            }
        }
        "math.exp" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Float(v.exp())),
                ConstVal::Int(v) => Some(ConstVal::Float((*v as f64).exp())),
                _ => None,
            }
        }
        "math.sin" | "math.cos" | "math.tan" | "math.asin" | "math.acos" | "math.atan" => {
            let a = operands.first().copied().flatten()?;
            let v = match a {
                ConstVal::Float(v) => *v,
                ConstVal::Int(v) => *v as f64,
                _ => return None,
            };
            let result = match name {
                "math.sin" => v.sin(),
                "math.cos" => v.cos(),
                "math.tan" => v.tan(),
                "math.asin" => v.asin(),
                "math.acos" => v.acos(),
                "math.atan" => v.atan(),
                _ => unreachable!(),
            };
            Some(ConstVal::Float(result))
        }
        "math.fabs" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Float(v.abs())),
                ConstVal::Int(v) => Some(ConstVal::Float((*v as f64).abs())),
                _ => None,
            }
        }
        "math.trunc" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Int(v.trunc() as i64)),
                ConstVal::Int(v) => Some(ConstVal::Int(*v)),
                _ => None,
            }
        }
        "math.isfinite" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Bool(v.is_finite())),
                ConstVal::Int(_) => Some(ConstVal::Bool(true)),
                _ => None,
            }
        }
        "math.isinf" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Bool(v.is_infinite())),
                ConstVal::Int(_) => Some(ConstVal::Bool(false)),
                _ => None,
            }
        }
        "math.isnan" => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Float(v) => Some(ConstVal::Bool(v.is_nan())),
                ConstVal::Int(_) => Some(ConstVal::Bool(false)),
                _ => None,
            }
        }
        "math.copysign" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.copysign(*y))),
                _ => None,
            }
        }
        "math.pow" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.powf(*y))),
                _ => None,
            }
        }
        "math.atan2" | "math.hypot" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            match (a, b) {
                (ConstVal::Float(x), ConstVal::Float(y)) => {
                    let result = if name == "math.atan2" {
                        x.atan2(*y)
                    } else {
                        x.hypot(*y)
                    };
                    Some(ConstVal::Float(result))
                }
                _ => None,
            }
        }
        "math.gcd" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            if let (ConstVal::Int(x), ConstVal::Int(y)) = (a, b) {
                fn gcd(mut a: i64, mut b: i64) -> i64 {
                    a = a.abs();
                    b = b.abs();
                    while b != 0 {
                        let t = b;
                        b = a % b;
                        a = t;
                    }
                    a
                }
                Some(ConstVal::Int(gcd(*x, *y)))
            } else {
                None
            }
        }
        "math.lcm" => {
            if operands.len() < 2 {
                return None;
            }
            let a = operands[0]?;
            let b = operands[1]?;
            if let (ConstVal::Int(x), ConstVal::Int(y)) = (a, b) {
                if *x == 0 || *y == 0 {
                    Some(ConstVal::Int(0))
                } else {
                    fn gcd(mut a: i64, mut b: i64) -> i64 {
                        a = a.abs();
                        b = b.abs();
                        while b != 0 {
                            let t = b;
                            b = a % b;
                            a = t;
                        }
                        a
                    }
                    let g = gcd(*x, *y);
                    x.checked_div(g)
                        .and_then(|q| q.checked_mul(*y))
                        .map(|v| ConstVal::Int(v.abs()))
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Concrete evaluation of known pure methods on constant receivers.
fn eval_concrete_method(
    receiver_type: &str,
    method: &str,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    let receiver = operands.first().copied().flatten()?;
    match receiver_type {
        "str" => {
            let s = if let ConstVal::Str(s) = receiver {
                s
            } else {
                return None;
            };
            match method {
                "upper" => Some(ConstVal::Str(s.to_uppercase())),
                "lower" => Some(ConstVal::Str(s.to_lowercase())),
                "strip" => Some(ConstVal::Str(s.trim().to_string())),
                "lstrip" => Some(ConstVal::Str(s.trim_start().to_string())),
                "rstrip" => Some(ConstVal::Str(s.trim_end().to_string())),
                "title" => {
                    let mut result = String::with_capacity(s.len());
                    let mut prev_is_boundary = true;
                    for c in s.chars() {
                        if prev_is_boundary && c.is_alphabetic() {
                            for uc in c.to_uppercase() {
                                result.push(uc);
                            }
                            prev_is_boundary = false;
                        } else if !c.is_alphabetic() {
                            result.push(c);
                            prev_is_boundary = true;
                        } else {
                            for lc in c.to_lowercase() {
                                result.push(lc);
                            }
                            prev_is_boundary = false;
                        }
                    }
                    Some(ConstVal::Str(result))
                }
                "capitalize" => {
                    let mut chars = s.chars();
                    let result = match chars.next() {
                        Some(c) => {
                            let upper: String = c.to_uppercase().collect();
                            let lower: String = chars.flat_map(|c| c.to_lowercase()).collect();
                            format!("{}{}", upper, lower)
                        }
                        None => String::new(),
                    };
                    Some(ConstVal::Str(result))
                }
                "swapcase" => {
                    let result: String = s
                        .chars()
                        .flat_map(|c| {
                            if c.is_uppercase() {
                                c.to_lowercase().collect::<Vec<_>>()
                            } else if c.is_lowercase() {
                                c.to_uppercase().collect::<Vec<_>>()
                            } else {
                                vec![c]
                            }
                        })
                        .collect();
                    Some(ConstVal::Str(result))
                }
                "isalpha" => Some(ConstVal::Bool(
                    !s.is_empty() && s.chars().all(|c| c.is_alphabetic()),
                )),
                "isdigit" => Some(ConstVal::Bool(
                    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()),
                )),
                "isalnum" => Some(ConstVal::Bool(
                    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric()),
                )),
                "isspace" => Some(ConstVal::Bool(
                    !s.is_empty() && s.chars().all(|c| c.is_whitespace()),
                )),
                "isupper" => Some(ConstVal::Bool(
                    s.chars().any(|c| c.is_uppercase()) && !s.chars().any(|c| c.is_lowercase()),
                )),
                "islower" => Some(ConstVal::Bool(
                    s.chars().any(|c| c.is_lowercase()) && !s.chars().any(|c| c.is_uppercase()),
                )),
                "startswith" => {
                    let prefix = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(p) = prefix {
                        Some(ConstVal::Bool(s.starts_with(p.as_str())))
                    } else {
                        None
                    }
                }
                "endswith" => {
                    let suffix = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(p) = suffix {
                        Some(ConstVal::Bool(s.ends_with(p.as_str())))
                    } else {
                        None
                    }
                }
                "find" => {
                    let needle = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(n) = needle {
                        let idx = s.find(n.as_str()).map(|i| i as i64).unwrap_or(-1);
                        Some(ConstVal::Int(idx))
                    } else {
                        None
                    }
                }
                "rfind" => {
                    let needle = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(n) = needle {
                        let idx = s.rfind(n.as_str()).map(|i| i as i64).unwrap_or(-1);
                        Some(ConstVal::Int(idx))
                    } else {
                        None
                    }
                }
                "count" => {
                    let needle = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(n) = needle {
                        if n.is_empty() {
                            // Python: "abc".count("") == 4 (len + 1)
                            Some(ConstVal::Int(s.len() as i64 + 1))
                        } else {
                            Some(ConstVal::Int(s.matches(n.as_str()).count() as i64))
                        }
                    } else {
                        None
                    }
                }
                "replace" => {
                    if operands.len() < 3 {
                        return None;
                    }
                    let old = operands[1]?;
                    let new = operands[2]?;
                    if let (ConstVal::Str(o), ConstVal::Str(n)) = (old, new) {
                        Some(ConstVal::Str(s.replace(o.as_str(), n.as_str())))
                    } else {
                        None
                    }
                }
                "removeprefix" => {
                    let prefix = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(p) = prefix {
                        let result = s.strip_prefix(p.as_str()).unwrap_or(s);
                        Some(ConstVal::Str(result.to_string()))
                    } else {
                        None
                    }
                }
                "removesuffix" => {
                    let suffix = operands.get(1).copied().flatten()?;
                    if let ConstVal::Str(p) = suffix {
                        let result = s.strip_suffix(p.as_str()).unwrap_or(s);
                        Some(ConstVal::Str(result.to_string()))
                    } else {
                        None
                    }
                }
                "zfill" => {
                    let width = operands.get(1).copied().flatten()?;
                    if let ConstVal::Int(w) = width {
                        let w = *w as usize;
                        if s.len() >= w {
                            Some(ConstVal::Str(s.clone()))
                        } else {
                            let (prefix, body) = if s.starts_with('-') || s.starts_with('+') {
                                (&s[..1], &s[1..])
                            } else {
                                ("", s.as_str())
                            };
                            let fill = w - s.len();
                            Some(ConstVal::Str(format!(
                                "{}{}{}",
                                prefix,
                                "0".repeat(fill),
                                body
                            )))
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        "int" => {
            let v = if let ConstVal::Int(v) = receiver {
                *v
            } else {
                return None;
            };
            match method {
                "bit_length" => {
                    if v == 0 {
                        Some(ConstVal::Int(0))
                    } else {
                        Some(ConstVal::Int(64 - v.abs().leading_zeros() as i64))
                    }
                }
                "bit_count" => Some(ConstVal::Int(v.unsigned_abs().count_ones() as i64)),
                _ => None,
            }
        }
        "float" => {
            let v = if let ConstVal::Float(v) = receiver {
                *v
            } else {
                return None;
            };
            match method {
                "is_integer" => Some(ConstVal::Bool(v.fract() == 0.0 && v.is_finite())),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::TirBlock;
    use crate::tir::ops::{Dialect, TirOp};
    use crate::tir::types::TirType;

    /// Helper: create a function with a single block, apply SCCP, return the block's ops.
    fn run_sccp_on_ops(ops: Vec<TirOp>, next_value: u32) -> (Vec<TirOp>, Terminator) {
        let mut func = TirFunction::new("test".into(), vec![], TirType::None);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = ops;
            entry.terminator = Terminator::Return { values: vec![] };
        }
        func.next_value = next_value;
        run(&mut func);
        let entry = &func.blocks[&func.entry_block];
        (entry.ops.clone(), entry.terminator.clone())
    }

    fn make_const_int(result: u32, value: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_const_float(result: u32, value: f64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("f_value".into(), AttrValue::Float(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstFloat,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_const_bool(result: u32, value: bool) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Bool(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_binop(opcode: OpCode, result: u32, lhs: u32, rhs: u32) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(lhs), ValueId(rhs)],
            results: vec![ValueId(result)],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_check_exception(target_label: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(target_label));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        }
    }

    #[test]
    fn fold_int_addition() {
        // 1 + 2 => 3
        let ops = vec![
            make_const_int(0, 1),
            make_const_int(1, 2),
            make_binop(OpCode::Add, 2, 0, 1),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        // The Add op should be rewritten to ConstInt(3).
        assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(3)));
    }

    #[test]
    fn fold_comparison_gt() {
        // 5 > 3 => true
        let ops = vec![
            make_const_int(0, 5),
            make_const_int(1, 3),
            make_binop(OpCode::Gt, 2, 0, 1),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
        assert_eq!(
            result_ops[2].attrs.get("value"),
            Some(&AttrValue::Bool(true))
        );
    }

    #[test]
    fn fold_constant_cond_branch_true() {
        // if true: goto bb1, else: goto bb2 => Branch to bb1
        let mut func = TirFunction::new("test".into(), vec![], TirType::None);
        let then_id = func.fresh_block();
        let else_id = func.fresh_block();

        let const_true = make_const_bool(0, true);
        func.next_value = 1;

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_true);
            entry.terminator = Terminator::CondBranch {
                cond: ValueId(0),
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            };
        }

        // Add stub blocks so iteration doesn't miss them.
        func.blocks.insert(
            then_id,
            TirBlock {
                id: then_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            else_id,
            TirBlock {
                id: else_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func);
        let entry = &func.blocks[&func.entry_block];
        match &entry.terminator {
            Terminator::Branch { target, .. } => {
                assert_eq!(*target, then_id);
            }
            other => panic!("expected Branch, got {:?}", other),
        }
        assert!(stats.ops_removed > 0);
    }

    #[test]
    fn branch_fold_keeps_check_exception_handler_block_reachable() {
        let mut func = TirFunction::new("test".into(), vec![], TirType::None);
        func.has_exception_handling = true;
        let active_id = func.fresh_block();
        let dead_id = func.fresh_block();
        let exit_id = func.fresh_block();
        let handler_id = func.fresh_block();
        func.label_id_map.insert(handler_id.0, 100);

        let const_true = make_const_bool(0, true);
        func.next_value = 1;

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_true);
            entry.terminator = Terminator::CondBranch {
                cond: ValueId(0),
                then_block: active_id,
                then_args: vec![],
                else_block: dead_id,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            active_id,
            TirBlock {
                id: active_id,
                args: vec![],
                ops: vec![make_check_exception(100)],
                terminator: Terminator::Branch {
                    target: exit_id,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            dead_id,
            TirBlock {
                id: dead_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler_id,
            TirBlock {
                id: handler_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func);

        assert!(stats.ops_removed > 0);
        assert!(
            !func.blocks.contains_key(&dead_id),
            "constant branch fold should still remove the truly dead normal successor"
        );
        assert!(
            func.blocks.contains_key(&handler_id),
            "check_exception handler blocks must remain reachable after SCCP branch folding"
        );
        assert_eq!(func.label_id_map.get(&handler_id.0), Some(&100));
    }

    #[test]
    fn no_fold_parameter_plus_const() {
        // x + 1 where x is a function parameter => no folding
        let mut func = TirFunction::new("test".into(), vec![TirType::I64], TirType::I64);
        // param is ValueId(0)
        let const_one = make_const_int(1, 1);
        let add = make_binop(OpCode::Add, 2, 0, 1);
        func.next_value = 3;
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_one);
            entry.ops.push(add);
            entry.terminator = Terminator::Return {
                values: vec![ValueId(2)],
            };
        }

        let stats = run(&mut func);
        let entry = &func.blocks[&func.entry_block];
        // The Add should remain an Add (not folded).
        assert_eq!(entry.ops[1].opcode, OpCode::Add);
        assert_eq!(stats.values_changed, 0);
    }

    #[test]
    fn fold_float_multiplication() {
        // 1.0 * 2.0 => 2.0
        let ops = vec![
            make_const_float(0, 1.0),
            make_const_float(1, 2.0),
            make_binop(OpCode::Mul, 2, 0, 1),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstFloat);
        assert_eq!(
            result_ops[2].attrs.get("f_value"),
            Some(&AttrValue::Float(2.0))
        );
    }

    // --- Concrete eval tests for effects-driven constant folding ---

    fn make_const_str(result: u32, value: &str) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str(value.into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_call_builtin(result: u32, name: &str, args: Vec<u32>) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str(name.into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands: args.into_iter().map(ValueId).collect(),
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_call_method(result: u32, method: &str, args: Vec<u32>) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("method".into(), AttrValue::Str(method.into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallMethod,
            operands: args.into_iter().map(ValueId).collect(),
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    #[test]
    fn fold_len_of_constant_string() {
        // len("hello") => 5
        let ops = vec![
            make_const_str(0, "hello"),
            make_call_builtin(1, "len", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[1].attrs.get("value"), Some(&AttrValue::Int(5)));
    }

    #[test]
    fn fold_abs_of_negative_int() {
        // abs(-42) => 42
        let ops = vec![make_const_int(0, -42), make_call_builtin(1, "abs", vec![0])];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[1].attrs.get("value"), Some(&AttrValue::Int(42)));
    }

    #[test]
    fn fold_math_sqrt_constant() {
        // math.sqrt(4.0) => 2.0
        let ops = vec![
            make_const_float(0, 4.0),
            make_call_builtin(1, "math.sqrt", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstFloat);
        assert_eq!(
            result_ops[1].attrs.get("f_value"),
            Some(&AttrValue::Float(2.0))
        );
    }

    #[test]
    fn fold_math_floor_constant() {
        // math.floor(3.7) => 3
        let ops = vec![
            make_const_float(0, 3.7),
            make_call_builtin(1, "math.floor", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[1].attrs.get("value"), Some(&AttrValue::Int(3)));
    }

    #[test]
    fn fold_str_upper_method() {
        // "hello".upper() => "HELLO"
        let ops = vec![
            make_const_str(0, "hello"),
            make_call_method(1, "upper", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[1].attrs.get("s_value"),
            Some(&AttrValue::Str("HELLO".into()))
        );
    }

    #[test]
    fn fold_str_lower_method() {
        // "WORLD".lower() => "world"
        let ops = vec![
            make_const_str(0, "WORLD"),
            make_call_method(1, "lower", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[1].attrs.get("s_value"),
            Some(&AttrValue::Str("world".into()))
        );
    }

    #[test]
    fn fold_str_strip_method() {
        // "  hi  ".strip() => "hi"
        let ops = vec![
            make_const_str(0, "  hi  "),
            make_call_method(1, "strip", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[1].attrs.get("s_value"),
            Some(&AttrValue::Str("hi".into()))
        );
    }

    #[test]
    fn fold_str_startswith_method() {
        // "hello".startswith("hel") => True
        let ops = vec![
            make_const_str(0, "hello"),
            make_const_str(1, "hel"),
            make_call_method(2, "startswith", vec![0, 1]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
        assert_eq!(
            result_ops[2].attrs.get("value"),
            Some(&AttrValue::Bool(true))
        );
    }

    #[test]
    fn fold_min_of_two_ints() {
        // min(5, 3) => 3
        let ops = vec![
            make_const_int(0, 5),
            make_const_int(1, 3),
            make_call_builtin(2, "min", vec![0, 1]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(3)));
    }

    #[test]
    fn fold_chr_ord_roundtrip() {
        // chr(65) => "A"
        let ops = vec![make_const_int(0, 65), make_call_builtin(1, "chr", vec![0])];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[1].attrs.get("s_value"),
            Some(&AttrValue::Str("A".into()))
        );
    }

    #[test]
    fn fold_hex_of_int() {
        // hex(255) => "0xff"
        let ops = vec![make_const_int(0, 255), make_call_builtin(1, "hex", vec![0])];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[1].attrs.get("s_value"),
            Some(&AttrValue::Str("0xff".into()))
        );
    }

    #[test]
    fn no_fold_print_builtin() {
        // print("hello") should NOT be folded (I/O side effect)
        let ops = vec![
            make_const_str(0, "hello"),
            make_call_builtin(1, "print", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::CallBuiltin);
    }

    #[test]
    fn fold_str_replace_method() {
        // "hello world".replace("world", "rust") => "hello rust"
        let ops = vec![
            make_const_str(0, "hello world"),
            make_const_str(1, "world"),
            make_const_str(2, "rust"),
            make_call_method(3, "replace", vec![0, 1, 2]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 4);
        assert_eq!(result_ops[3].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[3].attrs.get("s_value"),
            Some(&AttrValue::Str("hello rust".into()))
        );
    }

    #[test]
    fn fold_int_bit_length_method() {
        // (255).bit_length() => 8
        let ops = vec![
            make_const_int(0, 255),
            make_call_method(1, "bit_length", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[1].attrs.get("value"), Some(&AttrValue::Int(8)));
    }

    #[test]
    fn fold_bool_builtin() {
        // bool(0) => False
        let ops = vec![make_const_int(0, 0), make_call_builtin(1, "bool", vec![0])];
        let (result_ops, _) = run_sccp_on_ops(ops, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstBool);
        assert_eq!(
            result_ops[1].attrs.get("value"),
            Some(&AttrValue::Bool(false))
        );
    }

    #[test]
    fn fold_math_gcd() {
        // math.gcd(12, 8) => 4
        let ops = vec![
            make_const_int(0, 12),
            make_const_int(1, 8),
            make_call_builtin(2, "math.gcd", vec![0, 1]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(4)));
    }

    // --- Compound constant folding tests ---

    fn make_build_list(result: u32, elements: Vec<u32>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BuildList,
            operands: elements.into_iter().map(ValueId).collect(),
            results: vec![ValueId(result)],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_build_dict(result: u32, kv_pairs: Vec<u32>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BuildDict,
            operands: kv_pairs.into_iter().map(ValueId).collect(),
            results: vec![ValueId(result)],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    #[test]
    fn fold_len_of_constant_list() {
        // len([1, 2, 3]) => 3
        let ops = vec![
            make_const_int(0, 1),
            make_const_int(1, 2),
            make_const_int(2, 3),
            make_build_list(3, vec![0, 1, 2]),
            make_call_builtin(4, "len", vec![3]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 5);
        assert_eq!(result_ops[4].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[4].attrs.get("value"), Some(&AttrValue::Int(3)));
    }

    #[test]
    fn fold_len_of_constant_dict() {
        // len({"a": 1, "b": 2}) => 2
        let ops = vec![
            make_const_str(0, "a"),
            make_const_int(1, 1),
            make_const_str(2, "b"),
            make_const_int(3, 2),
            make_build_dict(4, vec![0, 1, 2, 3]),
            make_call_builtin(5, "len", vec![4]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 6);
        assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(2)));
    }

    #[test]
    fn fold_len_of_range() {
        // len(range(10)) => 10
        let ops = vec![
            make_const_int(0, 10),
            make_call_builtin(1, "range", vec![0]),
            make_call_builtin(2, "len", vec![1]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(10)));
    }

    #[test]
    fn fold_len_of_range_with_start_stop() {
        // len(range(3, 10)) => 7
        let ops = vec![
            make_const_int(0, 3),
            make_const_int(1, 10),
            make_call_builtin(2, "range", vec![0, 1]),
            make_call_builtin(3, "len", vec![2]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 4);
        assert_eq!(result_ops[3].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[3].attrs.get("value"), Some(&AttrValue::Int(7)));
    }

    #[test]
    fn fold_len_of_range_with_step() {
        // len(range(0, 10, 3)) => 4  (0, 3, 6, 9)
        let ops = vec![
            make_const_int(0, 0),
            make_const_int(1, 10),
            make_const_int(2, 3),
            make_call_builtin(3, "range", vec![0, 1, 2]),
            make_call_builtin(4, "len", vec![3]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 5);
        assert_eq!(result_ops[4].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[4].attrs.get("value"), Some(&AttrValue::Int(4)));
    }

    #[test]
    fn fold_len_of_empty_range() {
        // len(range(10, 0)) => 0
        let ops = vec![
            make_const_int(0, 10),
            make_const_int(1, 0),
            make_call_builtin(2, "range", vec![0, 1]),
            make_call_builtin(3, "len", vec![2]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 4);
        assert_eq!(result_ops[3].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[3].attrs.get("value"), Some(&AttrValue::Int(0)));
    }

    #[test]
    fn fold_len_of_negative_step_range() {
        // len(range(10, 0, -2)) => 5  (10, 8, 6, 4, 2)
        let ops = vec![
            make_const_int(0, 10),
            make_const_int(1, 0),
            make_const_int(2, -2),
            make_call_builtin(3, "range", vec![0, 1, 2]),
            make_call_builtin(4, "len", vec![3]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 5);
        assert_eq!(result_ops[4].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[4].attrs.get("value"), Some(&AttrValue::Int(5)));
    }

    #[test]
    fn fold_string_concatenation() {
        // "hello" + " " + "world" => "hello world"
        let ops = vec![
            make_const_str(0, "hello"),
            make_const_str(1, " "),
            make_binop(OpCode::Add, 2, 0, 1),
            make_const_str(3, "world"),
            make_binop(OpCode::Add, 4, 2, 3),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 5);
        // The intermediate "hello " should fold, then "hello " + "world" => "hello world"
        assert_eq!(result_ops[2].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[2].attrs.get("s_value"),
            Some(&AttrValue::Str("hello ".into()))
        );
        assert_eq!(result_ops[4].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[4].attrs.get("s_value"),
            Some(&AttrValue::Str("hello world".into()))
        );
    }

    #[test]
    fn fold_string_repeat() {
        // "ab" * 3 => "ababab"
        let ops = vec![
            make_const_str(0, "ab"),
            make_const_int(1, 3),
            make_binop(OpCode::Mul, 2, 0, 1),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[2].attrs.get("s_value"),
            Some(&AttrValue::Str("ababab".into()))
        );
    }

    #[test]
    fn fold_string_repeat_zero() {
        // "abc" * 0 => ""
        let ops = vec![
            make_const_str(0, "abc"),
            make_const_int(1, 0),
            make_binop(OpCode::Mul, 2, 0, 1),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstStr);
        assert_eq!(
            result_ops[2].attrs.get("s_value"),
            Some(&AttrValue::Str("".into()))
        );
    }

    #[test]
    fn fold_bool_of_constant_list() {
        // bool([]) => False, bool([1]) => True
        let ops_empty = vec![
            make_build_list(0, vec![]),
            make_call_builtin(1, "bool", vec![0]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops_empty, 2);
        assert_eq!(result_ops[1].opcode, OpCode::ConstBool);
        assert_eq!(
            result_ops[1].attrs.get("value"),
            Some(&AttrValue::Bool(false))
        );

        let ops_nonempty = vec![
            make_const_int(0, 42),
            make_build_list(1, vec![0]),
            make_call_builtin(2, "bool", vec![1]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops_nonempty, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
        assert_eq!(
            result_ops[2].attrs.get("value"),
            Some(&AttrValue::Bool(true))
        );
    }

    #[test]
    fn fold_sum_of_constant_list() {
        // sum([1, 2, 3, 4]) => 10
        let ops = vec![
            make_const_int(0, 1),
            make_const_int(1, 2),
            make_const_int(2, 3),
            make_const_int(3, 4),
            make_build_list(4, vec![0, 1, 2, 3]),
            make_call_builtin(5, "sum", vec![4]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 6);
        assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(10)));
    }

    #[test]
    fn fold_sorted_of_constant_list() {
        // sorted([3, 1, 2]) => [1, 2, 3]
        // len(sorted([3, 1, 2])) => 3
        let ops = vec![
            make_const_int(0, 3),
            make_const_int(1, 1),
            make_const_int(2, 2),
            make_build_list(3, vec![0, 1, 2]),
            make_call_builtin(4, "sorted", vec![3]),
            make_call_builtin(5, "len", vec![4]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 6);
        // sorted result stays as BuildList (no ConstList opcode), but len propagates
        assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(3)));
    }

    #[test]
    fn fold_list_concat() {
        // len([1, 2] + [3, 4]) => 4
        let ops = vec![
            make_const_int(0, 1),
            make_const_int(1, 2),
            make_build_list(2, vec![0, 1]),
            make_const_int(3, 3),
            make_const_int(4, 4),
            make_build_list(5, vec![3, 4]),
            make_binop(OpCode::Add, 6, 2, 5),
            make_call_builtin(7, "len", vec![6]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 8);
        assert_eq!(result_ops[7].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[7].attrs.get("value"), Some(&AttrValue::Int(4)));
    }

    #[test]
    fn fold_list_repeat() {
        // len([1, 2] * 3) => 6
        let ops = vec![
            make_const_int(0, 1),
            make_const_int(1, 2),
            make_build_list(2, vec![0, 1]),
            make_const_int(3, 3),
            make_binop(OpCode::Mul, 4, 2, 3),
            make_call_builtin(5, "len", vec![4]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 6);
        assert_eq!(result_ops[5].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[5].attrs.get("value"), Some(&AttrValue::Int(6)));
    }

    #[test]
    fn fold_bool_of_range() {
        // bool(range(0)) => False
        let ops = vec![
            make_const_int(0, 0),
            make_call_builtin(1, "range", vec![0]),
            make_call_builtin(2, "bool", vec![1]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
        assert_eq!(
            result_ops[2].attrs.get("value"),
            Some(&AttrValue::Bool(false))
        );

        // bool(range(5)) => True
        let ops = vec![
            make_const_int(0, 5),
            make_call_builtin(1, "range", vec![0]),
            make_call_builtin(2, "bool", vec![1]),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
        assert_eq!(
            result_ops[2].attrs.get("value"),
            Some(&AttrValue::Bool(true))
        );
    }

    #[test]
    fn no_fold_oversized_list() {
        // Building a list with > MAX_COMPOUND_ELEMENTS should not fold.
        // We test with 1001 elements (above the cap).
        let mut ops = Vec::new();
        for i in 0..1001u32 {
            ops.push(make_const_int(i, i as i64));
        }
        let elem_ids: Vec<u32> = (0..1001).collect();
        ops.push(make_build_list(1001, elem_ids));
        ops.push(make_call_builtin(1002, "len", vec![1001]));
        let (result_ops, _) = run_sccp_on_ops(ops, 1003);
        // The BuildList should NOT fold (too large), so len() can't fold either.
        let len_op = &result_ops[1002];
        assert_eq!(len_op.opcode, OpCode::CallBuiltin);
    }

    #[test]
    fn range_len_helper_correctness() {
        // Verify range_len matches Python semantics for edge cases
        assert_eq!(range_len(0, 10, 1), 10);
        assert_eq!(range_len(0, 10, 2), 5);
        assert_eq!(range_len(0, 10, 3), 4);
        assert_eq!(range_len(0, 0, 1), 0);
        assert_eq!(range_len(5, 5, 1), 0);
        assert_eq!(range_len(10, 0, -1), 10);
        assert_eq!(range_len(10, 0, -2), 5);
        assert_eq!(range_len(10, 0, -3), 4);
        assert_eq!(range_len(0, -10, -1), 10);
        assert_eq!(range_len(0, 10, -1), 0); // empty (step goes wrong way)
        assert_eq!(range_len(10, 0, 1), 0); // empty (step goes wrong way)
        assert_eq!(range_len(0, 1, 1), 1);
        assert_eq!(range_len(-5, 5, 1), 10);
    }
}
