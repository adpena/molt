//! The observable state of a synchronous activation before each instruction.
//! A home names its post-store SSA view. Its code slot is the frontend's
//! localsplus index, independent of the inlined body's new value numbers.

use std::collections::VecDeque;
use std::rc::Rc;

use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::frame_entry::FrameEntryKind;
use molt_ir::runtime_fn_generated::RuntimeFn;
use molt_ir::{ExecutionContextPolicy, OpKind};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{
    exception_label_to_block, exception_transfer_target, terminator_successors,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::{
    frame_elision, frame_home_placement, frame_location_placement, spliced_frame,
};
use crate::tir::values::ValueId;

#[derive(Debug, Clone)]
pub(super) struct FramePoint {
    pub(super) location: Option<i64>,
    pub(super) homes: Rc<[(i64, ValueId)]>,
    pub(super) published: bool,
}

#[derive(Debug, Clone)]
pub(super) struct SyncFrame {
    points: HashMap<BlockId, Vec<Option<FramePoint>>>,
}

#[derive(Clone, PartialEq, Eq)]
struct State {
    live: bool,
    // None is a join with different bound slots. Such a join can retire
    // already-published frames, but cannot reconstruct one for an observer.
    homes: Option<Rc<[(i64, ValueId)]>>,
    published: bool,
    had_location: bool,
}

impl SyncFrame {
    pub(super) fn point(&self, block: BlockId, index: usize) -> Option<&FramePoint> {
        self.points.get(&block)?.get(index)?.as_ref()
    }

    /// The composed path snapshots the original, singly assigned formal homes.
    pub(super) fn analyze(func: &TirFunction) -> Option<Self> {
        if func.execution_context != ExecutionContextPolicy::Local
            || func.frame_locals_exposed()
            || func.has_state_machine()
            || func.has_exception_handlers()
            || func.is_codegen_partition()
        {
            return None;
        }
        let entry = func.blocks.get(&func.entry_block)?.ops.first()?;
        if !is_marker(entry, OpKind::TraceEnterSlot)
            || !entry.operands.is_empty()
            || !entry.results.is_empty()
        {
            return None;
        }
        let Some(AttrValue::Int(entry_value)) = entry.attrs.get("value") else {
            return None;
        };
        let shape = FrameEntryKind::from_entry_value(*entry_value).shape()?;
        if shape.entry_value() != *entry_value {
            return None;
        }
        let params: HashSet<_> = func.blocks[&func.entry_block]
            .args
            .iter()
            .map(|arg| arg.id)
            .collect();
        let mut written = HashSet::default();
        let mut carriers = None;
        for (&id, block) in &func.blocks {
            for (at, op) in block.ops.iter().enumerate() {
                if is_marker(op, OpKind::TraceEnterSlot) && (id != func.entry_block || at != 0) {
                    return None;
                }
                if op.origin == Some(OpKind::FrameHomeStore) {
                    let slot = frame_location_placement::home_slot(op)?;
                    if slot >= shape.homes as usize
                        || op.opcode != OpCode::Copy
                        || op.operands.len() != 1
                        || op.results.len() != 1
                        || !params.contains(&op.operands[0])
                        || !written.insert(slot)
                    {
                        return None;
                    }
                    // Module inlining precedes the pass that records this
                    // proof. Query the same representation rule lazily.
                    if !op.has_nothrow_proof()
                        && frame_home_placement::boxes_heap_int(
                            op,
                            carriers.get_or_insert_with(|| {
                                crate::representation_facts::analyze_representations(func)
                            }),
                        )
                    {
                        return None;
                    }
                }
                if matches!(
                    op.origin,
                    Some(
                        OpKind::FrameHomeLoad
                            | OpKind::FrameHomeClear
                            | OpKind::FrameHomeTake
                            | OpKind::FrameHomeCell
                            | OpKind::FrameHomePrivateCell
                            | OpKind::FrameLocalsSet
                            | OpKind::FrameContextSet
                    )
                ) || frame_location_placement::direct_frame_runtime(op)
                    || spliced_frame::is_spliced_frame_call(op)
                {
                    return None;
                }
            }
        }
        let locations = frame_location_placement::reaching_locations(func);
        let labels = exception_label_to_block(func);
        let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
        let initial = State {
            live: false,
            homes: Some(Rc::from([])),
            published: false,
            had_location: false,
        };
        let mut incoming = HashMap::default();
        incoming.insert(func.entry_block, initial);
        let mut queue = VecDeque::from([func.entry_block]);
        let mut points = HashMap::default();
        while let Some(id) = queue.pop_front() {
            let block = func.blocks.get(&id)?;
            let mut state = incoming[&id].clone();
            let mut row = Vec::with_capacity(block.ops.len());
            for (at, op) in block.ops.iter().enumerate() {
                let exit = is_marker(op, OpKind::TraceExit);
                let check = op.opcode == OpCode::CheckException;
                let location = op
                    .frame_location()
                    .filter(|_| check)
                    .or_else(|| locations.get(&(id, at)).copied());
                let frozen_exit = exit && state.published;
                if state.live
                    && !frozen_exit
                    && (state.homes.is_none() || (state.had_location && location.is_none()))
                    && (exit
                        || check
                        || op.runtime == Some(RuntimeFn::ExceptionTracePrepend)
                        || spliced_frame::may_observe_frames(op, &exact))
                    && op.opcode != OpCode::FrameSetLocation
                {
                    return None;
                }
                row.push(state.live.then(|| FramePoint {
                    location,
                    homes: state.homes.clone().unwrap_or_else(|| Rc::from([])),
                    published: state.published,
                }));
                if is_marker(op, OpKind::TraceEnterSlot) {
                    if state.live {
                        return None;
                    }
                    state.live = true;
                } else if exit {
                    if !state.live || !op.operands.is_empty() || !op.results.is_empty() {
                        return None;
                    }
                    state.live = false;
                    state.homes = Some(Rc::from([]));
                    state.had_location = false;
                    state.published = false;
                } else if op.origin == Some(OpKind::FrameHomeStore) {
                    if !state.live {
                        return None;
                    }
                    let key = i64::try_from(frame_location_placement::home_slot(op)?).ok()?;
                    let mut homes = state.homes.as_ref()?.to_vec();
                    let index = homes.binary_search_by_key(&key, |&(slot, _)| slot).err()?;
                    homes.insert(index, (key, op.results[0]));
                    state.homes = Some(Rc::from(homes));
                    state.published = false;
                } else if op.opcode == OpCode::FrameSetLocation {
                    if !state.live
                        || !matches!(op.attrs.get("value"), Some(AttrValue::Int(value)) if *value >= 0)
                    {
                        return None;
                    }
                    state.had_location = true;
                    state.published = false;
                } else if check {
                    let target = exception_transfer_target(op, &labels)?;
                    let kind = op.exception_edge_kind()?;
                    let mut failure = state.clone();
                    failure.had_location |= op.frame_location().is_some();
                    failure.published |=
                        kind == ExceptionEdgeKind::Origin || op.is_async_work_poll();
                    merge(target, failure, &mut incoming, &mut queue)?;
                    // An asynchronous handler may have materialized the
                    // check's own location on its normal continuation.
                    state.had_location |= op.is_async_work_poll() && op.frame_location().is_some();
                }
            }
            if state.live
                && (block.terminator.exits_activation()
                    || frame_elision::terminator_observer(&block.terminator, &exact).is_some())
            {
                return None;
            }
            if matches!(block.terminator, Terminator::StateDispatch { .. }) {
                return None;
            }
            for target in terminator_successors(&block.terminator) {
                merge(target, state.clone(), &mut incoming, &mut queue)?;
            }
            points.insert(id, row);
        }
        Some(Self { points })
    }
}

fn is_marker(op: &TirOp, kind: OpKind) -> bool {
    op.opcode == OpCode::Copy && op.origin == Some(kind)
}

/// The entry state weakens monotonically. A back edge through an existing
/// store is refused when that store would replace a home it already owns.
fn merge(
    id: BlockId,
    state: State,
    incoming: &mut HashMap<BlockId, State>,
    queue: &mut VecDeque<BlockId>,
) -> Option<()> {
    if let Some(prior) = incoming.get_mut(&id) {
        if prior.live != state.live {
            return None;
        }
        let mut next = prior.clone();
        if next.homes != state.homes {
            next.homes = None;
        }
        next.published &= state.published;
        next.had_location |= state.had_location;
        if next == *prior {
            return Some(());
        }
        *prior = next;
    } else {
        incoming.insert(id, state);
    }
    queue.push_back(id);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapper() -> TirFunction {
        let ir: crate::ir::FunctionIR = serde_json::from_value(serde_json::json!({
            "return_abi": "void", "name": "wrapper", "execution_context": "local",
            "params": ["p"], "param_types": ["Any"], "ops": [
                {"kind":"trace_enter_slot","s_value":"wrapper","value":1},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"frame_home_store","value":0,"args":["p"],"out":"home"},
                {"kind":"frame_set_location","value":7},
                {"kind":"call","s_value":"inspect_caller","args":["home"],"out":"r"},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"frame_set_location","value":4},
                {"kind":"trace_exit"},{"kind":"ret_void"},
                {"kind":"label","value":9},{"kind":"trace_exit"},{"kind":"ret_void"}
            ]
        }))
        .unwrap();
        crate::tir::lower_from_simple::lower_to_tir(&ir)
    }

    #[test]
    fn callback_and_ordinary_exit_see_their_own_locations_and_home() {
        let func = wrapper();
        let frame = SyncFrame::analyze(&func).expect("single-store wrapper");
        let home = func
            .blocks
            .values()
            .flat_map(|b| &b.ops)
            .find(|op| op.origin == Some(OpKind::FrameHomeStore))
            .unwrap()
            .results[0];
        let mut exits = Vec::new();
        let mut callbacks = 0;
        for block in func.blocks.values() {
            for (at, op) in block.ops.iter().enumerate() {
                if op.opcode == OpCode::Call {
                    let point = frame.point(block.id, at).unwrap();
                    assert_eq!(point.location, Some(7));
                    assert_eq!(&*point.homes, &[(0, home)]);
                    callbacks += 1;
                } else if is_marker(op, OpKind::TraceExit) {
                    let point = frame.point(block.id, at).unwrap();
                    if !point.published {
                        assert_eq!(&*point.homes, &[(0, home)]);
                        assert_eq!(point.location, Some(4));
                    }
                    exits.push(!point.published);
                }
            }
        }
        exits.sort_unstable();
        assert_eq!(callbacks, 1);
        assert_eq!(exits, [false, true]);
    }

    #[test]
    fn malformed_layout_or_writable_frame_refuses_before_mutation() {
        let mut func = wrapper();
        func.attrs.insert(
            crate::tir::function::FRAME_LOCALS_EXPOSED_ATTR.into(),
            AttrValue::Bool(true),
        );
        assert!(SyncFrame::analyze(&func).is_none());
        func.attrs
            .remove(crate::tir::function::FRAME_LOCALS_EXPOSED_ATTR);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops[0].attrs.insert("value".into(), AttrValue::Int(0));
        assert!(SyncFrame::analyze(&func).is_none());
    }
}
