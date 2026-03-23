//! TIR optimization passes.
//! Each pass transforms a TirFunction in-place and returns statistics.

pub mod bce;
pub mod cha;
pub mod dce;
pub mod interprocedural;
pub mod escape_analysis;
pub mod refcount_elim;
pub mod sccp;
pub mod strength_reduction;
pub mod type_guard_hoist;
pub mod unboxing;

/// Statistics returned by each optimization pass.
#[derive(Debug, Default, Clone)]
pub struct PassStats {
    pub name: &'static str,
    pub values_changed: usize,
    pub ops_removed: usize,
    pub ops_added: usize,
}

/// Run the full TIR optimization pipeline on a function.
///
/// Pass order is critical — each pass feeds into the next:
/// 1. Unboxing (needs types from type_refine)
/// 2. Escape analysis (benefits from unboxed info)
/// 3. SCCP (folds constants after unboxing reveals types)
/// 4. Strength reduction (after SCCP reveals constant operands)
/// 5. BCE (after SCCP/SR simplify loop bounds)
/// 6. DCE (cleans up dead code from all prior passes)
///
/// The TIR verifier runs after all passes to catch invariant violations.
pub fn run_pipeline(func: &mut super::function::TirFunction) -> Vec<PassStats> {
    let mut stats = Vec::with_capacity(8);

    stats.push(unboxing::run(func));
    stats.push(escape_analysis::run(func));
    stats.push(refcount_elim::run(func));
    stats.push(type_guard_hoist::run(func));
    stats.push(sccp::run(func));
    stats.push(strength_reduction::run(func));
    stats.push(bce::run(func));
    stats.push(dce::run(func));

    // Verify TIR invariants after all passes.
    if let Err(errors) = super::verify::verify_function(func) {
        panic!(
            "TIR verification failed after optimization pipeline ({} errors): {:?}",
            errors.len(),
            errors
        );
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
