//! Bounds Check Elimination (BCE) pass for TIR.
//!
//! Annotates `Index` operations that are provably bounds-check-safe by adding
//! a `"bce_safe"` attribute (set to `AttrValue::Bool(true)`).  Downstream
//! codegen can test for this attribute and skip the runtime bounds check.
//!
//! ## Phases
//!
//! ### Phase 1 — constant-index BCE
//!
//! An `Index` op is marked safe when **all** of the following hold:
//!   1. The index operand was produced by a `ConstInt` operation.
//!   2. The constant value is **non-negative** (i.e. `value >= 0`).
//!   3. The container operand has a known length **greater than** the constant.
//!
//! Negative constant indices still require a runtime wraparound (Python
//! semantics `lst[-1]`), so they are intentionally left unmarked.
//!
//! ### Phase 2 — inductive range analysis (IRCE)
//!
//! For `for i in range(N)` loops, the induction variable `i` is provably in
//! `[0, N)`.  When `a[i]` appears inside such a loop and the container `a` has
//! a known length `>= N`, the bounds check is eliminated.
//!
//! The pass detects range loops by tracing:
//!   1. Loop header (block with `LoopRole::LoopHeader`) containing
//!      `IterNextUnboxed` or `ForIter`.
//!   2. The iterator operand traces back to a `GetIter` whose source is a
//!      `CallBuiltin` with `name = "range"`.
//!   3. The first argument to that `CallBuiltin` is the upper bound `N`.
//!
//! ### Phase 3 — while-loop guard analysis
//!
//! For `while i <= n` loops that index into a container of length `n+1`,
//! the loop guard directly proves `i < len(container)`.  The pass detects:
//!   1. Loop header with `CondBranch` whose condition is `Le(i, n)` or `Lt(i, n)`.
//!   2. The TRUE successor (loop body) is bounded by `i <= n` (Le) or `i < n` (Lt).
//!   3. Container length from `Mul(BuildList(elem), count)` = `count`.
//!   4. When `len(container) = Add(n, 1)` and guard gives `i <= n`, the proof
//!      completes: `i <= n < n+1 = len(container)`.
//!
//! Container length tracking:
//!   - `BuildList` with `k` operands has length `k`.
//!   - `Mul(BuildList_1_elem, count)` has length `count` (list repeat).
//!   - `CallBuiltin("range", N)` followed by `Call("list", ...)` has length `N`.
//!   - More patterns can be added as needed.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

use super::PassStats;

// ---------------------------------------------------------------------------
// Range fact: a value is provably in [0, upper_bound).
// ---------------------------------------------------------------------------

/// A proven range constraint on a value: `0 <= value < upper_bound`.
#[derive(Debug, Clone)]
struct RangeFact {
    /// The upper bound value.  Either a constant (stored in `const_upper`) or
    /// a non-constant `ValueId` that represents the dynamic bound.
    upper_bound: ValueId,
}

/// Known container length — either a compile-time constant or a dynamic
/// `ValueId` that equals the length.
#[derive(Debug, Clone)]
enum KnownLength {
    Constant(i64),
    SameAs(ValueId),
}

/// A proven upper-bound fact from a while-loop guard.
///
/// When the loop header's CondBranch condition is `Le(var, bound)`, within
/// the TRUE successor (loop body), `var <= bound` holds.  For `Lt(var, bound)`,
/// `var < bound` holds.
#[derive(Debug, Clone)]
struct GuardFact {
    /// The variable that is bounded (the index).
    var: ValueId,
    /// The bound value from the comparison.
    bound: ValueId,
    /// True if the comparison is `Le` (var <= bound), false if `Lt` (var < bound).
    is_le: bool,
}

/// Decomposition of an `Add(a, const)` value.
#[derive(Debug, Clone)]
struct AddConst {
    base: ValueId,
    offset: i64,
}

// ---------------------------------------------------------------------------
// Pass implementation
// ---------------------------------------------------------------------------

/// Bounds Check Elimination pass.
///
/// Phase 1: marks `Index` ops with constant non-negative in-range indices.
/// Phase 2: marks `Index` ops inside `for i in range(N)` loops when the
///          container length is provably `>= N`.
/// Phase 3: marks `Index`/`StoreIndex` ops inside `while i <= n` loops when
///          the container was created via `[elem] * (n+1)` (list repeat).
///
/// Returns [`PassStats`] describing how many ops were annotated
/// (`values_changed`).
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "bce",
        ..Default::default()
    };

    // Collect block ids first to avoid a long borrow on `func.blocks`.
    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();

    // -----------------------------------------------------------------------
    // Analysis Phase 1: constant integer values
    // -----------------------------------------------------------------------
    let mut const_int_value: HashMap<ValueId, i64> = HashMap::new();

    for bid in &block_ids {
        if let Some(block) = func.blocks.get(bid) {
            for op in &block.ops {
                if op.opcode == OpCode::ConstInt
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value")
                {
                    for &result in &op.results {
                        const_int_value.insert(result, *v);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Analysis Phase 2: trace value definitions for range/length analysis
    // -----------------------------------------------------------------------

    // Map: ValueId → the ValueId that a GetIter was applied to.
    let mut get_iter_source: HashMap<ValueId, ValueId> = HashMap::new();
    // Map: ValueId → (callee_name, first_arg) for CallBuiltin ops.
    let mut call_builtin_info: HashMap<ValueId, (String, Option<ValueId>)> = HashMap::new();
    // Map: ValueId → known container length.
    let mut container_length: HashMap<ValueId, KnownLength> = HashMap::new();
    // Map: induction ValueId → RangeFact (value in [0, upper_bound)).
    let mut range_facts: HashMap<ValueId, RangeFact> = HashMap::new();
    // Map: ValueId → AddConst decomposition (result = base + offset).
    let mut add_const_decomp: HashMap<ValueId, AddConst> = HashMap::new();
    // Map: ValueId → the op that defines it (opcode, operands).
    // Used to trace comparison ops from CondBranch conditions.
    let mut value_def_op: HashMap<ValueId, (OpCode, Vec<ValueId>)> = HashMap::new();
    // Map: loop header BlockId → GuardFact from while-loop condition.
    let mut while_guard_facts: HashMap<BlockId, GuardFact> = HashMap::new();

    // First pass: collect definitions across all blocks.
    for bid in &block_ids {
        if let Some(block) = func.blocks.get(bid) {
            for op in &block.ops {
                match op.opcode {
                    OpCode::GetIter => {
                        if !op.operands.is_empty() && !op.results.is_empty() {
                            get_iter_source.insert(op.results[0], op.operands[0]);
                        }
                    }
                    OpCode::CallBuiltin => {
                        let name = op
                            .attrs
                            .get("name")
                            .and_then(|v| {
                                if let AttrValue::Str(s) = v {
                                    Some(s.clone())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_default();
                        let first_arg = op.operands.first().copied();
                        for &result in &op.results {
                            call_builtin_info.insert(result, (name.clone(), first_arg));
                        }
                    }
                    OpCode::BuildList => {
                        // BuildList with N operands produces a list of length N.
                        let len = op.operands.len() as i64;
                        for &result in &op.results {
                            container_length.insert(result, KnownLength::Constant(len));
                        }
                    }
                    OpCode::Mul => {
                        // list_repeat: Mul(list_1_elem, count) -> container
                        // of length `count`.  Only fires when one operand is a
                        // BuildList with exactly 1 element.
                        if op.operands.len() == 2 && !op.results.is_empty() {
                            let (a, b) = (op.operands[0], op.operands[1]);
                            let list_count_pair =
                                if container_length.get(&a).is_some_and(|l| matches!(l, KnownLength::Constant(1))) {
                                    Some(b)
                                } else if container_length.get(&b).is_some_and(|l| matches!(l, KnownLength::Constant(1))) {
                                    Some(a)
                                } else {
                                    None
                                };
                            if let Some(count_val) = list_count_pair {
                                for &result in &op.results {
                                    if let Some(&c) = const_int_value.get(&count_val) {
                                        container_length.insert(result, KnownLength::Constant(c));
                                    } else {
                                        container_length.insert(result, KnownLength::SameAs(count_val));
                                    }
                                }
                            }
                        }
                    }
                    OpCode::Add => {
                        // Track Add(base, const) decomposition for matching
                        // container length = n+1 with loop guard i <= n.
                        if op.operands.len() == 2 && !op.results.is_empty() {
                            let (a, b) = (op.operands[0], op.operands[1]);
                            if let Some(&cv) = const_int_value.get(&b) {
                                for &result in &op.results {
                                    add_const_decomp.insert(result, AddConst { base: a, offset: cv });
                                }
                            } else if let Some(&cv) = const_int_value.get(&a) {
                                for &result in &op.results {
                                    add_const_decomp.insert(result, AddConst { base: b, offset: cv });
                                }
                            }
                        }
                    }
                    _ => {}
                }
                // Record all value definitions for tracing comparison ops.
                for &result in &op.results {
                    value_def_op.insert(result, (op.opcode, op.operands.clone()));
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Analysis Phase 3: detect range-loop induction variables
    // -----------------------------------------------------------------------
    // For each loop header, find IterNextUnboxed/ForIter ops.  Trace the
    // iterator back through GetIter → CallBuiltin("range", N) to establish
    // that the produced element is in [0, N).

    let loop_headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| {
            if *role == LoopRole::LoopHeader {
                Some(*bid)
            } else {
                None
            }
        })
        .collect();

    // Set of blocks that belong to each loop (header → body blocks).
    let mut loop_body_blocks: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();

    for &header in &loop_headers {
        let body = collect_loop_body(func, header);
        let body_set: HashSet<BlockId> = body.iter().copied().collect();

        // Find IterNextUnboxed/ForIter ops in the loop header or body that
        // produce the induction variable.
        for &bid in &body {
            if let Some(block) = func.blocks.get(&bid) {
                for op in &block.ops {
                    let is_iter_op = matches!(
                        op.opcode,
                        OpCode::IterNextUnboxed | OpCode::ForIter | OpCode::IterNext
                    );
                    if !is_iter_op || op.operands.is_empty() || op.results.is_empty() {
                        continue;
                    }

                    let iter_val = op.operands[0];
                    // The element value is results[0].
                    let elem_val = op.results[0];

                    // Trace: iter_val → GetIter(source) → CallBuiltin("range", N).
                    let source = match get_iter_source.get(&iter_val) {
                        Some(&s) => s,
                        None => continue,
                    };
                    let (callee, first_arg) = match call_builtin_info.get(&source) {
                        Some(info) => info,
                        None => continue,
                    };
                    // Only match single-argument range(N) — not range(start, stop) or
                    // range(start, stop, step) since those may produce negative values.
                    if callee != "range" && callee != "builtin_range" && callee != "molt_range" {
                        continue;
                    }
                    // For range(N) with a single argument, we need exactly 1 operand
                    // on the CallBuiltin.  range(start, stop) has 2 operands and may
                    // start at a negative value.
                    let bound_val = match first_arg {
                        Some(v) => *v,
                        None => continue,
                    };

                    // Verify the range call has exactly 1 operand (range(N), not range(a,b)).
                    if let Some(info_source) = call_builtin_info.get(&source) {
                        // Check the original CallBuiltin op operand count.
                        let mut operand_count = 0;
                        for bid2 in &block_ids {
                            if let Some(b) = func.blocks.get(bid2) {
                                for op2 in &b.ops {
                                    if op2.opcode == OpCode::CallBuiltin
                                        && !op2.results.is_empty()
                                        && op2.results[0] == source
                                    {
                                        operand_count = op2.operands.len();
                                    }
                                }
                            }
                        }
                        // range(N) has 1 operand.  range(start, stop) has 2.
                        // range(start, stop, step) has 3.  Only range(N) guarantees
                        // the induction variable is in [0, N).
                        if operand_count != 1 {
                            continue;
                        }
                        let _ = info_source; // suppress unused warning
                    }

                    // Verify N > 0 if it's a constant (range(0) produces no iterations,
                    // range(-5) produces no iterations — both are safe vacuously).
                    // If N is non-constant, we still know i is in [0, N) within the loop.
                    if let Some(&n_const) = const_int_value.get(&bound_val)
                        && n_const <= 0
                    {
                        // range(0) or range(negative) → loop never executes, no
                        // bounds checks to eliminate.
                        continue;
                    }

                    range_facts.insert(
                        elem_val,
                        RangeFact {
                            upper_bound: bound_val,
                        },
                    );
                }
            }
        }

        loop_body_blocks.insert(header, body_set);
    }

    // -----------------------------------------------------------------------
    // Analysis Phase 4: while-loop guard facts
    // -----------------------------------------------------------------------
    // For each loop header with a CondBranch terminator, trace the condition
    // back to a comparison op (Le or Lt).  In the TRUE successor (loop body),
    // the comparison is known to hold:
    //   Le(i, n) → i <= n → i < n+1
    //   Lt(i, n) → i < n

    for &header in &loop_headers {
        if let Some(block) = func.blocks.get(&header) {
            let cond_val = match &block.terminator {
                Terminator::CondBranch { cond, .. } => *cond,
                _ => continue,
            };
            // Trace the condition to a comparison op.
            if let Some((opcode, operands)) = value_def_op.get(&cond_val) {
                if operands.len() == 2 {
                    match opcode {
                        OpCode::Le => {
                            while_guard_facts.insert(header, GuardFact {
                                var: operands[0],
                                bound: operands[1],
                                is_le: true,
                            });
                        }
                        OpCode::Lt => {
                            while_guard_facts.insert(header, GuardFact {
                                var: operands[0],
                                bound: operands[1],
                                is_le: false,
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Transform Phase: annotate Index ops
    // -----------------------------------------------------------------------

    // Build a reverse map: block → which loop header(s) it belongs to.
    let mut block_to_loop: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for (header, body) in &loop_body_blocks {
        for &bid in body {
            block_to_loop.entry(bid).or_default().push(*header);
        }
    }

    for bid in &block_ids {
        if let Some(block) = func.blocks.get_mut(bid) {
            for op in block.ops.iter_mut() {
                if op.opcode != OpCode::Index && op.opcode != OpCode::StoreIndex {
                    continue;
                }
                // Already marked by a previous phase or pass.
                if op.attrs.contains_key("bce_safe") {
                    continue;
                }
                let container_operand = match op.operands.first() {
                    Some(&v) => v,
                    None => continue,
                };
                let index_operand = match op.operands.get(1) {
                    Some(&v) => v,
                    None => continue,
                };

                // --- Constant-index BCE (Phase 1 logic) ---
                // A non-negative constant index is marked bce_safe.  This
                // tells codegen the index does not need negative-wraparound
                // handling.  The runtime `molt_getitem_unchecked` still
                // performs a type dispatch that is safe for non-negative
                // indices on standard container types.
                if let Some(&const_val) = const_int_value.get(&index_operand) {
                    if const_val >= 0 {
                        op.attrs
                            .insert("bce_safe".to_string(), AttrValue::Bool(true));
                        stats.values_changed += 1;
                    }
                    continue;
                }

                // --- Inductive range BCE (Phase 2 logic) ---
                // Check if the index has a range fact from a for-range loop.
                let mut proven = false;
                if let Some(fact) = range_facts.get(&index_operand) {
                    let in_loop = block_to_loop
                        .get(bid)
                        .map(|headers| !headers.is_empty())
                        .unwrap_or(false);
                    if in_loop {
                        // Prove: len(container) >= upper_bound.
                        let upper_bound = fact.upper_bound;
                        proven = match container_length.get(&container_operand) {
                            Some(KnownLength::Constant(len)) => {
                                if let Some(&bound_const) = const_int_value.get(&upper_bound) {
                                    *len >= bound_const
                                } else {
                                    false
                                }
                            }
                            Some(KnownLength::SameAs(len_val)) => {
                                *len_val == upper_bound
                            }
                            None => false,
                        };
                    }
                }

                // --- While-loop guard BCE (Phase 3 logic) ---
                // Check if the index is bounded by a while-loop condition
                // (Le or Lt) and the container length matches.
                if !proven {
                    if let Some(headers) = block_to_loop.get(bid) {
                        for &header in headers {
                            if let Some(guard) = while_guard_facts.get(&header) {
                                if guard.var != index_operand {
                                    continue;
                                }
                                // guard.var <= guard.bound (Le) or
                                // guard.var < guard.bound (Lt)
                                // We need: index < len(container).
                                //
                                // For Le(i, n): i <= n, so i < n+1.
                                //   Proven if len(container) >= n+1.
                                // For Lt(i, n): i < n.
                                //   Proven if len(container) >= n.
                                proven = prove_guard_bound(
                                    guard,
                                    container_operand,
                                    &container_length,
                                    &const_int_value,
                                    &add_const_decomp,
                                );
                                if proven {
                                    break;
                                }
                            }
                        }
                    }
                }

                if proven {
                    op.attrs
                        .insert("bce_safe".to_string(), AttrValue::Bool(true));
                    stats.values_changed += 1;
                }
            }
        }
    }

    stats
}

/// Prove that a while-loop guard fact implies the index is in bounds.
///
/// For `Le(i, n)`: we need `len(container) > n`, i.e. `len >= n+1`.
/// For `Lt(i, n)`: we need `len(container) > i`, i.e. `len >= n`.
///
/// Matching strategies:
///   1. `len = SameAs(v)` where `v = Add(guard.bound, 1)` and guard is Le.
///   2. `len = Constant(c)` where `c > guard.bound` (const bound).
///   3. `len = SameAs(v)` where `v == guard.bound` and guard is Lt.
fn prove_guard_bound(
    guard: &GuardFact,
    container_operand: ValueId,
    container_length: &HashMap<ValueId, KnownLength>,
    const_int_value: &HashMap<ValueId, i64>,
    add_const_decomp: &HashMap<ValueId, AddConst>,
) -> bool {
    let len = match container_length.get(&container_operand) {
        Some(l) => l,
        None => return false,
    };

    match len {
        KnownLength::Constant(len_const) => {
            // Container has a known constant length.
            if let Some(&bound_const) = const_int_value.get(&guard.bound) {
                if guard.is_le {
                    // i <= bound, need len > bound, i.e. len >= bound + 1.
                    *len_const >= bound_const + 1
                } else {
                    // i < bound, need len >= bound.
                    *len_const >= bound_const
                }
            } else {
                false
            }
        }
        KnownLength::SameAs(len_val) => {
            if guard.is_le {
                // Guard: i <= n.  Need: len(container) >= n+1.
                // Check if len_val = Add(n, 1) where n = guard.bound.
                if let Some(decomp) = add_const_decomp.get(len_val) {
                    decomp.base == guard.bound && decomp.offset >= 1
                } else if *len_val == guard.bound {
                    // len == n, but we need len > n for Le. Not proven.
                    false
                } else {
                    false
                }
            } else {
                // Guard: i < n.  Need: len(container) >= n.
                // Proven if len_val == guard.bound (same SSA value).
                *len_val == guard.bound
            }
        }
    }
}

/// Collect all blocks that belong to a loop body rooted at `header`.
/// Uses the same logic as `loop_narrow::collect_loop_body`.
fn collect_loop_body(func: &TirFunction, header: BlockId) -> Vec<BlockId> {

    let mut ordered_blocks: Vec<BlockId> = func.blocks.keys().copied().collect();
    ordered_blocks.sort_by_key(|bid| bid.0);

    let mut body = vec![header];
    for bid in ordered_blocks {
        if bid == header || bid.0 <= header.0 {
            continue;
        }

        let role = func.loop_roles.get(&bid).cloned().unwrap_or(LoopRole::None);
        if role == LoopRole::LoopHeader {
            break;
        }

        body.push(bid);

        if let Some(block) = func.blocks.get(&bid) {
            let branches_to_header = match &block.terminator {
                Terminator::Branch { target, .. } => *target == header,
                Terminator::CondBranch {
                    then_block,
                    else_block,
                    ..
                } => *then_block == header || *else_block == header,
                _ => false,
            };
            if branches_to_header {
                break;
            }
        }
    }

    body
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_const_int(result: ValueId, value: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    fn make_call_builtin(name: &str, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str(name.into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    // Build a minimal function with a single entry block containing the
    // given ops, terminated by `Return { values: [] }`.
    fn func_with_ops(ops: Vec<TirOp>) -> TirFunction {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return { values: vec![] };
        func
    }

    // ------------------------------------------------------------------
    // Test 1: constant index >= 0 → marked bce_safe
    // ------------------------------------------------------------------
    #[test]
    fn constant_zero_index_marked_safe() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let container = func.fresh_value();
        let idx = func.fresh_value();
        let result = func.fresh_value();

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_const_int(idx, 0),
            make_op(OpCode::Index, vec![container, idx], vec![result]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);

        assert_eq!(stats.values_changed, 1);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index op with const 0 index must be marked bce_safe"
        );
    }

    // ------------------------------------------------------------------
    // Test 2: positive constant index → marked bce_safe
    // ------------------------------------------------------------------
    #[test]
    fn positive_constant_index_marked_safe() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let container = func.fresh_value();
        let idx = func.fresh_value();
        let result = func.fresh_value();

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_const_int(idx, 42),
            make_op(OpCode::Index, vec![container, idx], vec![result]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 1);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert_eq!(index_op.attrs.get("bce_safe"), Some(&AttrValue::Bool(true)));
    }

    // ------------------------------------------------------------------
    // Test 3: negative constant index → NOT marked
    // ------------------------------------------------------------------
    #[test]
    fn negative_constant_index_not_marked() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let container = func.fresh_value();
        let idx = func.fresh_value();
        let result = func.fresh_value();

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_const_int(idx, -1),
            make_op(OpCode::Index, vec![container, idx], vec![result]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "Negative constant must not be marked bce_safe"
        );
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "bce_safe must be absent for negative index"
        );
    }

    // ------------------------------------------------------------------
    // Test 4: non-constant index → NOT marked
    // ------------------------------------------------------------------
    #[test]
    fn non_constant_index_not_marked() {
        // Index operand comes from a function parameter — not a ConstInt.
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::None);
        let container = func.fresh_value();
        let result = func.fresh_value();
        let param_idx = ValueId(0); // function parameter

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_op(OpCode::Index, vec![container, param_idx], vec![result]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert!(!index_op.attrs.contains_key("bce_safe"));
    }

    // ------------------------------------------------------------------
    // Test 5: no Index ops → no changes, no panic
    // ------------------------------------------------------------------
    #[test]
    fn no_index_ops_no_changes() {
        let ops = vec![]; // empty body
        let mut func = func_with_ops(ops);
        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
    }

    // ------------------------------------------------------------------
    // Test 6: mixed ops — only constant non-negative indices marked
    // ------------------------------------------------------------------
    #[test]
    fn mixed_indices_only_safe_ones_marked() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::None);
        let container = func.fresh_value();
        let const_idx = func.fresh_value(); // ConstInt(5) → safe
        let neg_idx = func.fresh_value(); // ConstInt(-2) → unsafe
        let param_idx = ValueId(0); // parameter → unsafe

        let r0 = func.fresh_value();
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_const_int(const_idx, 5),
            make_const_int(neg_idx, -2),
            // Index with const non-negative → should be marked
            make_op(OpCode::Index, vec![container, const_idx], vec![r0]),
            // Index with const negative → should NOT be marked
            make_op(OpCode::Index, vec![container, neg_idx], vec![r1]),
            // Index with non-constant → should NOT be marked
            make_op(OpCode::Index, vec![container, param_idx], vec![r2]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![r0, r1, r2],
        };

        let stats = run(&mut func);
        assert_eq!(
            stats.values_changed, 1,
            "Only one Index should be marked bce_safe"
        );

        let index_ops: Vec<_> = func.blocks[&func.entry_block]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::Index)
            .collect();

        assert_eq!(index_ops.len(), 3);
        // First Index (const_idx = 5) → safe
        assert_eq!(
            index_ops[0].attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true))
        );
        // Second Index (neg_idx = -2) → not safe
        assert!(!index_ops[1].attrs.contains_key("bce_safe"));
        // Third Index (param) → not safe
        assert!(!index_ops[2].attrs.contains_key("bce_safe"));
    }

    // ==================================================================
    // Inductive range analysis tests
    // ==================================================================

    /// Helper to build a multi-block function simulating:
    ///
    /// ```python
    /// a = BuildList(elem0, elem1, ..., elem_{n-1})   # length = n
    /// range_obj = CallBuiltin("range", N)
    /// iter = GetIter(range_obj)
    /// # loop header:
    ///   i = IterNextUnboxed(iter)  → (elem, done)
    ///   if done: goto exit
    /// # loop body:
    ///   a[i]
    /// ```
    ///
    /// `list_len`: number of operands to BuildList (determines known length).
    /// `range_bound`: constant value for N in range(N).
    /// `use_negative_index`: if true, uses a ConstInt(-1) as the index instead
    ///                       of the induction variable.
    fn build_range_loop_func(
        list_len: usize,
        range_bound: i64,
        use_negative_index: bool,
    ) -> (TirFunction, BlockId, BlockId, BlockId) {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        // --- Entry block (bb0): build list, call range, get_iter, branch to header ---
        let mut list_elems: Vec<ValueId> = Vec::new();
        for _ in 0..list_len {
            list_elems.push(func.fresh_value());
        }
        let container = func.fresh_value();
        let bound_val = func.fresh_value();
        let range_obj = func.fresh_value();
        let iter_val = func.fresh_value();

        let mut entry_ops: Vec<TirOp> = Vec::new();
        // Create const int elements for the list.
        for &elem in &list_elems {
            entry_ops.push(make_const_int(elem, 0));
        }
        entry_ops.push(make_op(OpCode::BuildList, list_elems, vec![container]));
        entry_ops.push(make_const_int(bound_val, range_bound));
        entry_ops.push(make_call_builtin("range", vec![bound_val], vec![range_obj]));
        entry_ops.push(make_op(OpCode::GetIter, vec![range_obj], vec![iter_val]));

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        let entry = func.blocks.get_mut(&entry_block).unwrap();
        entry.ops = entry_ops;
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };

        // --- Loop header (bb1): IterNextUnboxed, cond branch ---
        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();

        let header_block = TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::IterNextUnboxed,
                vec![iter_val],
                vec![elem_val, done_val],
            )],
            terminator: Terminator::CondBranch {
                cond: done_val,
                then_block: exit_id,
                then_args: vec![],
                else_block: body_id,
                else_args: vec![],
            },
        };
        func.blocks.insert(header_id, header_block);
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        // --- Loop body (bb2): Index op, branch back to header ---
        let index_operand = if use_negative_index {
            let neg_idx = func.fresh_value();
            let body_ops = vec![
                make_const_int(neg_idx, -1),
                make_op(
                    OpCode::Index,
                    vec![container, neg_idx],
                    vec![func.fresh_value()],
                ),
            ];
            let body_block = TirBlock {
                id: body_id,
                args: vec![],
                ops: body_ops,
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![],
                },
            };
            func.blocks.insert(body_id, body_block);
            neg_idx
        } else {
            let index_result = func.fresh_value();
            let body_block = TirBlock {
                id: body_id,
                args: vec![],
                ops: vec![make_op(
                    OpCode::Index,
                    vec![container, elem_val],
                    vec![index_result],
                )],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![],
                },
            };
            func.blocks.insert(body_id, body_block);
            elem_val
        };
        let _ = index_operand;

        // --- Exit block (bb3): return ---
        let exit_block = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        func.blocks.insert(exit_id, exit_block);
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        (func, header_id, body_id, exit_id)
    }

    // ------------------------------------------------------------------
    // Test 7: range loop bounds elimination — a[i] in for i in range(N)
    //         where len(a) == N → bce_safe
    // ------------------------------------------------------------------
    #[test]
    fn range_loop_index_eliminated() {
        // a = [0, 0, 0, 0, 0]  (len=5)
        // for i in range(5): a[i]  → should be bce_safe
        let (mut func, _header, body_id, _exit) = build_range_loop_func(5, 5, false);

        let stats = run(&mut func);

        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present in loop body");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in range loop with matching container length must be bce_safe"
        );
        assert!(
            stats.values_changed >= 1,
            "At least one Index should be marked bce_safe"
        );
    }

    // ------------------------------------------------------------------
    // Test 8: negative index inside range loop → NOT marked
    // ------------------------------------------------------------------
    #[test]
    fn range_loop_negative_index_preserved() {
        // a = [0, 0, 0, 0, 0]  (len=5)
        // for i in range(5): a[-1]  → must NOT be bce_safe
        let (mut func, _header, body_id, _exit) = build_range_loop_func(5, 5, true);

        let stats = run(&mut func);

        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present in loop body");
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "Negative index inside range loop must NOT be marked bce_safe"
        );
        // The negative constant index should not be marked either.
        assert_eq!(
            stats.values_changed, 0,
            "No Index ops should be marked bce_safe with negative index"
        );
    }

    // ------------------------------------------------------------------
    // Test 9: non-range loop index → NOT marked (parameter-driven loop)
    // ------------------------------------------------------------------
    #[test]
    fn non_range_loop_index_preserved() {
        // Simulate a loop where the iterator comes from a parameter, not range().
        // The induction variable has no range fact.
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);

        let param_iter = ValueId(0); // function parameter used as iterator
        let container = func.fresh_value();
        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();
        let index_result = func.fresh_value();

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        // Entry: build a 5-element list, branch to header.
        let mut list_elems = Vec::new();
        for _ in 0..5 {
            list_elems.push(func.fresh_value());
        }
        let mut entry_ops: Vec<TirOp> = Vec::new();
        for &elem in &list_elems {
            entry_ops.push(make_const_int(elem, 0));
        }
        entry_ops.push(make_op(OpCode::BuildList, list_elems, vec![container]));

        let entry = func.blocks.get_mut(&entry_block).unwrap();
        entry.ops = entry_ops;
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };

        // Header: IterNextUnboxed from the parameter iterator.
        let header_block = TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::IterNextUnboxed,
                vec![param_iter],
                vec![elem_val, done_val],
            )],
            terminator: Terminator::CondBranch {
                cond: done_val,
                then_block: exit_id,
                then_args: vec![],
                else_block: body_id,
                else_args: vec![],
            },
        };
        func.blocks.insert(header_id, header_block);
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        // Body: a[i] where i is the element from the non-range iterator.
        let body_block = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::Index,
                vec![container, elem_val],
                vec![index_result],
            )],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        };
        func.blocks.insert(body_id, body_block);

        let exit_block = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        func.blocks.insert(exit_id, exit_block);
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        let stats = run(&mut func);

        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "Index with non-range iterator must NOT be marked bce_safe"
        );
        assert_eq!(stats.values_changed, 0);
    }

    // ------------------------------------------------------------------
    // Test 10: range loop where container is too small → NOT marked
    // ------------------------------------------------------------------
    #[test]
    fn range_loop_container_too_small_not_marked() {
        // a = [0, 0, 0]  (len=3)
        // for i in range(5): a[i]  → container too small, must NOT be bce_safe
        let (mut func, _header, body_id, _exit) = build_range_loop_func(3, 5, false);

        let stats = run(&mut func);

        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present in loop body");
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "Index in range loop with container smaller than bound must NOT be bce_safe"
        );
        assert_eq!(stats.values_changed, 0);
    }

    // ------------------------------------------------------------------
    // Test 11: range loop with container larger than bound → marked
    // ------------------------------------------------------------------
    #[test]
    fn range_loop_container_larger_than_bound() {
        // a = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]  (len=10)
        // for i in range(5): a[i]  → container is larger, should be bce_safe
        let (mut func, _header, body_id, _exit) = build_range_loop_func(10, 5, false);

        let stats = run(&mut func);

        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in range loop with oversized container must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }

    // ------------------------------------------------------------------
    // Test 12: while-loop guard BCE for sieve pattern (StoreIndex)
    // ------------------------------------------------------------------
    #[test]
    fn while_loop_guard_le_store_index_marked_safe() {
        let mut func = TirFunction::new("sieve".into(), vec![], TirType::None);

        let n = func.fresh_value();
        let const_1 = func.fresh_value();
        let n_plus_1 = func.fresh_value();
        let true_val = func.fresh_value();
        let list_1 = func.fresh_value();
        let is_prime = func.fresh_value();
        let false_val = func.fresh_value();
        let i_phi = func.fresh_value();
        let cond = func.fresh_value();
        let i_next = func.fresh_value();

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&entry_block).unwrap();
            entry.ops = vec![
                make_const_int(n, 100000),
                make_const_int(const_1, 1),
                make_op(OpCode::Add, vec![n, const_1], vec![n_plus_1]),
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_op(OpCode::Mul, vec![list_1, n_plus_1], vec![is_prime]),
            ];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        {
            let header_block = TirBlock {
                id: header_id,
                args: vec![],
                ops: vec![make_op(OpCode::Le, vec![i_phi, n], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body_id,
                    then_args: vec![],
                    else_block: exit_id,
                    else_args: vec![],
                },
            };
            func.blocks.insert(header_id, header_block);
            func.loop_roles.insert(header_id, LoopRole::LoopHeader);
        }

        {
            let body_block = TirBlock {
                id: body_id,
                args: vec![],
                ops: vec![
                    make_const_int(false_val, 0),
                    make_op(
                        OpCode::StoreIndex,
                        vec![is_prime, i_phi, false_val],
                        vec![],
                    ),
                    make_op(OpCode::Add, vec![i_phi, const_1], vec![i_next]),
                ],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![],
                },
            };
            func.blocks.insert(body_id, body_block);
        }

        {
            let exit_block = TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            };
            func.blocks.insert(exit_id, exit_block);
            func.loop_roles.insert(exit_id, LoopRole::LoopEnd);
        }

        let stats = run(&mut func);

        let store_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::StoreIndex)
            .expect("StoreIndex op must be present");
        assert_eq!(
            store_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "StoreIndex in while(i<=n) with is_prime=[True]*(n+1) must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }

    // ------------------------------------------------------------------
    // Test 13: while-loop guard BCE for Index (getitem)
    // ------------------------------------------------------------------
    #[test]
    fn while_loop_guard_le_index_marked_safe() {
        let mut func = TirFunction::new("sieve_get".into(), vec![], TirType::None);

        let n = func.fresh_value();
        let const_1 = func.fresh_value();
        let n_plus_1 = func.fresh_value();
        let true_val = func.fresh_value();
        let list_1 = func.fresh_value();
        let is_prime = func.fresh_value();
        let i_phi = func.fresh_value();
        let cond = func.fresh_value();
        let elem = func.fresh_value();
        let i_next = func.fresh_value();

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&entry_block).unwrap();
            entry.ops = vec![
                make_const_int(n, 100000),
                make_const_int(const_1, 1),
                make_op(OpCode::Add, vec![n, const_1], vec![n_plus_1]),
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_op(OpCode::Mul, vec![list_1, n_plus_1], vec![is_prime]),
            ];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        {
            let header_block = TirBlock {
                id: header_id,
                args: vec![],
                ops: vec![make_op(OpCode::Le, vec![i_phi, n], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body_id,
                    then_args: vec![],
                    else_block: exit_id,
                    else_args: vec![],
                },
            };
            func.blocks.insert(header_id, header_block);
            func.loop_roles.insert(header_id, LoopRole::LoopHeader);
        }

        {
            let body_block = TirBlock {
                id: body_id,
                args: vec![],
                ops: vec![
                    make_op(OpCode::Index, vec![is_prime, i_phi], vec![elem]),
                    make_op(OpCode::Add, vec![i_phi, const_1], vec![i_next]),
                ],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![],
                },
            };
            func.blocks.insert(body_id, body_block);
        }

        {
            let exit_block = TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            };
            func.blocks.insert(exit_id, exit_block);
            func.loop_roles.insert(exit_id, LoopRole::LoopEnd);
        }

        let stats = run(&mut func);

        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in while(i<=n) with container=[True]*(n+1) must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }

    // ------------------------------------------------------------------
    // Test 14: Lt guard with matching container length
    // ------------------------------------------------------------------
    #[test]
    fn while_loop_guard_lt_index_marked_safe() {
        let mut func = TirFunction::new("lt_test".into(), vec![], TirType::None);

        let n = func.fresh_value();
        let true_val = func.fresh_value();
        let list_1 = func.fresh_value();
        let container = func.fresh_value();
        let i_phi = func.fresh_value();
        let cond = func.fresh_value();
        let elem = func.fresh_value();
        let const_1 = func.fresh_value();
        let i_next = func.fresh_value();

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&entry_block).unwrap();
            entry.ops = vec![
                make_const_int(n, 100),
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_op(OpCode::Mul, vec![list_1, n], vec![container]),
            ];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        {
            let header_block = TirBlock {
                id: header_id,
                args: vec![],
                ops: vec![make_op(OpCode::Lt, vec![i_phi, n], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body_id,
                    then_args: vec![],
                    else_block: exit_id,
                    else_args: vec![],
                },
            };
            func.blocks.insert(header_id, header_block);
            func.loop_roles.insert(header_id, LoopRole::LoopHeader);
        }

        {
            let body_block = TirBlock {
                id: body_id,
                args: vec![],
                ops: vec![
                    make_op(OpCode::Index, vec![container, i_phi], vec![elem]),
                    make_const_int(const_1, 1),
                    make_op(OpCode::Add, vec![i_phi, const_1], vec![i_next]),
                ],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![],
                },
            };
            func.blocks.insert(body_id, body_block);
        }

        {
            let exit_block = TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            };
            func.blocks.insert(exit_id, exit_block);
            func.loop_roles.insert(exit_id, LoopRole::LoopEnd);
        }

        let stats = run(&mut func);

        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in while(i<n) with container of length n must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }
}
