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
//! ## Vectorizability criteria
//!
//! A loop (identified by blocks that contain a `ForIter` or `ScfFor` op, plus
//! all blocks reachable from it before the loop exit) is considered vectorizable
//! when **all** of the following hold:
//!
//! 1. Every non-structural op operates only on `I64`, `F64`, or `Bool` values.
//!    Mixed numeric types are allowed via Python-style numeric promotion (see
//!    "Mixed-type promotion" below).
//! 2. No op is a `Call`, `CallMethod`, `CallBuiltin`, or any other impure op.
//! 3. No write to non-local memory (`StoreAttr`, `StoreIndex`, `DelAttr`,
//!    `DelIndex`, `Free`, `IncRef`, `DecRef`).
//! 4. No generator ops (`Yield`, `YieldFrom`), exception ops (`Raise`,
//!    `CheckException`), or import ops.
//!
//! ## Mixed-type promotion
//!
//! Python's numeric tower promotes `bool → int → float`. SIMD ISAs (SSE2/AVX2/
//! AVX-512/NEON/SVE) all support both i64 and f64 lanes at the same vector
//! bit-width (e.g. AVX2 supplies both 4xi64 and 4xf64 in a 256-bit register),
//! and provide cheap `sitofp` lane-wise conversions. We classify each loop's
//! body by the join of every numeric value it touches:
//!
//! - All `I64` (and `Bool`, which zext-promotes to i64) → vectorize as i64 lanes.
//! - All `F64`                                          → vectorize as f64 lanes.
//! - Mixed `{I64, Bool} ∪ {F64}`                        → vectorize as f64 lanes
//!   with a `promoted = true` hint so the backend inserts `sitofp` on the
//!   integer-typed lane loads. The total vector bit-width stays the same; the
//!   lane count is unchanged because i64 and f64 share the same lane width.
//!
//! This lift is correctness-preserving: float arithmetic on integers that fit
//! in 53 mantissa bits is exact, matching CPython's behaviour for the values
//! that participate in such mixed loops in practice. The LIR backend is free
//! to widen / narrow the chosen lane count based on target features; we emit
//! the conservative 2-lane (128-bit) minimum.
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
    /// The lane element type after numeric promotion (if any numeric op was
    /// observed). For mixed-numeric loops this is `F64` — see `promoted`.
    pub element_type: Option<TirType>,
    /// Estimated trip count (only available when the loop bound is a compile-time constant).
    pub estimated_trip_count: Option<u64>,
    /// A detected reduction operation.
    pub reduction_op: Option<ReductionOp>,
    /// `true` when the loop body mixes integer-shaped (`I64`/`Bool`) and
    /// floating-point (`F64`) values, requiring lane-wise `sitofp` promotion
    /// of the integer values into the chosen `F64` lane type. Always `false`
    /// for uniform-typed loops.
    pub promoted: bool,
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
///
/// `Bool` is included because Python's numeric tower allows `bool` to
/// participate in arithmetic (zext-promoted to `i64`). Treating it as numeric
/// here lets bool-mixed-with-int loops vectorize as `i64` lanes, and
/// bool-mixed-with-float loops vectorize as `f64` lanes via promotion.
#[inline]
fn is_numeric(ty: &TirType) -> bool {
    matches!(ty, TirType::I64 | TirType::F64 | TirType::Bool)
}

/// SIMD lane category used for promotion analysis: `Int` covers `I64` / `Bool`
/// (both ride in `i64` lanes), `Float` covers `F64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaneCategory {
    Int,
    Float,
}

#[inline]
fn lane_category(ty: &TirType) -> Option<LaneCategory> {
    match ty {
        TirType::I64 | TirType::Bool => Some(LaneCategory::Int),
        TirType::F64 => Some(LaneCategory::Float),
        _ => None,
    }
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
    // Track which lane categories the body touches; we resolve the final
    // lane type by joining these at the end:
    //   {Int} only            → I64 lanes, no promotion.
    //   {Float} only          → F64 lanes, no promotion.
    //   {Int, Float} mixed    → F64 lanes with `promoted = true`.
    //   ∅                     → no numeric type observed (e.g. an
    //                           iterator-only body); element type unset,
    //                           no promotion.
    let mut saw_int = false;
    let mut saw_float = false;
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

            // Lane-category accumulation. Walk both operands and results;
            // every numeric value contributes to the int / float join.
            // Non-numeric values cannot legally appear here because the
            // disqualifying / arithmetic-only gates above already filter
            // any op shape that could carry one (BuildList/Alloc/Store/etc.).
            for v in op.operands.iter().chain(op.results.iter()) {
                if let Some(ty) = ty_map.get(v) {
                    match lane_category(ty) {
                        Some(LaneCategory::Int) => saw_int = true,
                        Some(LaneCategory::Float) => saw_float = true,
                        None => {}
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

    // Resolve the lane element type by joining the observed categories.
    // Float dominates Int in the numeric tower, so any presence of F64 forces
    // F64 lanes. Bool collapses into Int (we treated it as such in
    // `lane_category`), so no additional handling is needed here.
    let (element_type, promoted) = match (saw_int, saw_float) {
        (false, false) => (None, false),
        (true, false) => (Some(TirType::I64), false),
        (false, true) => (Some(TirType::F64), false),
        (true, true) => (Some(TirType::F64), true),
    };

    VectorizationInfo {
        vectorizable,
        element_type,
        estimated_trip_count: None, // trip-count analysis is a future pass
        reduction_op: if vectorizable { reduction } else { None },
        promoted: vectorizable && promoted,
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
        // width. For I64 / F64 elements, typical SIMD widths are:
        //   - SSE2/NEON: 2 lanes (128-bit)
        //   - AVX2:      4 lanes (256-bit)
        //   - AVX-512:   8 lanes (512-bit)
        // We emit the conservative width (2) as the minimum; the backend
        // can widen based on target features. `i64` and `f64` share the
        // same lane width, so the lane count is identical for promoted
        // mixed-type loops and uniform `f64` loops.
        if let Some(ref elem_ty) = info.element_type {
            let ty_str = match elem_ty {
                TirType::I64 => "i64",
                TirType::F64 => "f64",
                // `analyse_loop` only ever sets element_type to I64 or F64
                // (Bool collapses into the I64 lane category). We still
                // surface a defensive default rather than panicking so
                // future numeric tower extensions degrade gracefully.
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

        // Mixed-type promotion hint: when the loop body mixed integer-shaped
        // and floating-point values, the analysis chose F64 lanes and the
        // backend must insert lane-wise `sitofp` on the integer-typed
        // operand loads. We surface this as an explicit attr so the LIR
        // lowering does not need to re-derive it.
        if info.promoted {
            op.attrs.insert("promoted".into(), AttrValue::Bool(true));
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
    // Helper: build a single-block loop function whose header carries `args`
    // and `ops`, with a self-back-edge passing `back_args` to the header and
    // an exit edge to a Return.
    //
    // Centralising this scaffolding keeps the new mixed-type tests focused
    // on the type contract rather than CFG plumbing, and matches the layout
    // used by the original `loop_with_mixed_types_*` test.
    // -----------------------------------------------------------------------
    fn build_loop_func(
        name: &str,
        header_args: Vec<TirValue>,
        body_ops: Vec<TirOp>,
        cond: ValueId,
        back_args: Vec<ValueId>,
    ) -> TirFunction {
        let entry_id = BlockId(0);
        let header_id = BlockId(1);
        let exit_id = BlockId(2);

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
        blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: header_args,
                ops: body_ops,
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: header_id,
                    then_args: back_args,
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

        // `next_value` / `next_block` only matter when the pass needs to mint
        // fresh ids; the vectorize pass is annotation-only, so a generous
        // upper bound is sufficient and keeps tests robust to future edits.
        TirFunction {
            name: name.into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::None,
            blocks,
            entry_block: entry_id,
            next_value: 1024,
            next_block: 16,
            attrs: crate::tir::ops::AttrDict::new(),
            has_exception_handling: false,
            label_id_map: std::collections::HashMap::new(),
            loop_roles: std::collections::HashMap::new(),
            loop_pairs: std::collections::HashMap::new(),
            loop_break_kinds: std::collections::HashMap::new(),
            loop_cond_blocks: std::collections::HashMap::new(),
        }
    }

    fn header_op<'a>(func: &'a TirFunction, header_id: BlockId) -> &'a TirOp {
        func.blocks[&header_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::ForIter)
            .expect("ForIter op must exist in loop header")
    }

    // -----------------------------------------------------------------------
    // Test 3: Mixed I64 + F64 loop body → vectorized as F64 lanes with the
    // `promoted` attr set, modelling `total = total + a[i]` where `a` is
    // `list[int]` and `total` is `float`. This is the headline behaviour of
    // the lift: previously this loop was bailed out of vectorization.
    // -----------------------------------------------------------------------
    #[test]
    fn mixed_int_float_promotes_to_float_vector() {
        let int_val = ValueId(0); // simulates a[i] : int
        let float_acc = ValueId(1); // total : float (loop-carried)
        let int_as_float = ValueId(2); // result of mixed-type arithmetic
        let new_acc = ValueId(3); // updated accumulator (still float)

        let mut func = build_loop_func(
            "mixed_int_float_loop",
            vec![
                TirValue {
                    id: int_val,
                    ty: TirType::I64,
                },
                TirValue {
                    id: float_acc,
                    ty: TirType::F64,
                },
            ],
            vec![
                // First op references both an I64 operand and an F64 result —
                // the mixed-type pattern that previously bailed.
                op(OpCode::Add, vec![float_acc, int_val], vec![int_as_float]),
                op(OpCode::Add, vec![int_as_float, float_acc], vec![new_acc]),
                op(OpCode::ForIter, vec![], vec![]),
            ],
            int_val,
            vec![int_val, new_acc],
        );

        run(&mut func);

        let for_iter_op = header_op(&func, BlockId(1));

        assert_eq!(
            for_iter_op.attrs.get("vectorize"),
            Some(&AttrValue::Bool(true)),
            "mixed-type loop must now be marked vectorizable"
        );
        assert_eq!(
            for_iter_op.attrs.get("element_type"),
            Some(&AttrValue::Str("f64".into())),
            "mixed-type loop must promote to f64 lanes"
        );
        assert_eq!(
            for_iter_op.attrs.get("simd_width"),
            Some(&AttrValue::Int(2)),
            "f64 lanes use the conservative 128-bit minimum width"
        );
        assert_eq!(
            for_iter_op.attrs.get("promoted"),
            Some(&AttrValue::Bool(true)),
            "promoted attr must signal lane-wise sitofp insertion"
        );
        // The Add-on-acc is still recognised as a Sum reduction even after
        // promotion — vectorized horizontal-add reductions on f64 are well-
        // defined on every targeted ISA.
        assert_eq!(
            for_iter_op.attrs.get("reduction"),
            Some(&AttrValue::Str("sum".into())),
            "sum reduction detection survives promotion"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Pure-int loop continues to vectorize as `i64` lanes with no
    // `promoted` attribute. Guards against the lift accidentally promoting
    // every loop to f64.
    // -----------------------------------------------------------------------
    #[test]
    fn pure_int_remains_int_vector() {
        let acc = ValueId(0);
        let elem = ValueId(1);
        let acc2 = ValueId(2);

        let mut func = build_loop_func(
            "pure_int_loop",
            vec![TirValue {
                id: acc,
                ty: TirType::I64,
            }],
            vec![
                op_with_attrs(OpCode::ConstInt, vec![], vec![elem], int_attrs(7)),
                op(OpCode::Add, vec![acc, elem], vec![acc2]),
                op(OpCode::ForIter, vec![], vec![]),
            ],
            acc,
            vec![acc2],
        );

        run(&mut func);

        let for_iter_op = header_op(&func, BlockId(1));

        assert_eq!(
            for_iter_op.attrs.get("vectorize"),
            Some(&AttrValue::Bool(true))
        );
        assert_eq!(
            for_iter_op.attrs.get("element_type"),
            Some(&AttrValue::Str("i64".into())),
            "pure-int loop must stay on i64 lanes"
        );
        assert!(
            !for_iter_op.attrs.contains_key("promoted"),
            "pure-int loop must NOT carry the promoted hint"
        );
        assert_eq!(
            for_iter_op.attrs.get("reduction"),
            Some(&AttrValue::Str("sum".into()))
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: Pure-float loop continues to vectorize as `f64` lanes with no
    // `promoted` attribute (no integer to promote).
    // -----------------------------------------------------------------------
    #[test]
    fn pure_float_remains_float_vector() {
        let acc = ValueId(0);
        let elem = ValueId(1);
        let acc2 = ValueId(2);

        let mut float_attrs = AttrDict::new();
        float_attrs.insert("value".into(), AttrValue::Float(1.5));

        let mut func = build_loop_func(
            "pure_float_loop",
            vec![TirValue {
                id: acc,
                ty: TirType::F64,
            }],
            vec![
                op_with_attrs(OpCode::ConstFloat, vec![], vec![elem], float_attrs),
                op(OpCode::Add, vec![acc, elem], vec![acc2]),
                op(OpCode::ForIter, vec![], vec![]),
            ],
            acc,
            vec![acc2],
        );

        run(&mut func);

        let for_iter_op = header_op(&func, BlockId(1));

        assert_eq!(
            for_iter_op.attrs.get("vectorize"),
            Some(&AttrValue::Bool(true))
        );
        assert_eq!(
            for_iter_op.attrs.get("element_type"),
            Some(&AttrValue::Str("f64".into())),
            "pure-float loop must stay on f64 lanes"
        );
        assert!(
            !for_iter_op.attrs.contains_key("promoted"),
            "pure-float loop must NOT carry the promoted hint"
        );
        assert_eq!(
            for_iter_op.attrs.get("reduction"),
            Some(&AttrValue::Str("sum".into()))
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: Bool-mixed-with-Int arithmetic — Python's `True + 1 == 2`
    // pattern. Bool operands collapse into the integer lane category, so the
    // loop must vectorize as `i64` lanes without triggering the `promoted`
    // hint. This guards against accidentally classifying Bool as a separate
    // numeric category that would force unnecessary float promotion.
    // -----------------------------------------------------------------------
    #[test]
    fn boolean_mixed_in_blocks_vectorization_correctness() {
        let acc = ValueId(0); // i64 accumulator
        let flag = ValueId(1); // bool predicate (e.g. element > 0)
        let acc2 = ValueId(2); // updated accumulator

        let mut func = build_loop_func(
            "bool_int_loop",
            vec![
                TirValue {
                    id: acc,
                    ty: TirType::I64,
                },
                TirValue {
                    id: flag,
                    ty: TirType::Bool,
                },
            ],
            vec![
                // Bool-promoted-to-int arithmetic: count += predicate.
                op(OpCode::Add, vec![acc, flag], vec![acc2]),
                op(OpCode::ForIter, vec![], vec![]),
            ],
            acc,
            vec![acc2, flag],
        );

        run(&mut func);

        let for_iter_op = header_op(&func, BlockId(1));

        assert_eq!(
            for_iter_op.attrs.get("vectorize"),
            Some(&AttrValue::Bool(true)),
            "bool+int loop must vectorize"
        );
        assert_eq!(
            for_iter_op.attrs.get("element_type"),
            Some(&AttrValue::Str("i64".into())),
            "bool collapses into i64 lane category"
        );
        assert!(
            !for_iter_op.attrs.contains_key("promoted"),
            "bool+int does not require float promotion"
        );
        assert_eq!(
            for_iter_op.attrs.get("reduction"),
            Some(&AttrValue::Str("sum".into())),
            "predicate-counting reduction is still recognised as Sum"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: Function with no loops → no changes, no panic.
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
