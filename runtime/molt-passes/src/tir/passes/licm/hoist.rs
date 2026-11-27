use molt_ir::deterministic_hash::{HashMap, HashSet};

use super::super::value_range::ValueRange;
use super::PassStats;
use super::safety::is_hoistable;
use crate::tir::analysis::{AnalysisManager, DefMap, ImmediateDoms, LoopForest, PredMap};
use crate::tir::blocks::BlockId;
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;
/// Find the preheader block for a loop header.
/// The preheader is the unique executable predecessor outside the loop and
/// must dominate the header. Retained lexical latches and exception edges are
/// governed by the same canonical CFG relation as loop discovery.
fn find_preheader(
    header_bid: BlockId,
    loop_blocks: &HashSet<BlockId>,
    predecessors: &HashMap<BlockId, Vec<BlockId>>,
    tree: &dominators::DominatorTree,
) -> Option<BlockId> {
    let mut preds = predecessors
        .get(&header_bid)?
        .iter()
        .copied()
        .filter(|bid| tree.contains(*bid) && !loop_blocks.contains(bid));
    let preheader = preds.next()?;
    (preds.next().is_none() && tree.dominates(preheader, header_bid)).then_some(preheader)
}

pub(super) fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "licm",
        ..Default::default()
    };

    // Functions with exception handling are NOT skipped wholesale -
    // the per-op `is_hoistable` predicate restricts hoisting to a
    // safe-list of side-effect-free, never-throwing ops (constants,
    // pure arithmetic on already-computed values, etc.) for which
    // hoisting out of a try-bearing loop is observably equivalent
    // to lazy in-loop computation: the only behavioural difference
    // is the trivial "computed in preheader even if loop ran zero
    // iterations" case, which doesn't affect any program output.
    //
    // This is critical for performance: the frontend liberally emits
    // CHECK_EXCEPTION ops, and the lower_from_simple detection sets
    // `has_exception_handling = true` whenever any TryStart/TryEnd/
    // CheckException op appears.  In practice virtually every
    // non-trivial loop body trips this flag, so the prior wholesale
    // skip turned LICM into a no-op for nearly all real code.
    //
    // Loop-detection (natural-loop construction via dominators) is
    // unchanged by exception ops, since try_start/try_end/
    // check_exception don't add back-edges or alter the CFG topology
    // beyond the normal block-splitting that any structured
    // construct does.

    if am.get::<LoopForest>(func).headers.is_empty() {
        return stats;
    }
    am.get::<ValueRange>(func);
    am.get::<DefMap>(func);
    am.get::<PredMap>(func);
    am.get::<ImmediateDoms>(func);
    let forest = am.cached::<LoopForest>().unwrap();
    let predecessors = am.cached::<PredMap>().unwrap();
    let tree = am.cached::<ImmediateDoms>().unwrap();
    let value_ranges = am.cached::<ValueRange>().unwrap();
    let definitions = am.cached::<DefMap>().unwrap();
    let value_types = crate::tir::type_refine::extract_exact_scalar_map(func);

    // Inner loops go first. A move into an inner preheader can expose an
    // additional move out of the enclosing loop. Preserve header order on
    // equal depth, and keep the shared CFG and range facts borrowed.
    let mut headers_with_depth: Vec<_> = forest
        .headers
        .iter()
        .map(|&header| {
            let depth = forest
                .headers
                .iter()
                .filter(|&&other| other != header && forest.bodies[&other].contains(&header))
                .count();
            (header, depth)
        })
        .collect();
    headers_with_depth.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));

    // Hoisting changes the defining block of only the moved results.
    let mut moved_definitions = HashMap::default();

    // Collect all values used as block arguments in terminators.
    // These participate in phi resolution and must not be hoisted.
    let mut phi_values: HashSet<ValueId> = HashSet::default();
    for block in func.blocks.values() {
        block
            .terminator
            .for_each_edge(|_, args| phi_values.extend(args.iter().copied()));
    }

    let mut sorted_loop_blocks = Vec::new();
    let mut hoisted = Vec::new();
    for (header, _) in headers_with_depth {
        let loop_blocks = &forest.bodies[&header];
        let Some(preheader) = find_preheader(header, loop_blocks, predecessors, tree) else {
            continue;
        };

        sorted_loop_blocks.clear();
        sorted_loop_blocks.extend(loop_blocks.iter().map(|&block| {
            let position = tree
                .reverse_postorder_position(block)
                .expect("a natural-loop body contains only executable blocks");
            (position, block)
        }));
        sorted_loop_blocks.sort_unstable();

        // SSA definitions precede their uses in executable reverse postorder,
        // with source order within each block. Marking moves as they are
        // extracted lets dependent operations hoist in this same traversal.
        for &(_, loop_block) in &sorted_loop_blocks {
            let block = func.blocks.get_mut(&loop_block).unwrap();
            hoisted.extend(block.ops.extract_if(.., |op| {
                if op.results.is_empty()
                    || op.results.iter().any(|result| phi_values.contains(result))
                    || !is_hoistable(op, value_ranges, &value_types)
                {
                    return false;
                }
                let invariant = op.operands.iter().all(|operand| {
                    moved_definitions
                        .get(operand)
                        .or_else(|| definitions.get(operand))
                        .is_some_and(|&definition| {
                            !loop_blocks.contains(&definition)
                                && tree.dominates(definition, preheader)
                        })
                });
                if invariant {
                    moved_definitions.extend(op.results.iter().map(|&result| (result, preheader)));
                }
                invariant
            }));
            if !hoisted.is_empty() {
                stats.ops_removed += hoisted.len();
                stats.ops_added += hoisted.len();
                func.blocks
                    .get_mut(&preheader)
                    .unwrap()
                    .ops
                    .append(&mut hoisted);
            }
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
