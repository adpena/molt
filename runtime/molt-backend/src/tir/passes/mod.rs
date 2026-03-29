//! TIR optimization passes.
//! Each pass transforms a TirFunction in-place and returns statistics.

pub mod bce;
pub mod cha;
pub mod closure_spec;
pub mod dce;
pub mod deforestation;
pub mod escape_analysis;
pub mod fast_math;
pub mod interprocedural;
pub mod monomorphize;
pub mod polyhedral;
pub mod refcount_elim;
pub mod sccp;
pub mod strength_reduction;
pub mod type_guard_hoist;
pub mod unboxing;
pub mod vectorize;

/// Statistics returned by each optimization pass.
#[derive(Debug, Default, Clone)]
pub struct PassStats {
    pub name: &'static str,
    pub values_changed: usize,
    pub ops_removed: usize,
    pub ops_added: usize,
}

/// Sentinel value: when `run_pipeline` returns exactly this many stats
/// with all zeroes, it means verification failed and the caller should
/// fall back to unoptimized code.
pub const VERIFICATION_FAILED_SENTINEL: usize = 0;

/// Run the full TIR optimization pipeline on a function.
///
/// Pass order is critical -- each pass feeds into the next:
/// 1. Unboxing (needs types from type_refine)
/// 2. Escape analysis (benefits from unboxed info)
/// 3. Refcount elimination (uses escape analysis results)
/// 4. Type guard hoisting (moves checks up in CFG)
/// 5. SCCP (folds constants after unboxing reveals types)
/// 6. Strength reduction (after SCCP reveals constant operands)
/// 7. BCE (after SCCP/SR simplify loop bounds)
/// 8. DCE (cleans up dead code from all prior passes)
///
/// If post-pipeline verification fails, restores the pre-optimization
/// snapshot and returns an empty Vec to signal the caller should use
/// unoptimized code.  This avoids panicking (fatal under panic=abort).
pub fn run_pipeline(func: &mut super::function::TirFunction) -> Vec<PassStats> {
    // Snapshot the function BEFORE any mutation so we can restore on
    // verification failure.  Without this, a corrupting pass would leave
    // `func` in an invalid state with no recovery path.
    let snapshot = func.clone();

    let mut stats = Vec::with_capacity(8);

    // Each pass can be individually disabled for debugging:
    //   MOLT_TIR_SKIP=unboxing,sccp,dce (comma-separated pass names)
    let skip = std::env::var("MOLT_TIR_SKIP").unwrap_or_default();
    let skip_set: std::collections::HashSet<&str> = skip.split(',').collect();

    if !skip_set.contains("unboxing") {
        stats.push(unboxing::run(func));
    }
    if !skip_set.contains("escape_analysis") {
        stats.push(escape_analysis::run(func));
    }
    if !skip_set.contains("refcount_elim") {
        stats.push(refcount_elim::run(func));
    }
    if !skip_set.contains("type_guard_hoist") {
        stats.push(type_guard_hoist::run(func));
    }
    if !skip_set.contains("sccp") {
        stats.push(sccp::run(func));
    }
    if !skip_set.contains("strength_reduction") {
        stats.push(strength_reduction::run(func));
    }
    if !skip_set.contains("bce") {
        stats.push(bce::run(func));
    }
    if !skip_set.contains("dce") {
        stats.push(dce::run(func));
    }

    // If no pass changed anything, restore the snapshot to avoid any
    // incidental TIR structure mutation from pass traversals.  The
    // lower_to_simple_ir roundtrip on an unmodified snapshot is known-good;
    // passes that report zero changes should be identity transforms but
    // in practice may reorder blocks or invalidate metadata.
    let total_changes: usize = stats
        .iter()
        .map(|s| s.values_changed + s.ops_removed + s.ops_added)
        .sum();
    if total_changes == 0 {
        *func = snapshot;
        return stats;
    }

    // Verify TIR invariants after all passes.  On failure, restore the
    // pre-optimization snapshot so callers get valid (unoptimized) IR
    // instead of corrupted output.
    if let Err(errors) = super::verify::verify_function(func) {
        eprintln!(
            "[TIR] WARNING: verification found {} error(s) after optimization — \
             restoring pre-optimization snapshot: {:?}",
            errors.len(),
            errors
        );
        *func = snapshot;
        return Vec::new(); // signal: verification failed
    }

    // Print stats if TIR_OPT_STATS=1
    if std::env::var("TIR_OPT_STATS").as_deref() == Ok("1") {
        for s in &stats {
            eprintln!(
                "[TIR] {}: {} values changed, {} ops removed, {} ops added",
                s.name, s.values_changed, s.ops_removed, s.ops_added
            );
        }
    }

    stats
}
