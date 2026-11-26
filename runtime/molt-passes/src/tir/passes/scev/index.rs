use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::analysis::LoopForestResult;
use crate::tir::blocks::BlockId;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

use super::super::value_identity::ScalarFacts;

pub(super) type LoopContext = LoopForestResult;

/// Index of where every value is defined and the op (if any) that defines it.
pub(super) struct DefIndex<'a> {
    /// value → defining block.
    pub(super) def_block: HashMap<ValueId, BlockId>,
    /// value → (opcode, operands, no_signed_wrap) for op-defined values.
    pub(super) def_op: HashMap<ValueId, (OpCode, &'a [ValueId], bool)>,
    /// value → constant integer (ConstInt).
    pub(super) const_int: HashMap<ValueId, i64>,
    /// Argument value → (loop header, incoming payload index).
    pub(super) header_arg_sites: HashMap<ValueId, (BlockId, usize)>,
    /// Every explicit incoming edge from a reachable block, with its own payload.
    pub(super) header_edges: HashMap<BlockId, Vec<(BlockId, &'a [ValueId])>>,
    /// Mathematical scalar resolution, including exact borrowed binding views.
    /// It describes values, never permission to erase publication or callbacks.
    pub(super) copy_src: &'a HashMap<ValueId, ValueId>,
}

impl DefIndex<'_> {
    /// Follow plain-copy edges to the canonical source of `v`.
    pub(super) fn resolve(&self, mut v: ValueId) -> ValueId {
        // The copy graph is a DAG (SSA), but guard against pathological cycles
        // with a bounded walk.
        for _ in 0..64 {
            match self.copy_src.get(&v) {
                Some(&src) if src != v => v = src,
                _ => break,
            }
        }
        v
    }
}

pub(super) fn build_def_index<'a>(
    facts: &'a ScalarFacts<'_>,
    loop_headers: &HashSet<BlockId>,
) -> DefIndex<'a> {
    let func = facts.body();
    let mut def_block = HashMap::default();
    let mut def_op = HashMap::default();
    let mut const_int = HashMap::default();
    let mut header_arg_sites = HashMap::default();
    let mut header_edges: HashMap<_, Vec<_>> = HashMap::default();
    let copy_src = facts.forwarding();
    let reachable = crate::tir::dominators::executable_reachable_blocks(func);

    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            def_block.insert(arg.id, bid);
        }
        if loop_headers.contains(&bid) {
            header_arg_sites.extend(
                block
                    .args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| (arg.id, (bid, index))),
            );
        }
        // Retained lexical latches can carry fabricated phi values. Only
        // executable predecessors contribute to recurrence analysis.
        if reachable.contains(&bid) {
            block.terminator.for_each_labeled_edge(|_, target, args| {
                if loop_headers.contains(&target) {
                    header_edges.entry(target).or_default().push((bid, args));
                }
            });
        }
        for op in &block.ops {
            // A result proven to fit the inline-int47 window is the exact sum:
            // it did not wrap.
            let nsw = matches!(op.attrs.get("no_signed_wrap"), Some(AttrValue::Bool(true)))
                || op.has_inline_int_proof();
            if op.opcode == OpCode::ConstInt
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                for &r in &op.results {
                    const_int.insert(r, *v);
                }
            }
            for &r in &op.results {
                def_block.insert(r, bid);
                def_op.insert(r, (op.opcode, op.operands.as_slice(), nsw));
            }
        }
    }
    // Function parameters are defined at entry.
    for i in 0..func.param_types.len() {
        def_block
            .entry(ValueId(i as u32))
            .or_insert(func.entry_block);
    }

    DefIndex {
        def_block,
        def_op,
        const_int,
        header_arg_sites,
        header_edges,
        copy_src,
    }
}
