use std::collections::{BTreeSet, HashSet};

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::call_targets::is_gpu_runtime_symbol;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

fn s_value(op: &TirOp) -> Option<&str> {
    match op.attrs.get("s_value") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// One statically-resolvable, inlinable call site inside a caller block.
pub(super) struct CallSite {
    /// The caller block containing the `Call`.
    pub(super) block: BlockId,
    /// The op index of the `Call` within that block's `ops`.
    pub(super) op_index: usize,
    /// The callee name (a module-defined function).
    pub(super) callee: String,
}

/// Collect every statically-direct `Call` op in `caller` whose target is a
/// module-defined function (resolved via `s_value`), in deterministic order
/// (blocks sorted by id, ops in index order). Opaque calls, method dispatch,
/// builtin calls, gpu intrinsics, and copy-fallback calls are NOT collected —
/// only a first-class `Call` with an `s_value` naming a `defined` function.
pub(super) fn collect_call_sites(caller: &TirFunction, defined: &[String]) -> Vec<CallSite> {
    let defined_set: BTreeSet<&str> = defined.iter().map(String::as_str).collect();
    let mut sites = Vec::new();
    let mut block_ids: Vec<BlockId> = caller.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    for bid in block_ids {
        let block = &caller.blocks[&bid];
        for (op_index, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::Call {
                continue;
            }
            let Some(name) = s_value(op) else { continue };
            if is_gpu_runtime_symbol(name) {
                continue;
            }
            if !defined_set.contains(name) {
                continue;
            }
            sites.push(CallSite {
                block: bid,
                op_index,
                callee: name.to_string(),
            });
        }
    }
    sites
}

/// REFCOUNT guard: returns true if any of the call's argument values is the
/// result of an `IncRef` in the ≤2 ops immediately before the `Call`. Such a
/// site hands the callee an *owned* argument (the `IncRef` balances a `DecRef`
/// the callee would issue under a +1 convention, or the caller is materializing
/// an owned temporary). Inlining a +0-borrowed-parameter body there would leak
/// the extra reference, so the site is refused.
///
/// `IncRef`'s reference target is its operand (the value being retained). We
/// scan the two preceding ops for an `IncRef` whose operand is one of the call's
/// argument operands.
pub(super) fn call_site_has_arg_incref(
    block: &TirBlock,
    call_op_index: usize,
    call_args: &[ValueId],
) -> bool {
    if call_args.is_empty() {
        return false;
    }
    let arg_set: HashSet<ValueId> = call_args.iter().copied().collect();
    let lo = call_op_index.saturating_sub(2);
    for op in &block.ops[lo..call_op_index] {
        if op.opcode == OpCode::IncRef && op.operands.iter().any(|v| arg_set.contains(v)) {
            return true;
        }
    }
    false
}
