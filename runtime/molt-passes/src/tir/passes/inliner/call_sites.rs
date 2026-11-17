use std::collections::HashSet;

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::call_graph::CallGraph;
use crate::tir::call_targets::{gpu_runtime_result_type_for_op, static_call};
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

/// One statically-resolvable, inlinable call site inside a caller block.
pub(super) struct CallSite {
    /// The caller block containing the `Call`.
    pub(super) block: BlockId,
    /// The op index of the `Call` within that block's `ops`.
    pub(super) op_index: usize,
    /// The callee, a function with a body in this module.
    pub(super) callee: molt_ir::FunctionId,
}

/// Collect every static `Call` op in `caller` ([`static_call`]: a direct call
/// or a proven guarded call) whose target is a function with a body in this
/// module, in deterministic order (blocks sorted by id, ops in index order).
/// Opaque calls, method dispatch, builtin calls, gpu intrinsics, and
/// copy-fallback calls are NOT collected.
pub(super) fn collect_call_sites(caller: &TirFunction, call_graph: &CallGraph) -> Vec<CallSite> {
    let mut sites = Vec::new();
    let mut block_ids: Vec<BlockId> = caller.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    for bid in block_ids {
        let block = &caller.blocks[&bid];
        for (op_index, op) in block.ops.iter().enumerate() {
            // A legacy direct Copy transport can retain a call-graph edge,
            // but only first-class calls are supported splice sites.
            if op.opcode != OpCode::Call {
                continue;
            }
            let Some(callee) = static_call(op).map(|call| call.callee) else {
                continue;
            };
            if gpu_runtime_result_type_for_op(op).is_some() || !call_graph.has_body(callee) {
                continue;
            }
            sites.push(CallSite {
                block: bid,
                op_index,
                callee,
            });
        }
    }
    sites
}

/// REFCOUNT guard: returns true if any of the call's argument values is the
/// result of an `IncRef` in the <=2 ops immediately before the `Call`. Such a
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
