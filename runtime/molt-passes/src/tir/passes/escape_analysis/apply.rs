//! The rewrite half of escape analysis: `apply` (stack-promotion + RC strip)
//! and the `run` convenience wrapper. See the module-level docs on [`super`].

use std::collections::{HashMap, HashSet};

use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::analysis::{
    analyze, dict_requiring_alloc_roots, finalizer_alloc_roots, rewritable_alloc_roots,
};
use super::classify::EscapeState;

/// Apply escape analysis results: rewrite non-escaping `Alloc` ops to
/// `StackAlloc`, and remove `IncRef`/`DecRef` on non-escaping values.
///
/// A value is "non-escaping the function" — and therefore stack-promotable —
/// iff its state is `NoEscape` or `ArgEscape`. `ArgEscape` means the value was
/// passed to a callee that provably only *borrows* it (an effect-free / pure
/// builtin or method) and never captures it: it crossed a call boundary but does
/// not outlive the frame, so it is exactly as promotable as a purely frame-local
/// `NoEscape` value. Only `GlobalEscape` (stored to heap/global or returned)
/// forces heap allocation. This preserves the pre-S5 behavior, under which
/// borrowing-call arguments were left at `NoEscape` and promoted.
pub fn apply(func: &mut TirFunction, escapes: &HashMap<ValueId, EscapeState>) -> PassStats {
    let mut stats = PassStats {
        name: "escape_analysis",
        values_changed: 0,
        attrs_changed: 0,
        ops_removed: 0,
        ops_added: 0,
        facts_changed: 0,
    };

    // The escape map now tracks container / task allocation sites too (so the
    // alias analysis can classify their escape state). But stack-promotion and
    // RC removal here apply ONLY to the originally-rewritable allocation roots
    // (`Alloc` / `ObjectNewBound`) and their transparent-move aliases. Touching a
    // `BuildList` / `AllocTask` result's refcount would be unsound (its RC
    // balance is the runtime's, not this pass's, to manage — dropping it risks a
    // leak or use-after-free). Restrict the promotable set accordingly; this
    // exactly preserves the pre-S5 contract.
    let rewritable_roots = rewritable_alloc_roots(func);

    // Instances that receive a generic (dict-routed) attribute store need a heap
    // `__dict__` and must NOT be stack-promoted: a fixed-layout immortal stack
    // object cannot anchor a `__dict__`, so `g.method = fn` (an out-of-layout
    // store) silently no-ops and `g.method()` then raises AttributeError. Exclude
    // these roots from promotion exactly as escape would (heap allocation), the
    // structurally-correct precondition for the dict-materialization path.
    let dict_required = dict_requiring_alloc_roots(func);

    // Instances whose class defines a `__del__` finalizer must stay heap-allocated
    // with a live refcount: stack-promoting them (→ IMMORTAL) or stripping their
    // RC would make the refcount-zero transition never happen, so `dec_ref_ptr`
    // would never dispatch `__del__`. Exclude them from BOTH the stack rewrite and
    // the RC strip below — the single shared fix for the LLVM/WASM/native `__del__`
    // parity hole. The non-finalizer common case is untouched (perf preserved).
    let del_required = finalizer_alloc_roots(func);

    // Collect non-escaping (NoEscape ∪ ArgEscape) values that are rewritable
    // allocation roots — those that do not escape the function and are therefore
    // safe to stack-promote / drop RC on. `ArgEscape` (borrowed-but-not-captured)
    // is as promotable as `NoEscape`.
    let no_escape: HashSet<ValueId> = escapes
        .iter()
        .filter(|&(vid, state)| {
            *state != EscapeState::GlobalEscape
                && rewritable_roots.contains(vid)
                && !dict_required.contains(vid)
                && !del_required.contains(vid)
        })
        .map(|(&vid, _)| vid)
        .collect();

    if no_escape.is_empty() {
        return stats;
    }

    for block in func.blocks.values_mut() {
        // Rewrite alloc-site opcodes for NoEscape values:
        //   Alloc           → StackAlloc
        //   ObjectNewBound  → ObjectNewBoundStack  (Phase 5 step 3)
        //
        // The ObjectNewBound rewrite requires the op to carry the
        // payload size (in bytes) on its `value` attr — the frontend
        // sets this from `class_info["size"]` for typed classes.
        // Without the size, the backend's StackSlot lowering cannot
        // determine the slot size, so we must NOT rewrite or the
        // backend would either fall back to heap (wasting an op
        // kind) or — worse, if the heap fallback were missing —
        // SIGSEGV.  When the size is missing, the heap path stands.
        for op in &mut block.ops {
            if op.opcode == OpCode::Alloc && op.results.iter().any(|r| no_escape.contains(r)) {
                op.opcode = OpCode::StackAlloc;
                stats.values_changed += 1;
            } else if op.opcode == OpCode::ObjectNewBound
                && op.results.iter().any(|r| no_escape.contains(r))
            {
                // Only rewrite when we have a payload size to size
                // the StackSlot with.  The frontend always emits the
                // size for the class-instantiation fold, but defend
                // against synthetic ops that lack it.
                let has_size = matches!(
                    op.attrs.get("value"),
                    Some(crate::tir::ops::AttrValue::Int(v)) if *v > 0
                );
                if has_size {
                    op.opcode = OpCode::ObjectNewBoundStack;
                    stats.values_changed += 1;
                }
            }
        }

        // Remove IncRef/DecRef on NoEscape values.
        let before_len = block.ops.len();
        block.ops.retain(|op| {
            !((op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef)
                && op.operands.iter().any(|o| no_escape.contains(o)))
        });
        stats.ops_removed += before_len - block.ops.len();
    }

    stats
}

/// Convenience: analyze + apply in one step.
pub fn run(func: &mut TirFunction) -> PassStats {
    let escapes = analyze(func);
    apply(func, &escapes)
}
