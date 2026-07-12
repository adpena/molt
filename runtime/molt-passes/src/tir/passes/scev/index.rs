use std::collections::{HashMap, HashSet};

use crate::tir::analysis::LoopForestResult;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

pub(super) type LoopContext = LoopForestResult;

/// Index of where every value is defined and the op (if any) that defines it.
pub(super) struct DefIndex {
    /// value → defining block.
    pub(super) def_block: HashMap<ValueId, BlockId>,
    /// value → (opcode, operands, no_signed_wrap) for op-defined values.
    pub(super) def_op: HashMap<ValueId, (OpCode, Vec<ValueId>, bool)>,
    /// value → constant integer (ConstInt).
    pub(super) const_int: HashMap<ValueId, i64>,
    /// header → its block-argument value ids (in order).
    pub(super) header_args: HashMap<BlockId, Vec<ValueId>>,
    /// Transparent-copy resolution: value → the canonical source value reached
    /// by following plain SSA copies (`is_plain_value_copy`). Lowering inserts
    /// many such copies between the IV phi, the guard comparison and the
    /// back-edge increment; resolving through them is what lets the recurrence
    /// recognizer see the canonical induction-variable shape. A plain copy is
    /// *semantically* the identity, so this resolution introduces no
    /// imprecision (it only removes copy noise).
    pub(super) copy_src: HashMap<ValueId, ValueId>,
}

impl DefIndex {
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

pub(super) fn build_def_index(func: &TirFunction, loop_headers: &HashSet<BlockId>) -> DefIndex {
    let mut def_block = HashMap::new();
    let mut def_op = HashMap::new();
    let mut const_int = HashMap::new();
    let mut header_args: HashMap<BlockId, Vec<ValueId>> = HashMap::new();
    let mut copy_src: HashMap<ValueId, ValueId> = HashMap::new();

    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            def_block.insert(arg.id, bid);
        }
        if loop_headers.contains(&bid) {
            header_args.insert(bid, block.args.iter().map(|a| a.id).collect());
        }
        for op in &block.ops {
            let nsw = matches!(op.attrs.get("no_signed_wrap"), Some(AttrValue::Bool(true)));
            if op.opcode == OpCode::ConstInt
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                for &r in &op.results {
                    const_int.insert(r, *v);
                }
            }
            // Record a transparent-copy edge result → source for a plain value
            // copy (single operand, single result, no semantic attrs).
            if op.is_plain_value_copy() {
                copy_src.insert(op.results[0], op.operands[0]);
            }
            for &r in &op.results {
                def_block.insert(r, bid);
                def_op.insert(r, (op.opcode, op.operands.clone(), nsw));
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
        header_args,
        copy_src,
    }
}

/// Incoming edges to a header that pass arguments: for each predecessor block,
/// the argument vector it forwards to `header` and whether it is a back-edge
/// (i.e. the predecessor is inside the loop body).
pub(super) struct HeaderIncoming {
    /// (predecessor, args, is_back_edge)
    pub(super) edges: Vec<(BlockId, Vec<ValueId>, bool)>,
}

pub(super) fn collect_header_incoming(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
) -> HeaderIncoming {
    let mut edges = Vec::new();
    for (&bid, block) in &func.blocks {
        let is_back = body.contains(&bid);
        match &block.terminator {
            Terminator::Branch { target, args } if *target == header => {
                edges.push((bid, args.clone(), is_back));
            }
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == header {
                    edges.push((bid, then_args.clone(), is_back));
                }
                if *else_block == header {
                    edges.push((bid, else_args.clone(), is_back));
                }
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            } => {
                for (_, tgt, args) in cases {
                    if *tgt == header {
                        edges.push((bid, args.clone(), is_back));
                    }
                }
                if *default == header {
                    edges.push((bid, default_args.clone(), is_back));
                }
            }
            _ => {}
        }
    }
    HeaderIncoming { edges }
}
