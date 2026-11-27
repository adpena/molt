//! A lexical level with no reader between its push and its observations can
//! stay virtual until the first physical observer. The TIR's handler labels,
//! payloads, source location and eventual retirement remain authoritative.

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{build_pred_map, exception_transfer_target};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{ExceptionEdgeKind, copy_kind_mints_owned_alias_ref_table};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;
use molt_codegen_abi::boxed_word::{generate_box_inline_int, generate_inline_int_fits};
use molt_codegen_abi::{
    QNAN, TAG_INT, TAG_NONE, THREAD_RECORD_EXCEPTION_HANDLER_DEPTH_OFFSET,
    THREAD_RECORD_EXCEPTION_STACK_BASELINE_OFFSET,
};
use molt_ir::OpKind;
use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, IntBinary, IntComparison};
use super::function::FunctionCx;

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// A virtual level never changed either the depth or the active stack's
    /// length. The first restore verifies that it targets that same depth.
    /// If it has to call the runtime, later restores must remain physical:
    /// alignment can release an exception and reenter Python.
    pub(super) fn restore_virtual_depth(
        &self,
        b: &mut B,
        target: ValueId,
        slot: B::Slot,
        first: bool,
    ) -> bool {
        let operands = self.operands();
        // All of these restores use the same SSA value. If turning it into
        // an ABI word could allocate, keep ordinary operand custody at each.
        if !self.runtime.contains(RuntimeFn::ExceptionStackSetDepth) || operands.mints_owner(target)
        {
            return false;
        }
        let target = operands.boxed_word(b, target);
        let state = b.load_slot(slot);
        let mismatch = if first {
            let thread = b.thread_record_word();
            let record = b.word_to_address(thread);
            let depth = b.load_word(record, THREAD_RECORD_EXCEPTION_HANDLER_DEPTH_OFFSET);
            // Use the same nonnegative u32 subset as baseline exit.
            let high = b.and(depth, !u64::from(u32::MAX));
            let expected = b.or(depth, QNAN | TAG_INT);
            let difference = b.int_binary(IntBinary::Xor, target, expected, false);
            let difference = b.int_binary(IntBinary::Or, difference, high, false);
            b.int_binary(IntBinary::Or, difference, state, false)
        } else {
            state
        };
        let zero = b.constant(0);
        let physical = b.int_compare(IntComparison::NotEqual, mismatch, zero);
        let apply = b.create_block("exception_depth_restore");
        let done = b.create_block("exception_depth_ready");
        b.branch_if(physical, (apply, &[]), (done, &[]));
        b.switch_to_block(apply);
        if first {
            let one = b.constant(1);
            b.store_slot(slot, one);
        }
        let target = b.word_value(target);
        cold_coordinate(b, RuntimeFn::ExceptionStackSetDepth, &[target]);
        b.jump(done, &[]);
        b.switch_to_block(done);
        true
    }

    /// The scalar coordinates are independent of the active exception stack's
    /// owned objects. SetDepth, Push and Pop still use the physical stack.
    pub(super) fn exception_coordinate_call(
        &mut self,
        b: &mut B,
        op: &TirOp,
        entry: RuntimeFn,
    ) -> bool {
        let result = match entry {
            RuntimeFn::ExceptionStackDepth | RuntimeFn::ExceptionStackEnter => {
                let enter = entry == RuntimeFn::ExceptionStackEnter;
                let offset = if enter {
                    THREAD_RECORD_EXCEPTION_STACK_BASELINE_OFFSET
                } else {
                    THREAD_RECORD_EXCEPTION_HANDLER_DEPTH_OFFSET
                };
                let thread = b.thread_record_word();
                let record = b.word_to_address(thread);
                let previous = b.load_word(record, offset);
                let fits = generate_inline_int_fits(b, previous);
                b.choose(
                    fits,
                    |b| {
                        if enter {
                            let depth =
                                b.load_word(record, THREAD_RECORD_EXCEPTION_HANDLER_DEPTH_OFFSET);
                            b.store_word(
                                record,
                                THREAD_RECORD_EXCEPTION_STACK_BASELINE_OFFSET,
                                depth,
                            );
                        }
                        generate_box_inline_int(b, previous)
                    },
                    |b| cold_coordinate(b, entry, &[]),
                )
            }
            RuntimeFn::ExceptionStackExit => {
                self.operands()
                    .with_words(b, &op.operands, "exception_baseline", |b, words, _| {
                        let previous = words[0];
                        // A nonnegative u32 payload fits both native usize and wasm32.
                        // The runtime decodes all other values, including None on
                        // cleanup paths with no saved baseline.
                        let header = b.and(previous, !u64::from(u32::MAX));
                        let tag = b.constant(QNAN | TAG_INT);
                        let inline = b.int_compare(IntComparison::Equal, header, tag);
                        b.choose(
                            inline,
                            |b| {
                                let raw = b.and(previous, u64::from(u32::MAX));
                                let thread = b.thread_record_word();
                                let record = b.word_to_address(thread);
                                b.store_word(
                                    record,
                                    THREAD_RECORD_EXCEPTION_STACK_BASELINE_OFFSET,
                                    raw,
                                );
                                b.constant(QNAN | TAG_NONE)
                            },
                            |b| {
                                let previous = b.word_value(previous);
                                cold_coordinate(b, entry, &[previous])
                            },
                        )
                    })
            }
            _ => return false,
        };
        let value = b.word_value(result);
        self.bind_owned(b, op, value);
        true
    }
}

fn cold_coordinate<B: CodegenBuilder>(b: &mut B, entry: RuntimeFn, args: &[B::Value]) -> B::Word {
    let callee = b.runtime_callee(entry);
    let result = b
        .call_cold(callee, args)
        .expect("exception stack coordinate returns a word");
    b.expect_word(result)
}

type Point = (BlockId, usize);

#[derive(Clone, Copy)]
pub(crate) enum Event {
    Enter,
    Observe,
    Retire,
    Restore { first: bool },
}

fn nullary(op: &TirOp, kind: OpKind) -> bool {
    op.opcode == OpCode::Copy
        && op.origin == Some(kind)
        && op.operands.is_empty()
        && op.results.is_empty()
}

fn only_successor(
    func: &TirFunction,
    preds: &HashMap<BlockId, Vec<BlockId>>,
    labels: &HashMap<i64, BlockId>,
    id: BlockId,
) -> Option<BlockId> {
    let block = &func.blocks[&id];
    let Terminator::Branch { target, args } = &block.terminator else {
        return None;
    };
    // A normal and an exceptional edge from the same block can both reach
    // the successor even when its predecessor list contains only this ID.
    (args.is_empty()
        && preds.get(target)?.as_slice() == [id]
        && !block
            .ops
            .iter()
            .any(|op| exception_transfer_target(op, labels) == Some(*target)))
    .then_some(*target)
}

fn frame_transport(op: &TirOp) -> bool {
    op.opcode == OpCode::FrameSetLocation || op.is_plain_value_copy() || nullary(op, OpKind::Line)
}

/// Find the same depth restore on the unobserved tail of the retiring
/// level. A call, release, second depth target or rejoining edge ends
/// this proof. The exceptional successors keep their original cleanup.
fn restores(
    func: &TirFunction,
    preds: &HashMap<BlockId, Vec<BlockId>>,
    labels: &HashMap<i64, BlockId>,
    (mut id, index): Point,
) -> Vec<Point> {
    let mut start = index + 1;
    let mut visited = HashSet::default();
    let mut restores = Vec::new();
    let mut depth: Option<ValueId> = None;
    while visited.insert(id) {
        for (index, op) in func.blocks[&id].ops.iter().enumerate().skip(start) {
            if op.opcode == OpCode::Copy
                && op.origin == Some(OpKind::ExceptionStackSetDepth)
                && let [saved] = op.operands[..]
                && op.results.is_empty()
                && depth.is_none_or(|depth| depth == saved)
            {
                depth = Some(saved);
                restores.push((id, index));
            } else if !frame_transport(op)
                && !(op.opcode == OpCode::CheckException && !op.is_async_work_poll())
            {
                return restores;
            }
        }
        let Some(next) = only_successor(func, preds, labels, id) else {
            break;
        };
        id = next;
        start = 0;
    }
    restores
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
            if !(frame_transport(op)
                || op.opcode == OpCode::Copy
                    && op.origin.is_some_and(copy_kind_mints_owned_alias_ref_table))
            {
                return None;
            }
        }
        id = only_successor(func, preds, labels, id)?;
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
            let preds = preds.get_or_init(|| build_pred_map(func));
            let Some(mut points) = region(func, preds, labels, entry) else {
                continue;
            };
            let retire = points.pop().expect("a virtual level has a normal pop");
            let slot = b.entry_slot("exception_level_installed");
            slots.insert(entry, (Event::Enter, slot));
            slots.insert(retire, (Event::Retire, slot));
            for point in points {
                slots.insert(point, (Event::Observe, slot));
            }
            for (index, restore) in restores(func, preds, labels, retire)
                .into_iter()
                .enumerate()
            {
                slots.insert(restore, (Event::Restore { first: index == 0 }, slot));
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
            Self::Enter | Self::Restore { .. } => unreachable!(),
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
