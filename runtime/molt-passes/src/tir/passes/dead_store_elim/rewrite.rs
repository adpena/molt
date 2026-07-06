use std::collections::{HashMap, HashSet};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::TirBlock;
use crate::tir::function::TirFunction;
use crate::tir::passes::PassStats;
use crate::tir::passes::alias_analysis::{AliasAnalysis, AliasAnalysisResult};
use crate::tir::values::ValueId;

use super::access::{stack_object_alloc_result, typed_slot_store};
use super::escape::{compute_escaping_roots, terminator_uses_root};

/// Run dead-store elimination on a single block.  Returns the number
/// of ops removed.
///
/// The slot-observation barrier ("could this op read/escape the slot of object
/// `root`?") and the transparent-SSA-copy alias roots are now answered by the
/// first-class alias analysis ([`AliasAnalysisResult::may_observe_slot`] /
/// [`AliasUnionFind`]) - the single source of truth that replaces the former
/// inline `AliasState` union-find and `may_observe_slot` list (Tier-0 S5 phase 1).
///
/// SOUNDNESS NOTE: the alias union-find here is computed over the *whole
/// function* in a single forward scan, whereas the former code rebuilt it
/// incrementally as it walked this block. In valid SSA every value is defined
/// before use, so the whole-function union-find is a **superset** of the
/// former incremental one at every program point. A superset of alias edges can
/// only make `operand_aliases_root` MORE often true: MORE pending stores
/// invalidated (observers detected), strictly more conservative. We therefore
/// never eliminate a store the old code would have kept live.
fn run_block(
    block: &mut TirBlock,
    alias: &AliasAnalysisResult,
    escaping_roots: &HashSet<ValueId>,
) -> usize {
    // Walk forward.  For each store at (obj, offset), record (idx, obj,
    // offset).  When we see a later store at the same (obj, offset)
    // with no intervening observer, mark the earlier one for removal.
    //
    // `pending`: most recent live store keyed by (obj, offset).
    //   When a new store at the same key arrives, the old store is
    //   killed (added to dead_indices).
    let mut pending: HashMap<(ValueId, i64), usize> = HashMap::new();
    let mut dead_indices: Vec<usize> = Vec::new();
    let mut stack_object_roots: HashSet<ValueId> = HashSet::new();

    for (idx, op) in block.ops.iter().enumerate() {
        // First: any op that observes `obj` invalidates pending stores
        // for that obj.  We must do this BEFORE handling stores so that
        // a load-then-store sequence doesn't kill the load's witness.
        let mut invalidated_keys: Vec<(ValueId, i64)> = Vec::new();
        for &(obj, offset) in pending.keys() {
            if alias.may_observe_slot(op, obj) {
                invalidated_keys.push((obj, offset));
            }
        }
        for key in &invalidated_keys {
            pending.remove(key);
        }

        if let Some(result) = stack_object_alloc_result(op) {
            stack_object_roots.insert(alias.root(result));
        }

        // Now handle the store, if this is one.
        if let Some((target, offset)) = typed_slot_store(op) {
            let key = (alias.root(target), offset);
            if let Some(prev_idx) = pending.insert(key, idx) {
                // The previous store at this (obj, offset) is dead.
                dead_indices.push(prev_idx);
            }
        }
    }

    // Pattern 2: the FINAL store to a stack object is dead iff the object is
    // unobservable outside this block.
    //
    // SOUNDNESS: TIR is MLIR-style block-argument SSA in name only: the SSA
    // construction admits *dominance-based* cross-block uses (a value defined
    // in a dominating block may be referenced in a dominated block WITHOUT being
    // threaded as a block argument; codegen resolves it via the dominance tree).
    // So an object whose pointer is captured (e.g. a `Copy` alias) can be read
    // by a `LoadAttr` in a LATER block while this block's terminator carries no
    // argument for it. The former `!terminator_uses_root` check modeled escape
    // via block-argument threading ALONE and therefore dropped the constructor's
    // field stores whenever a try/except (or any CFG split) separated the object
    // construction from a later field read, a silent zero-default miscompile.
    //
    // The correct precondition is whole-function: the object's alias root must
    // not be referenced in ANY block other than this one (`escaping_roots` is the
    // precomputed superset of roots used outside their producing block, covering
    // operands, terminator-referenced values, AND block-argument bindings). When
    // the root is confined to this block, the local `may_observe_slot` forward
    // walk above has already witnessed every observation, so a surviving `pending`
    // store is the genuinely-final, unread write and is safe to drop.
    for (&(root, _offset), &idx) in &pending {
        if stack_object_roots.contains(&root)
            && !escaping_roots.contains(&root)
            && !terminator_uses_root(&block.terminator, root, &alias.aliases)
        {
            dead_indices.push(idx);
        }
    }

    if dead_indices.is_empty() {
        return 0;
    }

    // Remove ops in reverse-index order to preserve the indices of
    // earlier removals.
    dead_indices.sort_unstable();
    dead_indices.dedup();
    let removed = dead_indices.len();
    for &idx in dead_indices.iter().rev() {
        block.ops.remove(idx);
    }
    removed
}

/// Public entry point - run dead-store elimination on every block.
pub(super) fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let alias = am.get::<AliasAnalysis>(func).clone();
    let escaping_roots = compute_escaping_roots(func, &alias);
    let mut total_removed = 0usize;
    for block in func.blocks.values_mut() {
        total_removed += run_block(block, &alias, &escaping_roots);
    }
    PassStats {
        name: "dead_store_elim",
        values_changed: 0,
        attrs_changed: 0,
        ops_removed: total_removed,
        ops_added: 0,
        facts_changed: 0,
    }
}
