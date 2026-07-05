use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{Analysis, LoopForest, LoopForestResult};
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{ScevExpr, TripCount};
use crate::tir::values::ValueId;

use super::builder::ScevBuilder;
use super::index::build_def_index;
use super::result::ScevResult;
use super::trip_count::compute_trip_count;

/// Public entry: compute the scalar-evolution facts for `func`. Shared by the
/// [`ScalarEvolution`] analysis and the value-range analysis (single source of
/// truth — no duplicate recurrence recognizer).
pub fn compute_scev(func: &TirFunction) -> ScevResult {
    let loops = <LoopForest as Analysis>::compute(func);
    compute_scev_with_loop_forest(func, &loops)
}

/// Compute scalar-evolution facts against the canonical LoopForest supplied by
/// a caller that already owns loop-shape discovery.
pub(crate) fn compute_scev_with_loop_forest(
    func: &TirFunction,
    loops: &LoopForestResult,
) -> ScevResult {
    if loops.headers.is_empty() {
        // No loops → no recurrences and no trip counts. Still classify
        // constants/invariants so value-range can use them, but that is cheap
        // and done lazily by value-range itself; here we return empty.
        return ScevResult::default();
    }
    let loop_headers: HashSet<BlockId> = loops.headers.iter().copied().collect();
    let defs = build_def_index(func, &loop_headers);
    let mut builder = ScevBuilder::new(func, loops, &defs);

    // Compute SCEV for the IV header-arg of each loop (the primary recurrences),
    // plus the back-edge increments and any affine derivations the consumer may
    // query. We classify every value reachable as a header arg or op result.
    let mut exprs: HashMap<ValueId, ScevExpr> = HashMap::new();

    // Header args first (recognizes the IVs and populates iv_of_header).
    for &header in &loops.headers {
        if let Some(args) = defs.header_args.get(&header) {
            for &a in args {
                let e = builder.scev(a);
                if !matches!(e, ScevExpr::Unknown) {
                    exprs.insert(a, e);
                }
            }
        }
    }

    // Then every op-defined value, so derived IVs (e.g. `j = i + c`) and
    // affine index expressions get a recurrence too.
    for block in func.blocks.values() {
        for op in &block.ops {
            for &r in &op.results {
                if exprs.contains_key(&r) {
                    continue;
                }
                let e = builder.scev(r);
                if !matches!(e, ScevExpr::Unknown) {
                    exprs.insert(r, e);
                }
            }
        }
    }

    // Trip counts: for a loop whose header tests `Lt(iv, stop)` (positive unit
    // step) or `Gt(iv, stop)` (negative unit step), the trip count is derivable
    // from start, step and stop. We compute a constant trip count when start,
    // step and stop are all constants; otherwise a symbolic bound when sound.
    let mut trip_counts: HashMap<BlockId, TripCount> = HashMap::new();
    let iv_of_header = builder.iv_of_header.clone();
    for &header in &loops.headers {
        let tc = compute_trip_count(func, &defs, &iv_of_header, &mut builder, header);
        trip_counts.insert(header, tc);
    }

    ScevResult {
        exprs,
        trip_counts,
        headers: loops.headers.clone(),
    }
}
