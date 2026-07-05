use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    VectorReductionRule, VectorizeBodyAction, opcode_vectorize_facts_table,
};
use crate::tir::ops::TirOp;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

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
    pub(super) fn as_str(self) -> &'static str {
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

#[inline]
fn reduction_op_for_rule(rule: VectorReductionRule) -> Option<ReductionOp> {
    match rule {
        VectorReductionRule::None => None,
        VectorReductionRule::Sum => Some(ReductionOp::Sum),
        VectorReductionRule::Product => Some(ReductionOp::Product),
        VectorReductionRule::And => Some(ReductionOp::And),
        VectorReductionRule::Or => Some(ReductionOp::Or),
        VectorReductionRule::Min => Some(ReductionOp::Min),
        VectorReductionRule::Max => Some(ReductionOp::Max),
    }
}

// ---------------------------------------------------------------------------
// Helpers — op classification
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VectorizeBodyDecision {
    Analyze,
    Skip,
    Reject,
}

/// Translate generated opcode facts into the pass-local action.
/// The generated table owns opcode membership. This helper owns the only live
/// refinement: a Copy op is analyzable only when its attrs prove it is a plain
/// value copy rather than a lifetime/ownership operation.
#[inline]
fn vectorize_body_decision(op: &TirOp, action: VectorizeBodyAction) -> VectorizeBodyDecision {
    match action {
        VectorizeBodyAction::ScalarArithmetic => VectorizeBodyDecision::Analyze,
        VectorizeBodyAction::CopyIfPlain if op.is_plain_value_copy() => {
            VectorizeBodyDecision::Analyze
        }
        VectorizeBodyAction::IterationControl | VectorizeBodyAction::NonEscapingGuard => {
            VectorizeBodyDecision::Skip
        }
        VectorizeBodyAction::Reject | VectorizeBodyAction::CopyIfPlain => {
            VectorizeBodyDecision::Reject
        }
    }
}

/// SIMD lane category used for promotion analysis: `Int` covers `I64` / `Bool`
/// (both ride in `i64` lanes), `Float` covers `F64`.
/// `Bool` is included in the numeric tower because Python promotes
/// `bool → int → float`; zext-promoting `bool` to `i64` lets bool-mixed-with-int
/// loops vectorize as `i64` lanes, while bool-mixed-with-float loops vectorize
/// as `f64` lanes via the same `sitofp` promotion as integers.
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
pub(super) fn analyse_loop(func: &TirFunction, body: &HashSet<BlockId>) -> VectorizationInfo {
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
            let opcode_facts = opcode_vectorize_facts_table(op.opcode);
            match vectorize_body_decision(op, opcode_facts.body_action) {
                VectorizeBodyDecision::Skip => continue,
                VectorizeBodyDecision::Analyze => {}
                VectorizeBodyDecision::Reject => {
                    vectorizable = false;
                    break;
                }
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
                    // Min/Max via comparison ops: when the accumulator is
                    // compared and the result feeds a conditional select of the
                    // accumulator, this is a min/max reduction pattern. The
                    // opcode-to-family mapping is generated from op_kinds.toml.
                    reduction = reduction_op_for_rule(opcode_facts.reduction_rule);
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
