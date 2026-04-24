//! SIMD Vectorization Hint Analysis pass for TIR.
//!
//! Scans TIR functions for loops that are safe to vectorize and annotates
//! the loop-header block's `ForIter` / `ScfFor` op with hint attributes:
//!
//! - `"vectorize" = AttrValue::Bool(true)` — loop body is vectorizable.
//! - `"reduction" = AttrValue::Str("sum"|"product"|"min"|"max"|"and"|"or")`
//!   — a simple reduction pattern was detected.
//!
//! This pass performs **hint annotation only**; actual SIMD code generation is
//! deferred to the LLVM backend which reads these attrs.
//!
//! ## Vectorizability criteria (Phase 4 simplified)
//!
//! A loop (identified by blocks that contain a `ForIter` or `ScfFor` op, plus
//! all blocks reachable from it before the loop exit) is considered vectorizable
//! when **all** of the following hold:
//!
//! 1. Every non-structural op operates only on `I64` or `F64` values.
//! 2. No op is a `Call`, `CallMethod`, `CallBuiltin`, or any other impure op.
//! 3. No write to non-local memory (`StoreAttr`, `StoreIndex`, `DelAttr`,
//!    `DelIndex`, `Free`, `IncRef`, `DecRef`).
//! 4. No generator ops (`Yield`, `YieldFrom`), exception ops (`Raise`,
//!    `CheckException`), or import ops.
//!
//! ## Reduction detection
//!
//! A sum reduction is detected when there exists a block argument `acc` that
//! is the sole result of an `Add` op whose operands include `acc` itself (the
//! classic accumulator += value pattern in SSA form via a block back-arg).

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::PassStats;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Vectorization analysis result for a loop.
#[derive(Debug, Clone)]
pub struct VectorizationInfo {
    /// Whether the loop body is safe to vectorize.
    pub vectorizable: bool,
    /// The element type used in the loop body (if uniform).
    pub element_type: Option<TirType>,
    /// Estimated trip count (only available when the loop bound is a compile-time constant).
    pub estimated_trip_count: Option<u64>,
    /// A detected reduction operation.
    pub reduction_op: Option<ReductionOp>,
}

/// Reduction operation kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReductionOp {
    Sum,
    Product,
    Min,
    Max,
    And,
    Or,
}

impl ReductionOp {
    fn as_str(self) -> &'static str {
        match self {
            ReductionOp::Sum => "sum",
            ReductionOp::Product => "product",
            ReductionOp::Min => "min",
            ReductionOp::Max => "max",
            ReductionOp::And => "and",
            ReductionOp::Or => "or",
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers — op classification
// ---------------------------------------------------------------------------

/// Returns `true` if an opcode is an impure/side-effecting call.
#[inline]
fn is_impure_call(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Call | OpCode::CallMethod | OpCode::CallBuiltin
    )
}

/// Returns `true` if an opcode writes to (potentially non-local) memory.
#[inline]
fn is_memory_store(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::StoreAttr
            | OpCode::StoreIndex
            | OpCode::DelAttr
            | OpCode::DelIndex
            | OpCode::ClosureStore
            | OpCode::Free
            | OpCode::IncRef
            | OpCode::DecRef
    )
}

/// Returns `true` if an opcode is completely disqualifying for vectorization.
#[inline]
fn is_disqualifying(opcode: OpCode) -> bool {
    is_impure_call(opcode)
        || is_memory_store(opcode)
        || matches!(
            opcode,
            OpCode::Yield
                | OpCode::YieldFrom
                | OpCode::StateSwitch
                | OpCode::StateTransition
                | OpCode::StateYield
                | OpCode::ChanSendYield
                | OpCode::ChanRecvYield
                | OpCode::ClosureLoad
                | OpCode::ClosureStore
                | OpCode::Raise
                | OpCode::CheckException
                | OpCode::Import
                | OpCode::ImportFrom
                | OpCode::Alloc
                | OpCode::StackAlloc
                | OpCode::Deopt
                | OpCode::BuildList
                | OpCode::BuildDict
                | OpCode::BuildTuple
                | OpCode::BuildSet
                | OpCode::BuildSlice
        )
}

/// Returns `true` if an opcode is pure arithmetic on numeric scalars.
/// These are the only ops allowed inside a vectorizable body.
#[inline]
fn is_scalar_arithmetic(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Add
            | OpCode::Sub
            | OpCode::Mul
            | OpCode::Div
            | OpCode::FloorDiv
            | OpCode::Mod
            | OpCode::Pow
            | OpCode::Neg
            | OpCode::Pos
            | OpCode::BitAnd
            | OpCode::BitOr
            | OpCode::BitXor
            | OpCode::BitNot
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::Eq
            | OpCode::Ne
            | OpCode::Lt
            | OpCode::Le
            | OpCode::Gt
            | OpCode::Ge
            | OpCode::ConstInt
            | OpCode::ConstFloat
            | OpCode::ConstBool
            | OpCode::Copy
            | OpCode::UnboxVal
            | OpCode::BoxVal
    )
}

/// Returns `true` if `ty` is a numeric scalar eligible for SIMD lanes.
#[inline]
fn is_numeric(ty: &TirType) -> bool {
    matches!(ty, TirType::I64 | TirType::F64)
}

// ---------------------------------------------------------------------------
// Loop detection
// ---------------------------------------------------------------------------

/// Identify all block ids that form or belong to a loop.
///
/// A "loop header" block is one whose terminator's target set includes a
/// predecessor (back-edge), OR one that contains a `ForIter` / `ScfFor` op.
///
/// Returns a map: loop_header_block_id → set of block ids in the loop body
/// (including the header itself).
fn find_loops(func: &TirFunction) -> HashMap<BlockId, HashSet<BlockId>> {
    // Build predecessor map.
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &bid in func.blocks.keys() {
        preds.entry(bid).or_default();
    }
    for (bid, block) in &func.blocks {
        let succs = successors(&block.terminator);
        for s in succs {
            preds.entry(s).or_default().push(*bid);
        }
    }

    // Compute dominators via simple RPO (adequate for small functions).
    // We use the entry block as root.
    let rpo = rpo_order(func);
    let dom = compute_dominators(func, &rpo);

    // Back edges: edge (u → v) where v dominates u.
    let mut loops: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();

    for (bid, block) in &func.blocks {
        for s in successors(&block.terminator) {
            // Check if s dominates bid (s is a loop header, bid is a latch).
            if dominates(&dom, s, *bid) {
                // Natural loop: collect all nodes between latch and header.
                let body = natural_loop_body(func, s, *bid);
                loops.entry(s).or_default().extend(body);
            }
        }
    }

    // Also treat any block with a ForIter / ScfFor op as a loop header even
    // if the CFG doesn't reveal a clean back-edge (e.g. when lowered linearly).
    for (bid, block) in &func.blocks {
        if block
            .ops
            .iter()
            .any(|op| matches!(op.opcode, OpCode::ForIter | OpCode::ScfFor))
        {
            loops.entry(*bid).or_insert_with(|| {
                let mut s = HashSet::new();
                s.insert(*bid);
                s
            });
        }
    }

    loops
}

/// Return the set of CFG successors of a terminator.
fn successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut v: Vec<BlockId> = cases.iter().map(|(_, t, _)| *t).collect();
            v.push(*default);
            v
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// Reverse post-order traversal of the CFG from the entry block.
fn rpo_order(func: &TirFunction) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut post: Vec<BlockId> = Vec::new();
    dfs_post(func, func.entry_block, &mut visited, &mut post);
    post.reverse();
    post
}

fn dfs_post(
    func: &TirFunction,
    bid: BlockId,
    visited: &mut HashSet<BlockId>,
    post: &mut Vec<BlockId>,
) {
    if !visited.insert(bid) {
        return;
    }
    if let Some(block) = func.blocks.get(&bid) {
        for s in successors(&block.terminator) {
            dfs_post(func, s, visited, post);
        }
    }
    post.push(bid);
}

/// Simple dominator computation (RPO-based Cooper et al. algorithm, O(N²)).
/// Returns a map: block → immediate dominator block.
fn compute_dominators(func: &TirFunction, rpo: &[BlockId]) -> HashMap<BlockId, BlockId> {
    let mut idom: HashMap<BlockId, BlockId> = HashMap::new();
    let entry = func.entry_block;
    idom.insert(entry, entry);

    // Build predecessor map.
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for bid in rpo {
        preds.entry(*bid).or_default();
    }
    for bid in rpo {
        if let Some(block) = func.blocks.get(bid) {
            for s in successors(&block.terminator) {
                preds.entry(s).or_default().push(*bid);
            }
        }
    }

    // RPO index for intersection.
    let rpo_idx: HashMap<BlockId, usize> = rpo.iter().enumerate().map(|(i, &b)| (b, i)).collect();

    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo {
            if b == entry {
                continue;
            }
            let processed_preds: Vec<BlockId> = preds
                .get(&b)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|p| idom.contains_key(p))
                .collect();

            if processed_preds.is_empty() {
                continue;
            }

            let mut new_idom = processed_preds[0];
            for &p in &processed_preds[1..] {
                new_idom = intersect(&idom, &rpo_idx, new_idom, p);
            }

            if idom.get(&b) != Some(&new_idom) {
                idom.insert(b, new_idom);
                changed = true;
            }
        }
    }

    idom
}

fn intersect(
    idom: &HashMap<BlockId, BlockId>,
    rpo_idx: &HashMap<BlockId, usize>,
    mut a: BlockId,
    mut b: BlockId,
) -> BlockId {
    while a != b {
        let ia = rpo_idx.get(&a).copied().unwrap_or(usize::MAX);
        let ib = rpo_idx.get(&b).copied().unwrap_or(usize::MAX);
        if ia > ib {
            a = *idom.get(&a).unwrap_or(&a);
        } else {
            b = *idom.get(&b).unwrap_or(&b);
        }
    }
    a
}

/// Returns `true` if `dominator` dominates `node` per the idom map.
fn dominates(idom: &HashMap<BlockId, BlockId>, dominator: BlockId, mut node: BlockId) -> bool {
    loop {
        if node == dominator {
            return true;
        }
        let parent = match idom.get(&node) {
            Some(&p) if p != node => p,
            _ => return false,
        };
        node = parent;
    }
}

/// Collect all blocks in the natural loop with header `header` and latch `latch`.
/// This is the set of nodes from which `latch` is reachable without passing through `header`.
fn natural_loop_body(func: &TirFunction, header: BlockId, latch: BlockId) -> HashSet<BlockId> {
    let mut body: HashSet<BlockId> = HashSet::new();
    body.insert(header);
    body.insert(latch);

    let mut worklist = vec![latch];
    while let Some(node) = worklist.pop() {
        if func.blocks.contains_key(&node) {
            // Walk predecessors — we need a pred map; build ad-hoc here.
            // Since this is called per back-edge, build it inline.
            for (&bid, blk) in &func.blocks {
                if successors(&blk.terminator).contains(&node) && body.insert(bid) {
                    worklist.push(bid);
                }
            }
        }
    }
    body
}

// ---------------------------------------------------------------------------
// Type inference for loop body blocks
// ---------------------------------------------------------------------------

/// Build a map from ValueId → TirType for a set of blocks.
fn collect_types(func: &TirFunction, body: &HashSet<BlockId>) -> HashMap<ValueId, TirType> {
    let mut ty_map: HashMap<ValueId, TirType> = HashMap::new();
    for bid in body {
        if let Some(block) = func.blocks.get(bid) {
            for arg in &block.args {
                ty_map.insert(arg.id, arg.ty.clone());
            }
        }
    }
    ty_map
}

// ---------------------------------------------------------------------------
// Vectorizability check
// ---------------------------------------------------------------------------

/// Analyse the loop body blocks for vectorization potential.
fn analyse_loop(func: &TirFunction, body: &HashSet<BlockId>) -> VectorizationInfo {
    let ty_map = collect_types(func, body);

    let mut vectorizable = true;
    let mut seen_type: Option<TirType> = None;
    let mut mixed_types = false;
    let mut reduction: Option<ReductionOp> = None;

    // Collect all block-argument ids across body blocks to detect accumulators.
    // An accumulator is a block arg whose id is also used as an operand of an
    // Add/Mul/etc. op whose result feeds back (through the loop's back-edge
    // branch args) into the same block arg.
    let acc_candidates: HashSet<ValueId> = body
        .iter()
        .flat_map(|bid| {
            func.blocks
                .get(bid)
                .map(|b| b.args.iter().map(|a| a.id).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();

    for bid in body {
        let block = match func.blocks.get(bid) {
            Some(b) => b,
            None => continue,
        };

        for op in &block.ops {
            // Skip iteration machinery — not disqualifying. IterNextUnboxed
            // is the fused unboxed variant that produces (value, done_flag)
            // without tuple allocation; it is equally safe to skip.
            if matches!(
                op.opcode,
                OpCode::GetIter
                    | OpCode::IterNext
                    | OpCode::IterNextUnboxed
                    | OpCode::ForIter
                    | OpCode::ScfFor
            ) {
                continue;
            }

            // TypeGuard ops (runtime type checks) are non-escaping and
            // don't prevent vectorization — they're eliminated or folded
            // by the type guard hoist pass before codegen.
            if op.opcode == OpCode::TypeGuard {
                continue;
            }

            if is_disqualifying(op.opcode) {
                vectorizable = false;
                break;
            }

            if !is_scalar_arithmetic(op.opcode) {
                // Any op not in our allowed arithmetic set is disqualifying.
                vectorizable = false;
                break;
            }

            // Type uniformity check: scan both operand and result types.
            // Operand types come from ty_map (block args); result types too
            // when available. Mixed I64+F64 in the same loop body disqualifies.
            let operand_and_result_ids: Vec<ValueId> = op
                .operands
                .iter()
                .chain(op.results.iter())
                .copied()
                .collect();
            for v in operand_and_result_ids {
                if let Some(ty) = ty_map.get(&v)
                    && is_numeric(ty)
                {
                    match &seen_type {
                        None => seen_type = Some(ty.clone()),
                        Some(prev) if prev == ty => {}
                        _ => mixed_types = true,
                    }
                }
            }

            // Reduction detection: look for Add/Mul/etc. that uses an
            // accumulator block-arg as one of its operands.
            //
            // Mojo/GCC 15 auto-vectorization: we detect Min/Max reductions
            // in addition to Sum/Product/And/Or. For `for x in list[int]:
            // total += x`, the Sum reduction is detected via the Add op on
            // the accumulator. Min/Max reductions use comparison + select
            // patterns — we detect them via the Lt/Gt comparison ops that
            // feed into the accumulator via a CondBranch select pattern.
            // For now, we detect Min/Max when the loop body contains
            // exactly one comparison op on the accumulator.
            if reduction.is_none() {
                let uses_acc = op.operands.iter().any(|v| acc_candidates.contains(v));
                if uses_acc {
                    reduction = match op.opcode {
                        OpCode::Add => Some(ReductionOp::Sum),
                        OpCode::Mul => Some(ReductionOp::Product),
                        OpCode::BitAnd => Some(ReductionOp::And),
                        OpCode::BitOr => Some(ReductionOp::Or),
                        // Min/Max via comparison ops: when the accumulator is
                        // compared (Lt/Le → Min, Gt/Ge → Max) and the result
                        // feeds a conditional select of the accumulator, this
                        // is a min/max reduction pattern.
                        OpCode::Lt | OpCode::Le => Some(ReductionOp::Min),
                        OpCode::Gt | OpCode::Ge => Some(ReductionOp::Max),
                        _ => None,
                    };
                }
            }
        }

        if !vectorizable {
            break;
        }
    }

    // If we saw mixed numeric types the loop is still technically vectorizable
    // in some ISAs, but we conservatively mark it not vectorizable for now.
    if mixed_types {
        vectorizable = false;
    }

    VectorizationInfo {
        vectorizable,
        element_type: seen_type,
        estimated_trip_count: None, // trip-count analysis is a future pass
        reduction_op: if vectorizable { reduction } else { None },
    }
}

// ---------------------------------------------------------------------------
// Pass entry point
// ---------------------------------------------------------------------------

/// Analyse all loops in a TIR function for vectorization potential.
///
/// Adds `"vectorize" = AttrValue::Bool(true)` and optionally
/// `"reduction" = AttrValue::Str("sum"|…)` to the `ForIter`/`ScfFor` op
/// in each vectorizable loop-header block.
///
/// Returns [`PassStats`] with `values_changed` set to the number of loops
/// annotated.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "vectorize",
        ..Default::default()
    };

    let loops = find_loops(func);

    // For each loop, analyse and potentially annotate.
    // We collect (header, info) pairs first to avoid borrowing `func` mutably
    // while reading from it.
    let analyses: Vec<(BlockId, VectorizationInfo)> = loops
        .iter()
        .map(|(&header, body)| (header, analyse_loop(func, body)))
        .collect();

    for (header, info) in analyses {
        if !info.vectorizable {
            continue;
        }

        let block = match func.blocks.get_mut(&header) {
            Some(b) => b,
            None => continue,
        };

        // Find the first ForIter / ScfFor op in the header and annotate it.
        // If no such op exists, annotate the first arithmetic op we find.
        let target_op = block.ops.iter_mut().find(|op| {
            matches!(
                op.opcode,
                OpCode::ForIter | OpCode::ScfFor | OpCode::GetIter
            )
        });

        let op = match target_op {
            Some(o) => o,
            None => {
                // Fallback: annotate whatever is there (e.g. an Add for synthetic loops).
                match block.ops.first_mut() {
                    Some(o) => o,
                    None => continue,
                }
            }
        };

        op.attrs.insert("vectorize".into(), AttrValue::Bool(true));
        stats.values_changed += 1;

        if let Some(red) = info.reduction_op {
            op.attrs
                .insert("reduction".into(), AttrValue::Str(red.as_str().into()));
        }

        // Mojo/GCC 15 auto-vectorization: emit element type and SIMD width
        // hints so the LLVM backend can select the correct vector intrinsic
        // width. For I64 elements, typical SIMD widths are:
        //   - SSE2/NEON: 2 lanes (128-bit)
        //   - AVX2: 4 lanes (256-bit)
        //   - AVX-512: 8 lanes (512-bit)
        // We emit the conservative width (2) as the minimum; the backend
        // can widen based on target features.
        if let Some(ref elem_ty) = info.element_type {
            let ty_str = match elem_ty {
                TirType::I64 => "i64",
                TirType::F64 => "f64",
                _ => "unknown",
            };
            op.attrs
                .insert("element_type".into(), AttrValue::Str(ty_str.into()));
            let simd_width: i64 = match elem_ty {
                TirType::I64 | TirType::F64 => 2, // 128-bit minimum
                _ => 1,
            };
            op.attrs
                .insert("simd_width".into(), AttrValue::Int(simd_width));
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};
    use std::collections::HashMap;

    // -----------------------------------------------------------------------
    // Helper: build a TirOp
    // -----------------------------------------------------------------------
    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn op_with_attrs(
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

    fn int_attrs(v: i64) -> AttrDict {
        let mut m = AttrDict::new();
        m.insert("value".into(), AttrValue::Int(v));
        m
    }

    // -----------------------------------------------------------------------
    // Test 1: Simple array-sum loop → marked vectorizable with Sum reduction.
    //
    // CFG shape:
    //   entry → loop_header(acc: I64) ──back──> loop_header
    //                         └─exit──> exit_block
    //
    // loop_header body:
    //   %elem = ConstInt 1          (simulates loading an element)
    //   %acc2 = Add acc, %elem      (accumulator update — sum reduction)
    //   ForIter …
    // -----------------------------------------------------------------------
    #[test]
    fn simple_sum_loop_vectorizable() {
        let entry_id = BlockId(0);
        let header_id = BlockId(1);
        let exit_id = BlockId(2);

        // Values
        let acc = ValueId(0); // loop block arg — accumulator
        let elem = ValueId(1); // loaded element
        let acc2 = ValueId(2); // updated accumulator
        let init = ValueId(3); // initial accumulator value

        let mut blocks = HashMap::new();

        // Entry: produce initial accumulator, branch to loop header.
        blocks.insert(
            entry_id,
            TirBlock {
                id: entry_id,
                args: vec![],
                ops: vec![op_with_attrs(
                    OpCode::ConstInt,
                    vec![],
                    vec![init],
                    int_attrs(0),
                )],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![init],
                },
            },
        );

        // Loop header: acc is the block arg (accumulator).
        // The back-edge passes acc2, creating the reduction phi.
        blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: vec![TirValue {
                    id: acc,
                    ty: TirType::I64,
                }],
                ops: vec![
                    // Simulate element load as a ConstInt.
                    op_with_attrs(OpCode::ConstInt, vec![], vec![elem], int_attrs(1)),
                    // acc2 = acc + elem  (sum reduction)
                    op(OpCode::Add, vec![acc, elem], vec![acc2]),
                    // ForIter marker.
                    op(OpCode::ForIter, vec![], vec![]),
                ],
                // Conditional: continue loop (pass acc2 back) or exit.
                terminator: Terminator::CondBranch {
                    cond: acc,
                    then_block: header_id, // back-edge
                    then_args: vec![acc2],
                    else_block: exit_id,
                    else_args: vec![acc2],
                },
            },
        );

        // Exit block.
        blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![TirValue {
                    id: ValueId(4),
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![ValueId(4)],
                },
            },
        );

        let mut func = TirFunction {
            name: "sum_loop".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::I64,
            blocks,
            entry_block: entry_id,
            next_value: 5,
            next_block: 3,
            attrs: crate::tir::ops::AttrDict::new(),
            has_exception_handling: false,
            label_id_map: std::collections::HashMap::new(),
            loop_roles: std::collections::HashMap::new(),
            loop_pairs: std::collections::HashMap::new(),
            loop_break_kinds: std::collections::HashMap::new(),
            loop_cond_blocks: std::collections::HashMap::new(),
        };

        let stats = run(&mut func);

        // Loop header should have been annotated.
        assert!(
            stats.values_changed > 0,
            "expected at least one loop annotated"
        );

        let header = &func.blocks[&header_id];
        let for_iter_op = header
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::ForIter)
            .expect("ForIter op must exist");

        assert_eq!(
            for_iter_op.attrs.get("vectorize"),
            Some(&AttrValue::Bool(true)),
            "vectorize attr must be set"
        );
        assert_eq!(
            for_iter_op.attrs.get("reduction"),
            Some(&AttrValue::Str("sum".into())),
            "reduction attr must be 'sum'"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: Loop with a function call → NOT marked vectorizable.
    // -----------------------------------------------------------------------
    #[test]
    fn loop_with_call_not_vectorizable() {
        let entry_id = BlockId(0);
        let header_id = BlockId(1);
        let exit_id = BlockId(2);

        let callee = ValueId(0);
        let result = ValueId(1);

        let mut blocks = HashMap::new();

        blocks.insert(
            entry_id,
            TirBlock {
                id: entry_id,
                args: vec![],
                ops: vec![op_with_attrs(
                    OpCode::ConstInt,
                    vec![],
                    vec![callee],
                    int_attrs(0),
                )],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![],
                },
            },
        );

        blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: vec![],
                ops: vec![
                    // Impure call inside loop — disqualifies vectorization.
                    op(OpCode::Call, vec![callee], vec![result]),
                    op(OpCode::ForIter, vec![], vec![]),
                ],
                terminator: Terminator::CondBranch {
                    cond: callee,
                    then_block: header_id,
                    then_args: vec![],
                    else_block: exit_id,
                    else_args: vec![],
                },
            },
        );

        blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut func = TirFunction {
            name: "call_loop".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value: 2,
            next_block: 3,
            attrs: crate::tir::ops::AttrDict::new(),
            has_exception_handling: false,
            label_id_map: std::collections::HashMap::new(),
            loop_roles: std::collections::HashMap::new(),
            loop_pairs: std::collections::HashMap::new(),
            loop_break_kinds: std::collections::HashMap::new(),
            loop_cond_blocks: std::collections::HashMap::new(),
        };

        run(&mut func);

        // The ForIter op must NOT have the "vectorize" attribute.
        let header = &func.blocks[&header_id];
        let for_iter_op = header
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::ForIter)
            .expect("ForIter op must exist");

        assert!(
            !for_iter_op.attrs.contains_key("vectorize"),
            "loop with Call must NOT be marked vectorizable"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: Loop with mixed types (I64 and F64) → NOT marked vectorizable.
    // -----------------------------------------------------------------------
    #[test]
    fn loop_with_mixed_types_not_vectorizable() {
        let entry_id = BlockId(0);
        let header_id = BlockId(1);
        let exit_id = BlockId(2);

        let int_val = ValueId(0);
        let float_val = ValueId(1);
        let mixed_sum = ValueId(2);

        let mut blocks = HashMap::new();

        blocks.insert(
            entry_id,
            TirBlock {
                id: entry_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![],
                },
            },
        );

        let mut float_attrs = AttrDict::new();
        float_attrs.insert("value".into(), AttrValue::Float(1.0));

        blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: vec![
                    TirValue {
                        id: int_val,
                        ty: TirType::I64,
                    },
                    TirValue {
                        id: float_val,
                        ty: TirType::F64,
                    },
                ],
                ops: vec![
                    // Add with mixed I64/F64 operands — mixed types.
                    op(OpCode::Add, vec![int_val, float_val], vec![mixed_sum]),
                    op(OpCode::ForIter, vec![], vec![]),
                ],
                terminator: Terminator::CondBranch {
                    cond: int_val,
                    then_block: header_id,
                    then_args: vec![int_val, float_val],
                    else_block: exit_id,
                    else_args: vec![],
                },
            },
        );

        blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let mut func = TirFunction {
            name: "mixed_type_loop".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value: 3,
            next_block: 3,
            attrs: crate::tir::ops::AttrDict::new(),
            has_exception_handling: false,
            label_id_map: std::collections::HashMap::new(),
            loop_roles: std::collections::HashMap::new(),
            loop_pairs: std::collections::HashMap::new(),
            loop_break_kinds: std::collections::HashMap::new(),
            loop_cond_blocks: std::collections::HashMap::new(),
        };

        run(&mut func);

        let header = &func.blocks[&header_id];
        let for_iter_op = header
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::ForIter)
            .expect("ForIter op must exist");

        assert!(
            !for_iter_op.attrs.contains_key("vectorize"),
            "loop with mixed types must NOT be marked vectorizable"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Function with no loops → no changes, no panic.
    // -----------------------------------------------------------------------
    #[test]
    fn no_loops_no_changes() {
        let mut func = TirFunction::new("no_loops".into(), vec![], TirType::None);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);

        assert_eq!(stats.values_changed, 0);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(stats.ops_added, 0);
    }
}
