use molt_ir::deterministic_hash::{HashMap, HashSet};

use super::super::escape_analysis::{EscapeState, ParameterCapture};
use super::super::value_identity::{build_copy_map, resolve_copy};
use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::call_graph::CallGraph;
use crate::tir::call_targets::{gpu_runtime_result_type_for_op, static_call};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_is_inliner_numeric_raw_lane_consumer_table;
use crate::tir::ops::{OpCode, TirOp};
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

/// SSA witnesses for profitable inlining beyond the ordinary cost budget.
#[derive(Default)]
pub(super) struct InlineSiteUses {
    split_fields: HashSet<ValueId>,
    fresh_objects: HashSet<ValueId>,
    capture: std::cell::OnceCell<HashMap<ValueId, EscapeState>>,
    numeric_consumers: HashMap<ValueId, OpCode>,
}

impl InlineSiteUses {
    pub(super) fn of(caller: &TirFunction) -> Self {
        let mut uses = Self::default();
        for op in caller.blocks.values().flat_map(|block| &block.ops) {
            if op.opcode == OpCode::Copy && op.origin == Some(molt_ir::OpKind::StringSplitField) {
                uses.split_fields.extend(op.results.iter().copied());
            }
            if let Some(allocation) = super::super::typed_slot_access::boxed_allocation_layout(op)
                && !super::super::escape_analysis::op_result_defines_del(op)
            {
                uses.fresh_objects.insert(allocation.result);
            }
            if opcode_is_inliner_numeric_raw_lane_consumer_table(op.opcode)
                || super::super::checked_int_loops::is_checked(op.opcode)
                || op.opcode == OpCode::UnboxInlineInt
            {
                for &operand in &op.operands {
                    uses.numeric_consumers.entry(operand).or_insert(op.opcode);
                }
            }
        }
        uses.follow_numeric_operands(caller);
        uses
    }

    /// Price an opportunity at its physical producer, including a result
    /// projected on success and values forwarded into a loop through phis.
    /// These are profitability witnesses and never establish carrier safety.
    fn follow_numeric_operands(&mut self, caller: &TirFunction) {
        if self.numeric_consumers.is_empty() {
            return;
        }
        let copies = build_copy_map(caller);
        let incomings: HashMap<_, _> = crate::representation_facts::block_arg_incomings_for(caller)
            .into_iter()
            .filter_map(|((block, position), sources)| {
                Some((caller.blocks[&block].args.get(position)?.id, sources))
            })
            .collect();
        let mut pending: Vec<_> = self.numeric_consumers.keys().copied().collect();
        while let Some(value) = pending.pop() {
            let consumer = self.numeric_consumers[&value];
            let root = resolve_copy(&copies, value);
            let incoming = incomings.get(&value).map(Vec::as_slice).unwrap_or_default();
            for source in std::iter::once(root).chain(incoming.iter().copied()) {
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.numeric_consumers.entry(source)
                {
                    entry.insert(consumer);
                    pending.push(source);
                }
            }
        }
    }

    pub(super) fn receives_split_field(&self, op: &TirOp) -> bool {
        !self.split_fields.is_empty()
            && static_call(op).is_some_and(|call| {
                call.arguments
                    .iter()
                    .any(|arg| self.split_fields.contains(arg))
            })
    }

    /// A fresh object whose only nonlocal use is as a noncapturing argument
    /// can become local after splicing. Check every alias and every other
    /// use before spending the larger budget, once per candidate caller.
    pub(super) fn receives_noncaptured_allocation<'a>(
        &self,
        op: &TirOp,
        caller: &TirFunction,
        parameters: &mut ParameterCapture,
        body: impl Fn(molt_ir::FunctionId) -> Option<&'a TirFunction>,
    ) -> bool {
        let Some(call) = static_call(op) else {
            return false;
        };
        if !call
            .arguments
            .iter()
            .any(|v| self.fresh_objects.contains(v))
        {
            return false;
        }
        let states = self
            .capture
            .get_or_init(|| parameters.analyze(caller, body));
        call.arguments.iter().any(|v| {
            self.fresh_objects.contains(v) && states.get(v) == Some(&EscapeState::ArgEscape)
        })
    }

    pub(super) fn numeric_consumer(&self, op: &TirOp) -> Option<(ValueId, OpCode)> {
        let result = *op.results.first()?;
        self.numeric_consumers
            .get(&result)
            .map(|&consumer| (result, consumer))
    }
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
