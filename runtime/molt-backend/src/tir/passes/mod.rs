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

    // Dump pre-optimization TIR for functions that contain loops.
    // This captures the exact IR that triggers the pass interaction bug.
    let has_loop_role = !func.loop_roles.is_empty();
    // Dump TIR for loop-bearing functions when MOLT_DUMP_IR or TIR_DUMP is set.
    let dump_tir = std::env::var("MOLT_DUMP_IR").is_ok() || std::env::var("TIR_DUMP").is_ok();
    if has_loop_role && dump_tir {
        let _ = std::fs::create_dir_all("/tmp/molt_tir");
        let sanitized: String = func.name.chars()
            .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
            .collect();
        let mut dump = format!("// PRE-OPT TIR: {} (loop_roles={:?})\n", func.name, func.loop_roles);
        dump.push_str(&format!("// blocks: {}\n", func.blocks.len()));
        let mut bids: Vec<_> = func.blocks.keys().copied().collect();
        bids.sort_by_key(|b| b.0);
        for bid in &bids {
            let block = &func.blocks[bid];
            dump.push_str(&format!("\nblock {} (args={}, ops={}):\n", bid.0, block.args.len(), block.ops.len()));
            for op in &block.ops {
                dump.push_str(&format!("  {:?} operands={:?} results={:?}\n", op.opcode, op.operands, op.results));
            }
            dump.push_str(&format!("  TERM: {:?}\n", std::mem::discriminant(&block.terminator)));
            match &block.terminator {
                super::blocks::Terminator::Branch { target, args } =>
                    dump.push_str(&format!("    → block {} (args={})\n", target.0, args.len())),
                super::blocks::Terminator::CondBranch { cond, then_block, else_block, .. } =>
                    dump.push_str(&format!("    cond={:?} then={} else={}\n", cond, then_block.0, else_block.0)),
                super::blocks::Terminator::Return { values } =>
                    dump.push_str(&format!("    return {} values\n", values.len())),
                _ => {}
            }
        }
        let path = format!("/tmp/molt_tir/{}_pre.txt", sanitized);
        let _ = std::fs::write(&path, &dump);
    }

    macro_rules! run_pass {
        ($name:expr, $pass:expr) => {
            if !skip_set.contains($name) {
                stats.push($pass);
            }
        };
    }

    run_pass!("unboxing", unboxing::run(func));
    run_pass!("escape_analysis", escape_analysis::run(func));
    run_pass!("refcount_elim", refcount_elim::run(func));
    run_pass!("type_guard_hoist", type_guard_hoist::run(func));
    run_pass!("sccp", sccp::run(func));
    run_pass!("strength_reduction", strength_reduction::run(func));
    run_pass!("bce", bce::run(func));
    run_pass!("dce", dce::run(func));

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
        let _ = std::fs::write(
            "/tmp/molt_zero_delta.txt",
            format!("zero_delta restore for func={}\n", func.name),
        );
        *func = snapshot;
        return stats;
    }

    // Dump post-optimization TIR for loop-bearing functions.
    if has_loop_role && dump_tir {
        let sanitized: String = func.name.chars()
            .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
            .collect();
        let mut dump = format!("// POST-OPT TIR: {} (loop_roles={:?})\n", func.name, func.loop_roles);
        dump.push_str(&format!("// stats: {:?}\n", stats.iter().map(|s| (s.name, s.values_changed, s.ops_removed, s.ops_added)).collect::<Vec<_>>()));
        let mut bids: Vec<_> = func.blocks.keys().copied().collect();
        bids.sort_by_key(|b| b.0);
        for bid in &bids {
            let block = &func.blocks[bid];
            dump.push_str(&format!("\nblock {} (args={}, ops={}):\n", bid.0, block.args.len(), block.ops.len()));
            for op in &block.ops {
                dump.push_str(&format!("  {:?} operands={:?} results={:?}\n", op.opcode, op.operands, op.results));
            }
            match &block.terminator {
                super::blocks::Terminator::Branch { target, args } =>
                    dump.push_str(&format!("  TERM: Branch → block {} (args={})\n", target.0, args.len())),
                super::blocks::Terminator::CondBranch { cond, then_block, else_block, .. } =>
                    dump.push_str(&format!("  TERM: CondBranch cond={:?} then={} else={}\n", cond, then_block.0, else_block.0)),
                super::blocks::Terminator::Return { values } =>
                    dump.push_str(&format!("  TERM: Return {} values\n", values.len())),
                _ => dump.push_str("  TERM: other\n"),
            }
        }
        let path = format!("/tmp/molt_tir/{}_post.txt", sanitized);
        let _ = std::fs::write(&path, &dump);
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
