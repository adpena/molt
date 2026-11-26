//! The observable state of a synchronous activation before each instruction.
//! A home names its post-store SSA view. Its code slot is the frontend's
//! localsplus index, independent of the inlined body's new value numbers.

use std::collections::{BTreeMap, VecDeque};
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
    joined_homes: bool,
    /// False only at an exit whose exceptional predecessor already published
    /// the final frame on every path. A join of two such paths may have two
    /// different, already frozen locations. Re-linking would overwrite them.
    pub(super) refresh_exit: bool,
    pub(super) published: bool,
}

#[derive(Debug, Clone)]
pub(super) struct SyncFrame {
    points: HashMap<BlockId, Vec<Option<FramePoint>>>,
    pub(super) nested: super::nested_frames::NestedFrames,
    /// Non-formal stores own their Python value until the eager frame would pop.
    private: BTreeMap<i64, ValueId>,
    descending: bool,
    joined_entries: Vec<BlockId>,
    outgoing: HashMap<BlockId, Option<Rc<[(i64, ValueId)]>>>,
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

    pub(super) fn has_private_homes(&self) -> bool {
        !self.private.is_empty()
    }

    pub(super) fn owns_home(&self, op: &TirOp) -> bool {
        frame_location_placement::home_slot(op)
            .and_then(|slot| self.private.get(&(slot as i64)))
            .is_some()
    }

    /// Transfer each private home's credit through a frozen exception join to
    /// the original TraceExit. A pre-store edge contributes None, whose release
    /// is a no-op. The same SSA arcs carry custody through normal and exceptional
    /// transfers under DropInsertion. No joined state is used to take a snapshot.
    pub(super) fn prepare_private(
        &self,
        func: &TirFunction,
        body: &mut TirFunction,
        entry_ops: &mut Vec<TirOp>,
    ) -> Option<HashMap<(BlockId, usize), Vec<ValueId>>> {
        use crate::tir::types::TirType;
        use crate::tir::values::TirValue;

        let mut clears = HashMap::default();
        if self.private.is_empty() {
            return Some(clears);
        }
        let absent = (!self.joined_entries.is_empty()).then(|| {
            let value = body.fresh_value();
            body.value_types.insert(value, TirType::None);
            entry_ops.push(TirOp::new(OpCode::ConstNone, vec![], vec![value]));
            value
        });
        let mut joined = HashMap::default();
        for &id in &self.joined_entries {
            let mut args = Vec::with_capacity(self.private.len());
            for _ in &self.private {
                let value = body.fresh_value();
                body.value_types.insert(value, TirType::DynBox);
                body.blocks.get_mut(&id)?.args.push(TirValue {
                    id: value,
                    ty: TirType::DynBox,
                });
                args.push(value);
            }
            joined.insert(id, args);
        }
        let values = |id, homes: Option<&[(i64, ValueId)]>| -> Option<Vec<ValueId>> {
            let Some(homes) = homes else {
                return joined.get(&id).cloned();
            };
            Some(
                self.private
                    .keys()
                    .filter_map(|key| {
                        homes
                            .binary_search_by_key(key, |&(k, _)| k)
                            .ok()
                            .map(|index| homes[index].1)
                            .or(absent)
                    })
                    .collect(),
            )
        };
        let labels = exception_label_to_block(func);
        for (&id, block) in &func.blocks {
            for (at, op) in block.ops.iter().enumerate() {
                let point = self.point(id, at);
                let homes = point.map_or(Some(&[][..]), |p| (!p.joined_homes).then_some(&*p.homes));
                if let Some(target) = exception_transfer_target(op, &labels)
                    && joined.contains_key(&target)
                {
                    body.blocks.get_mut(&id)?.ops[at]
                        .operands
                        .extend(values(id, homes)?);
                }
                if is_marker(op, OpKind::TraceExit) && point.is_some() {
                    let mut held = values(id, homes)?;
                    held.retain(|&value| Some(value) != absent);
                    if self.descending {
                        held.reverse();
                    }
                    clears.insert((id, at), held);
                }
            }
            let homes = self
                .outgoing
                .get(&id)
                .map_or(Some(&[][..]), |state| state.as_deref());
            if terminator_successors(&block.terminator)
                .into_iter()
                .any(|id| joined.contains_key(&id))
            {
                let payload = values(id, homes)?;
                body.blocks
                    .get_mut(&id)?
                    .terminator
                    .for_each_edge_mut(|target, args| {
                        if joined.contains_key(target) {
                            args.extend_from_slice(&payload);
                        }
                    });
            }
        }
        Some(clears)
    }

    pub(super) fn analyze(func: &TirFunction) -> Option<Self> {
        Self::analyze_with_owned_formals(func, false)
    }

    /// A guarded read can retain the physical homes themselves through a
    /// post-read observer, including when the caller transferred its last owner.
    pub(super) fn analyze_with_owned_formals(
        func: &TirFunction,
        own_formals: bool,
    ) -> Option<Self> {
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
        let mut private = BTreeMap::new();
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
                    if own_formals || !params.contains(&op.operands[0]) {
                        private.insert(i64::try_from(slot).ok()?, op.results[0]);
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
                {
                    return None;
                }
            }
        }
        let nested = super::nested_frames::NestedFrames::read(func)?;
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
        let mut outgoing = HashMap::default();
        let aliases = (!private.is_empty())
            .then(|| crate::tir::passes::alias_analysis::build_alias_union_find(func));
        let private_roots: HashSet<_> = private.values().copied().collect();
        let is_home = |v| {
            aliases
                .as_ref()
                .is_some_and(|a| private_roots.contains(&a.root(v)))
        };
        while let Some(id) = queue.pop_front() {
            let block = func.blocks.get(&id)?;
            let mut state = incoming[&id].clone();
            let mut row = Vec::with_capacity(block.ops.len());
            for (at, op) in block.ops.iter().enumerate() {
                // A physical release or a read after retirement cannot use a
                // borrowed home view. A separately owned capture can survive.
                if !private.is_empty()
                    && ((!state.live && op.operands.iter().copied().any(is_home))
                        || super::activation::released_operands(op)
                            .into_iter()
                            .any(is_home))
                {
                    return None;
                }
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
                        || matches!(
                            nested.at(id, at),
                            Some(
                                super::nested_frames::Action::Open
                                    | super::nested_frames::Action::Finish
                            )
                        )
                        || op.runtime == Some(RuntimeFn::ExceptionTracePrepend)
                        || spliced_frame::may_observe_frames(op, &exact))
                    && op.opcode != OpCode::FrameSetLocation
                {
                    return None;
                }
                row.push(state.live.then(|| FramePoint {
                    location,
                    homes: state.homes.clone().unwrap_or_else(|| Rc::from([])),
                    joined_homes: state.homes.is_none(),
                    refresh_exit: !frozen_exit,
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
            if !private.is_empty() && !state.live {
                let mut uses_home = false;
                block.terminator.for_each_value(|v| uses_home |= is_home(v));
                if uses_home {
                    return None;
                }
            }
            if state.live
                && ((block.terminator.exits_activation() && !nested.dead_unwind(id))
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
            if !private.is_empty() {
                outgoing.insert(id, state.homes);
            }
            points.insert(id, row);
        }
        let mut joined_entries: Vec<_> = incoming
            .iter()
            .filter_map(|(&id, state)| state.homes.is_none().then_some(id))
            .collect();
        joined_entries.sort_unstable();
        Some(Self {
            points,
            nested,
            private,
            descending: shape.descending,
            joined_entries,
            outgoing,
        })
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
                    if point.refresh_exit {
                        assert_eq!(&*point.homes, &[(0, home)]);
                        assert_eq!(point.location, Some(4));
                    }
                    exits.push(point.refresh_exit);
                }
            }
        }
        exits.sort_unstable();
        assert_eq!(callbacks, 1);
        assert_eq!(exits, [false, true]);
    }

    #[test]
    fn callback_and_its_origin_share_a_bracket_and_the_exit_retires_the_identity() {
        use super::super::frame_splice::prepare_observable;

        let body = prepare_observable(&wrapper()).expect("observable wrapper");
        let call = body
            .blocks
            .values()
            .find_map(|block| {
                let at = block
                    .ops
                    .iter()
                    .position(|op| op.opcode == OpCode::Call && op.runtime.is_none())?;
                Some((block, at))
            })
            .expect("original callback");
        let (block, at) = call;
        let check = &block.ops[at + 1];
        assert_eq!(check.opcode, OpCode::CheckException);
        assert_eq!(check.frame_location(), None);
        assert_eq!(
            block.ops[at + 2].runtime,
            Some(RuntimeFn::SplicedFrameUnlink)
        );
        let labels = exception_label_to_block(&body);
        let cold = exception_transfer_target(check, &labels).unwrap();
        assert!(
            body.blocks[&cold]
                .ops
                .iter()
                .any(|op| op.runtime == Some(RuntimeFn::SplicedFrameOrigin))
        );
        assert!(
            body.blocks
                .values()
                .flat_map(|b| &b.ops)
                .any(|op| op.runtime == Some(RuntimeFn::SplicedFrameFinish))
        );
    }

    #[test]
    fn marked_origin_services_pending_before_reading_and_keeps_its_payload() {
        use super::super::frame_splice::prepare_observable;
        use crate::tir::passes::check_exception_elim::classify::RaiseFacts;
        use crate::tir::passes::exception_observation::{
            PostOperationObservation, post_operation_observation,
        };
        use crate::tir::types::TirType;
        use crate::tir::values::TirValue;

        let mut func = wrapper();
        let labels = exception_label_to_block(&func);
        let (&bid, at) = func
            .blocks
            .iter()
            .find_map(|(id, b)| {
                b.ops
                    .iter()
                    .position(|op| op.opcode == OpCode::Call)
                    .map(|at| (id, at))
            })
            .unwrap();
        // A callback-free operation can still fail. Its result is never a
        // substitute for a payload carried on its exception edge.
        let op = &func.blocks[&bid].ops[at];
        let arg = op.operands[0];
        let result = op.results[0];
        let other = func.fresh_value();
        let source = func.blocks.get_mut(&bid).unwrap();
        let mut replacement = TirOp::new(OpCode::ConstStr, vec![], vec![result]);
        replacement
            .attrs
            .insert("s_value".into(), AttrValue::Str("payload".into()));
        source.ops[at] = replacement;
        let check = &mut source.ops[at + 1];
        check.mark_async_work_poll();
        check.operands.push(arg);
        let old_target = exception_transfer_target(check, &labels).unwrap();
        // Give this particular edge its own landing before the shared exit.
        let landing = func.fresh_block();
        let label = crate::tir::clone_support::LabelAllocator::after_labels(
            crate::tir::clone_support::function_label_ids(&func),
        )
        .fresh();
        func.label_id_map.insert(landing.0, label);
        func.blocks.get_mut(&bid).unwrap().ops[at + 1]
            .attrs
            .insert("value".into(), AttrValue::Int(label));
        func.blocks.insert(
            landing,
            crate::tir::blocks::TirBlock {
                id: landing,
                args: vec![TirValue {
                    id: other,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: old_target,
                    args: vec![],
                },
            },
        );
        let body = prepare_observable(&func).unwrap();
        crate::tir::verify::verify_function(&body).unwrap();
        let gate = &body.blocks[&bid];
        let produced = gate
            .ops
            .iter()
            .position(|op| op.opcode == OpCode::ConstStr)
            .unwrap();
        assert_eq!(gate.ops[produced + 1].opcode, OpCode::AsyncWorkPending);
        assert!(!gate.ops.iter().any(spliced_frame::is_spliced_frame_call));
        let preds = crate::tir::dominators::build_pred_map(&body);
        let Some(PostOperationObservation::PolledOrigin { service, fast, .. }) =
            post_operation_observation(&body, bid, produced, &preds, &RaiseFacts::of(&body))
        else {
            panic!("missing post-operation poll pair")
        };
        let labels = exception_label_to_block(&body);
        for ((arm, idx), pending) in [(service, true), (fast, false)] {
            let ops = &body.blocks[&arm].ops;
            let check = &ops[idx];
            assert_eq!(check.is_async_work_poll(), pending);
            assert_eq!(check.exception_edge_kind(), Some(ExceptionEdgeKind::Origin));
            assert_eq!(check.exception_payload(), [arg]);
            assert_eq!(
                ops[..idx]
                    .iter()
                    .any(|op| op.runtime == Some(RuntimeFn::SplicedFrameLink)),
                pending
            );
            let cold = &body.blocks[&exception_transfer_target(check, &labels).unwrap()];
            assert!(
                matches!(&cold.terminator,Terminator::Branch{target,args} if *target==landing && args==&[cold.args[0].id])
            );
        }
    }

    #[test]
    fn private_checked_result_clears_after_retirement_on_bound_and_unbound_paths() {
        use super::super::frame_splice::prepare_observable;
        use crate::tir::target_info::TargetInfo;

        let ir: crate::ir::FunctionIR = serde_json::from_value(serde_json::json!({
            "return_abi":"void", "name":"setter", "execution_context":"local",
            "params":["arg"], "param_types":["Any"], "ops":[
                {"kind":"trace_enter_slot","s_value":"setter","value":2},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"frame_home_store","value":0,"args":["arg"],"out":"formal"},
                {"kind":"frame_set_location","value":1},
                {"kind":"call","s_value":"checked","args":["formal"],"out":"result"},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"frame_home_store","value":1,"args":["result"],"out":"local"},
                {"kind":"frame_set_location","value":2},
                {"kind":"call","s_value":"observer","args":["local"],"out":"unused"},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"trace_exit"},{"kind":"ret_void"},
                {"kind":"label","value":9},{"kind":"trace_exit"},{"kind":"ret_void"}
            ]
        }))
        .unwrap();
        let mut func = crate::tir::lower_from_simple::lower_to_tir(&ir);
        for op in func.blocks.values_mut().flat_map(|b| &mut b.ops) {
            if op.origin == Some(OpKind::FrameHomeStore) {
                op.mark_nothrow_proof();
            }
        }
        let store = func
            .blocks
            .values()
            .flat_map(|b| &b.ops)
            .find(|op| {
                op.origin == Some(OpKind::FrameHomeStore)
                    && frame_location_placement::home_slot(op) == Some(1)
            })
            .unwrap();
        let (source, local) = (store.operands[0], store.results[0]);
        let mut body = prepare_observable(&func).expect("private call result");
        crate::tir::verify::verify_function(&body).unwrap();
        let bound = body
            .blocks
            .values()
            .find(|b| b.args.iter().any(|arg| arg.id == local))
            .unwrap()
            .id;
        assert!(
            body.blocks
                .values()
                .any(|b| b.terminator.first_edge_args_to(bound) == Some(&[source][..]))
        );
        let target = exception_label_to_block(&func)[&9];
        let joined = body.blocks[&target]
            .args
            .last()
            .expect("mixed home ownership")
            .id;
        let labels = exception_label_to_block(&body);
        let mut inputs = Vec::new();
        for op in body
            .blocks
            .values()
            .flat_map(|b| &b.ops)
            .filter(|op| op.opcode == OpCode::CheckException)
        {
            let cold = &body.blocks[&exception_transfer_target(op, &labels).unwrap()];
            if cold.terminator.has_successor(target) {
                assert_eq!(
                    cold.terminator.first_edge_args_to(target).unwrap(),
                    &[cold.args.last().unwrap().id]
                );
                inputs.push(*op.exception_payload().last().unwrap());
            }
        }
        let absent = body.blocks[&body.entry_block]
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::ConstNone)
            .unwrap()
            .results[0];
        inputs.sort_unstable();
        let mut expected = [absent, local];
        expected.sort_unstable();
        assert_eq!(inputs, expected);
        let clears = [local, joined].map(|v| {
            let cleared = body
                .blocks
                .values()
                .find(|b| {
                    b.ops
                        .iter()
                        .any(|op| op.opcode == OpCode::DecRef && op.operands == [v])
                })
                .unwrap();
            assert!(body.blocks.values().any(|b| {
                b.terminator.has_successor(cleared.id)
                    && b.ops
                        .iter()
                        .any(|op| op.runtime == Some(RuntimeFn::SplicedFrameFinish))
            }));
            cleared.id
        });
        crate::tir::pass_manager::build_drop_pipeline(TargetInfo::native_release_fast())
            .run(&mut body);
        crate::tir::verify::verify_function(&body).unwrap();
        assert!(
            !body
                .blocks
                .values()
                .flat_map(|b| &b.ops)
                .any(|op| op.opcode == OpCode::IncRef && op.operands == [source]),
            "the dying call result moves into the home"
        );
        for (value, at) in [local, joined].into_iter().zip(clears) {
            assert!(
                body.blocks[&at]
                    .ops
                    .iter()
                    .any(|op| op.opcode == OpCode::DecRef && op.operands == [value])
            );
        }
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
