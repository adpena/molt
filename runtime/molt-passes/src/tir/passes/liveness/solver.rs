use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

use super::api::TirLivenessResult;
use super::flow::{live_out_of, terminator_direct_uses};
use super::raw::compute_raw_scalars;

/// Backward-dataflow liveness with representation filtering. See module docs.
///
/// **Alias-root canonicalization (design 20 §1.2 — `Copy`/`TypeGuard` are
/// borrowed aliases).** A transparent SSA copy (`b = Copy(a)`, including the
/// SimpleIR `copy_var` / `load_var` / `identity_alias` carriers that lower to
/// `Copy`) names the SAME heap object as its source — it carries NO new
/// reference. Liveness therefore operates in **alias-root space**: every value
/// reference (use, def/kill, edge arg, terminator use) is canonicalized to its
/// alias root before the dataflow runs. This collapses a `Copy`-chain into one
/// ownership entity so the drop pass drops the underlying object EXACTLY ONCE (at
/// the last use of any chain member), instead of once per `Copy` — the latter is
/// a refcount underflow / use-after-free (the loop-carried accumulator loads its
/// phi via `load_var`→`Copy` every iteration; dropping each copy double-frees the
/// live accumulator). Block args have no defining op, so the union-find never
/// unions them away — a loop-header phi stays its own root and is the single
/// owner of the loop-carried value.
pub fn compute_liveness(func: &TirFunction) -> TirLivenessResult {
    let raw_scalars = compute_raw_scalars(func);
    // Alias union-find: canonicalize transparent copies to their root owner.
    let aliases = crate::tir::passes::alias_analysis::build_alias_union_find(func);
    let canon = |v: ValueId| -> ValueId { aliases.root(v) };
    // A root is heap-carrying unless the root itself is a raw scalar. We test the
    // ROOT's repr: the carrier of the owned object is the root's carrier (a
    // `Copy` of a raw i64 is still raw; a `Copy` of a boxed value is boxed).
    let heap_carrying = |v: ValueId| -> bool { !raw_scalars.contains(&canon(v)) };
    // Interior-borrow keepalive (design 20): a use of a value produced by a
    // borrowing read (`LoadAttr`/`Index`) keeps its SOURCE object live too (the
    // result may borrow into / index the source's backing store — e.g. the
    // `Counter._handle` raw-int registry handle, whose owning wrapper's finalizer
    // destroys the registry entry). Threaded into both the per-block Use sets and
    // the edge-arg/terminator-use propagation so the source's live range covers the
    // borrow result's live range identically in forward and backward directions.
    let borrows = crate::tir::passes::alias_analysis::build_borrow_provenance(func, &aliases);
    // The heap-carrying source roots a use of `v` keeps alive (in addition to `v`'s
    // own root). Empty on the common path (no borrowing reads / non-borrow value).
    let keepalive_roots = |v: ValueId| -> Vec<ValueId> {
        if borrows.is_empty() {
            return Vec::new();
        }
        borrows
            .keepalive_roots(v, &canon)
            .into_iter()
            .filter(|&r| !raw_scalars.contains(&r))
            .collect()
    };

    // Per-block block-arg id sets (kills at block entry).
    let mut block_args: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    for (&bid, block) in &func.blocks {
        block_args.insert(bid, block.args.iter().map(|a| a.id).collect());
    }

    // Per-block Use / Kill restricted to heap-carrying values.
    // Use[B]  = values used by an op in B before any in-block def, plus the
    //           terminator's direct uses (cond / switch value / return values).
    // Kill[B] = op results + block args.
    let mut use_set: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    let mut kill_set: HashMap<BlockId, HashSet<ValueId>> = HashMap::new();
    for (&bid, block) in &func.blocks {
        let mut uses: HashSet<ValueId> = HashSet::new();
        let mut defs: HashSet<ValueId> = HashSet::new();
        // Block args are defined at entry. A block arg is its own alias root
        // (no defining op), so `canon` is identity here, but we apply it for
        // uniformity.
        for arg in &block.args {
            defs.insert(canon(arg.id));
        }
        for op in &block.ops {
            for &operand in &op.operands {
                let r = canon(operand);
                // Upward-exposed use: read before it is defined in this block.
                if !defs.contains(&r) && heap_carrying(operand) {
                    uses.insert(r);
                }
                // A use of a borrow result is also a use of its source object(s):
                // each keepalive source root is upward-exposed unless defined
                // earlier in this block (design 20 interior-borrow keepalive).
                for src_root in keepalive_roots(operand) {
                    if !defs.contains(&src_root) {
                        uses.insert(src_root);
                    }
                }
            }
            // A transparent-copy result is the SAME owned object as its root, so
            // it does NOT kill the root (it is a borrow alias). Only NON-alias
            // results define a fresh value. We canonicalize the result: if it
            // aliases an existing root, `canon(res)` is that root and inserting it
            // is a no-op for liveness (the root was already live/defined). A
            // genuine fresh result canonicalizes to itself.
            for &res in &op.results {
                defs.insert(canon(res));
            }
        }
        // Terminator direct uses are reads at the end of the block; they are
        // upward-exposed unless defined earlier in the block.
        for v in terminator_direct_uses(&block.terminator) {
            let r = canon(v);
            if !defs.contains(&r) && heap_carrying(v) {
                uses.insert(r);
            }
            // Borrow keepalive for a terminator direct use (a returned borrow
            // result keeps its source live to the return).
            for src_root in keepalive_roots(v) {
                if !defs.contains(&src_root) {
                    uses.insert(src_root);
                }
            }
        }
        use_set.insert(bid, uses);
        // Kill = all defs (op results + block args), in root space.
        kill_set.insert(bid, defs);
    }

    // Fixpoint over reverse-postorder (processing predecessors-after-successors
    // converges fastest, but any order reaches the same fixpoint).
    let order = crate::tir::dominators::reachable_blocks_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    let block_ids: Vec<BlockId> = {
        let mut v: Vec<BlockId> = func.blocks.keys().copied().collect();
        v.sort_unstable_by_key(|b| b.0);
        v
    };

    let mut live_in: HashMap<BlockId, HashSet<ValueId>> =
        block_ids.iter().map(|&b| (b, HashSet::new())).collect();
    let mut live_out: HashMap<BlockId, HashSet<ValueId>> =
        block_ids.iter().map(|&b| (b, HashSet::new())).collect();

    let mut changed = true;
    while changed {
        changed = false;
        // Iterate in descending BlockId order as a cheap reverse-ish walk.
        for &bid in block_ids.iter().rev() {
            let Some(block) = func.blocks.get(&bid) else {
                continue;
            };
            let new_out = live_out_of(
                block,
                &live_in,
                &block_args,
                &heap_carrying,
                &canon,
                &keepalive_roots,
            );
            // LiveIn = (LiveOut \ Kill) ∪ Use
            let kill = &kill_set[&bid];
            let uses = &use_set[&bid];
            let mut new_in: HashSet<ValueId> = new_out
                .iter()
                .copied()
                .filter(|v| !kill.contains(v))
                .collect();
            new_in.extend(uses.iter().copied());

            if new_out != live_out[&bid] {
                live_out.insert(bid, new_out);
                changed = true;
            }
            if new_in != live_in[&bid] {
                live_in.insert(bid, new_in);
                changed = true;
            }
        }
    }
    // Unreachable blocks (not in the reachable set) keep their empty sets — a
    // block no path executes contributes no liveness.
    let _ = &order;

    TirLivenessResult {
        live_in,
        live_out,
        raw_scalars,
    }
}
