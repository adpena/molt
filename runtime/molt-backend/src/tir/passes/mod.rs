//! TIR optimization passes.
//! Each pass transforms a TirFunction in-place and returns statistics.

pub mod bce;
pub mod block_versioning;
pub mod branchless_count;
pub mod canonicalize;
pub mod cha;
pub mod closure_spec;
pub mod copy_prop;
pub mod dce;
pub mod deforestation;
pub mod effects;
pub mod escape_analysis;
pub mod ownership;
pub mod fast_math;
pub mod interprocedural;
pub mod iter_devirt;
pub mod loop_narrow;
pub mod monomorphize;
pub mod polyhedral;
pub mod range_devirt;
mod reachability;
pub mod refcount_elim;
pub mod reuse_analysis;
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
/// If the optimized function violates TIR invariants, this is a compiler bug
/// and the pipeline panics immediately. Zero-delta pipelines still return
/// per-pass stats; they simply restore the original snapshot before lowering.
pub fn run_pipeline(func: &mut super::function::TirFunction) -> Vec<PassStats> {
    // Snapshot the function BEFORE any mutation so unchanged pipelines can
    // lower the original IR structurally without pass-induced metadata drift.
    let snapshot = func.clone();

    let mut stats = Vec::with_capacity(10);

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
        let sanitized: String = func
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let mut dump = format!(
            "// PRE-OPT TIR: {} (loop_roles={:?})\n",
            func.name, func.loop_roles
        );
        dump.push_str(&format!("// blocks: {}\n", func.blocks.len()));
        let mut bids: Vec<_> = func.blocks.keys().copied().collect();
        bids.sort_by_key(|b| b.0);
        for bid in &bids {
            let block = &func.blocks[bid];
            dump.push_str(&format!(
                "\nblock {} (args={}, ops={}):\n",
                bid.0,
                block.args.len(),
                block.ops.len()
            ));
            for op in &block.ops {
                dump.push_str(&format!(
                    "  {:?} operands={:?} results={:?}\n",
                    op.opcode, op.operands, op.results
                ));
            }
            dump.push_str(&format!(
                "  TERM: {:?}\n",
                std::mem::discriminant(&block.terminator)
            ));
            match &block.terminator {
                super::blocks::Terminator::Branch { target, args } => {
                    dump.push_str(&format!("    → block {} (args={})\n", target.0, args.len()))
                }
                super::blocks::Terminator::CondBranch {
                    cond,
                    then_block,
                    else_block,
                    ..
                } => dump.push_str(&format!(
                    "    cond={:?} then={} else={}\n",
                    cond, then_block.0, else_block.0
                )),
                super::blocks::Terminator::Return { values } => {
                    dump.push_str(&format!("    return {} values\n", values.len()))
                }
                _ => {}
            }
        }
        let _ = crate::debug_artifacts::write_debug_artifact(
            format!("tir/{}_pre.txt", sanitized),
            dump,
        );
    }

    macro_rules! run_pass {
        ($name:expr, $pass:expr) => {
            if !skip_set.contains($name) {
                stats.push($pass);
            }
        };
    }

    // ── Lowering passes ──────────────────────────────────────────
    // Devirtualize iterators and ranges into concrete loops.
    run_pass!("range_devirt", range_devirt::run(func));
    run_pass!("iter_devirt", iter_devirt::run(func));
    run_pass!(
        "tuple_scalarize",
        deforestation::run_tuple_scalarize(func)
    );
    run_pass!("loop_narrow", loop_narrow::run(func));

    // ── Canonicalization (phase 1) ───────────────────────────────
    // Normalize all ops to canonical form BEFORE type-directed passes.
    // Following MLIR/LLVM instcombine philosophy: canonicalize runs
    // multiple times — once before and once after unboxing reveals types.
    run_pass!("canonicalize", canonicalize::run(func));

    // ── Type-directed optimization ─────────────────────────────
    run_pass!("unboxing", unboxing::run(func));
    run_pass!("block_versioning", block_versioning::run(func));

    // ── Canonicalization (phase 2) ───────────────────────────────
    // Re-canonicalize after unboxing: unboxed operations may reveal
    // new identity/absorbing patterns (e.g., unboxed int x + 0).
    run_pass!("canonicalize_post", canonicalize::run(func));

    // ── Memory optimization ────────────────────────────────────
    run_pass!("escape_analysis", escape_analysis::run(func));
    run_pass!("refcount_elim", refcount_elim::run(func));
    run_pass!("reuse_analysis", reuse_analysis::run(func));

    // ── Value optimization ─────────────────────────────────────
    run_pass!("type_guard_hoist", type_guard_hoist::run(func));
    run_pass!("sccp", sccp::run(func));
    run_pass!("strength_reduction", strength_reduction::run(func));
    // Fast math: reassociate and simplify floating-point expressions
    // when the user has opted in via annotations or global flags.
    run_pass!("fast_math", fast_math::run(func));
    run_pass!("branchless_count", branchless_count::run(func));
    run_pass!("bce", bce::run(func));
    // Auto-vectorization: detect and vectorize parallel loop bodies
    // using SIMD operations (SSE/AVX on x86, NEON on ARM).
    run_pass!("vectorize", vectorize::run(func));
    // Polyhedral optimization: loop tiling, interchange, and fusion
    // for nested loop nests with affine bounds.
    run_pass!("polyhedral", polyhedral::run(func));

    // ── Cleanup ────────────────────────────────────────────────
    // Copy propagation resolves chains introduced by prior passes.
    // DCE removes everything left dead.
    run_pass!("copy_prop", copy_prop::run(func));
    run_pass!("dce", dce::run(func));

    // If no pass changed anything, restore the snapshot to avoid any
    // incidental TIR structure mutation from pass traversals. The pipeline
    // still returns stats and downstream lowering still roundtrips the
    // restored TIR instead of silently bypassing TIR.
    let total_changes: usize = stats
        .iter()
        .map(|s| s.values_changed + s.ops_removed + s.ops_added)
        .sum();
    if total_changes == 0 {
        *func = snapshot.clone();
    }

    // Dump post-optimization TIR for loop-bearing functions.
    if has_loop_role && dump_tir {
        let sanitized: String = func
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let mut dump = format!(
            "// POST-OPT TIR: {} (loop_roles={:?})\n",
            func.name, func.loop_roles
        );
        dump.push_str(&format!(
            "// stats: {:?}\n",
            stats
                .iter()
                .map(|s| (s.name, s.values_changed, s.ops_removed, s.ops_added))
                .collect::<Vec<_>>()
        ));
        let mut bids: Vec<_> = func.blocks.keys().copied().collect();
        bids.sort_by_key(|b| b.0);
        for bid in &bids {
            let block = &func.blocks[bid];
            dump.push_str(&format!(
                "\nblock {} (args={}, ops={}):\n",
                bid.0,
                block.args.len(),
                block.ops.len()
            ));
            for op in &block.ops {
                dump.push_str(&format!(
                    "  {:?} operands={:?} results={:?}\n",
                    op.opcode, op.operands, op.results
                ));
            }
            match &block.terminator {
                super::blocks::Terminator::Branch { target, args } => dump.push_str(&format!(
                    "  TERM: Branch → block {} (args={})\n",
                    target.0,
                    args.len()
                )),
                super::blocks::Terminator::CondBranch {
                    cond,
                    then_block,
                    else_block,
                    ..
                } => dump.push_str(&format!(
                    "  TERM: CondBranch cond={:?} then={} else={}\n",
                    cond, then_block.0, else_block.0
                )),
                super::blocks::Terminator::Return { values } => {
                    dump.push_str(&format!("  TERM: Return {} values\n", values.len()))
                }
                _ => dump.push_str("  TERM: other\n"),
            }
        }
        let _ = crate::debug_artifacts::write_debug_artifact(
            format!("tir/{}_post.txt", sanitized),
            dump,
        );
    }

    if let Err(errors) = super::verify::verify_function(func) {
        panic!(
            "[TIR] verification failed after optimization of '{}': {:?}",
            func.name, errors
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
