//! A lexical level with no reader between its push and its observations can
//! stay virtual until the first physical observer. The TIR's handler labels,
//! payloads, source location and eventual retirement remain authoritative.

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{build_pred_map, exception_transfer_target};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{ExceptionEdgeKind, copy_kind_mints_owned_alias_ref_table};
use crate::tir::ops::{OpCode, TirOp};
use molt_ir::OpKind;
use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, IntComparison};

type Point = (BlockId, usize);

#[derive(Clone, Copy)]
pub(crate) enum Event {
    Enter,
    Observe,
    Retire,
}

fn nullary(op: &TirOp, kind: OpKind) -> bool {
    op.opcode == OpCode::Copy
        && op.origin == Some(kind)
        && op.operands.is_empty()
        && op.results.is_empty()
}

fn region(
    func: &TirFunction,
    preds: &HashMap<BlockId, Vec<BlockId>>,
    labels: &HashMap<i64, BlockId>,
    entry: Point,
) -> Option<Vec<Point>> {
    let (mut id, start) = entry;
    let marker = func.blocks[&id].ops.get(start + 1)?;
    if marker.opcode != OpCode::TryStart {
        return None;
    }
    let mut visited = HashSet::default();
    let mut at = start + 2;
    let mut observations = Vec::new();
    loop {
        if !visited.insert(id) {
            return None;
        }
        let block = &func.blocks[&id];
        for (index, op) in block.ops.iter().enumerate().skip(at) {
            if nullary(op, OpKind::ExceptionPop) {
                if observations.is_empty() {
                    return None;
                }
                observations.push((id, index));
                return Some(observations);
            }
            if op.opcode == OpCode::CheckException
                && op.exception_edge_kind() == Some(ExceptionEdgeKind::Origin)
            {
                observations.push((id, index));
                continue;
            }
            // Only pure SSA forwarding, retains and location publications. No
            // release, coercion, foreign call or inspection of handled state.
            if !(op.opcode == OpCode::FrameSetLocation
                || op.is_plain_value_copy()
                || op.opcode == OpCode::Copy
                    && (op.origin == Some(OpKind::Line)
                        || op.origin.is_some_and(copy_kind_mints_owned_alias_ref_table)))
            {
                return None;
            }
        }
        let Terminator::Branch { target, args } = &block.terminator else {
            return None;
        };
        // The predecessor map folds duplicate block IDs. A check before the
        // push can still bypass it from that unique predecessor.
        if !args.is_empty()
            || preds.get(target)?.as_slice() != [id]
            || block
                .ops
                .iter()
                .any(|op| exception_transfer_target(op, labels) == Some(*target))
        {
            return None;
        }
        id = *target;
        at = 0;
    }
}

pub(super) fn slots<B: CodegenBuilder>(
    b: &mut B,
    func: &TirFunction,
    labels: &HashMap<i64, BlockId>,
) -> HashMap<Point, (Event, B::Slot)> {
    let mut slots = HashMap::default();
    let preds = std::cell::OnceCell::new();
    for block in func.blocks.values() {
        for (index, op) in block.ops.iter().enumerate() {
            if !nullary(op, OpKind::ExceptionPush) {
                continue;
            }
            let entry = (block.id, index);
            let Some(mut points) = region(
                func,
                preds.get_or_init(|| build_pred_map(func)),
                labels,
                entry,
            ) else {
                continue;
            };
            let retire = points.pop().expect("a virtual level has a normal pop");
            let slot = b.entry_slot("exception_level_installed");
            slots.insert(entry, (Event::Enter, slot));
            slots.insert(retire, (Event::Retire, slot));
            for point in points {
                slots.insert(point, (Event::Observe, slot));
            }
        }
    }
    slots
}

impl Event {
    pub(super) fn transition<B: CodegenBuilder>(self, b: &mut B, slot: B::Slot) {
        if matches!(self, Self::Enter) {
            let installed = b
                .call_runtime(RuntimeFn::ExceptionPushOrDefer, &[])
                .expect("the runtime answers the level's physical status");
            b.store_slot(slot, installed);
            return;
        }
        let state = b.load_slot(slot);
        let zero = b.constant(0);
        let (when, call) = match self {
            Self::Observe => (IntComparison::Equal, RuntimeFn::ExceptionPush),
            Self::Retire => (IntComparison::NotEqual, RuntimeFn::ExceptionPop),
            Self::Enter => unreachable!(),
        };
        let needed = b.int_compare(when, state, zero);
        let apply = b.create_block("exception_level_transition");
        let done = b.create_block("exception_level_ready");
        b.branch_if(needed, (apply, &[]), (done, &[]));
        b.switch_to_block(apply);
        b.call_runtime(call, &[]);
        if matches!(self, Self::Observe) {
            let physical = b.constant(1);
            b.store_slot(slot, physical);
        }
        b.jump(done, &[]);
        b.switch_to_block(done);
    }
}
