//! Virtualize a proven, singly entered synchronous activation. The caller
//! binds the final synthetic argument to the invoked function object and
//! keeps each reference-bearing parameter alive independently of its home.

use molt_ir::runtime_fn_generated::RuntimeFn;
use molt_ir::{ExecutionContextPolicy, OpKind, ParameterCustody};

use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::clone_support::{LabelAllocator, function_label_ids};
use crate::tir::dominators::{
    CfgEdgePolicy, exception_label_to_block, exception_transfer_target, reachable_blocks_with,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrValue, FRAME_LOCATION_ATTR, OpCode, TirOp};
use crate::tir::passes::spliced_frame::{self, Binding, KeySpace, Level};
use crate::tir::types::TirType;
use crate::tir::values::TirValue;

use super::nested_frames::Action;
use super::sync_frame::SyncFrame;

/// The caller supplies already-prepared parameter custody. A normal return
/// rejoins the authored call-site observation. Every reachable unwind has
/// already captured the physical caller and must rejoin an Unwind observer.
#[cfg(test)]
pub(super) fn prepare_observable(callee: &TirFunction) -> Option<TirFunction> {
    let frame = SyncFrame::analyze(callee)?;
    prepare_with_frame(callee, frame, None)
}

pub(super) fn prepare_with_frame(
    callee: &TirFunction,
    frame: SyncFrame,
    pending_field: Option<(super::packed_read::Site, crate::tir::values::ValueId)>,
) -> Option<TirFunction> {
    let mut field_cut_reached = false;
    let normal = reachable_blocks_with(callee, CfgEdgePolicy::TerminatorOnly);
    if normal.iter().any(|id| {
        matches!(callee.blocks[id].terminator, Terminator::Unwind) && !frame.nested.dead_unwind(*id)
    }) {
        return None;
    }
    let entry_check = callee.blocks[&callee.entry_block].ops.get(1)?;
    if entry_check.opcode != OpCode::CheckException
        || entry_check.is_async_work_poll()
        || entry_check.has_checked_invocation_binding()
        || !entry_check.operands.is_empty()
    {
        return None;
    }
    let mut body = callee.clone();
    let mut custody: Vec<_> = (0..body.param_types.len())
        .map(|p| body.parameter_custody(p))
        .collect();
    custody.push(ParameterCustody::Borrowed);
    let function = body.fresh_value();
    body.blocks.get_mut(&body.entry_block)?.args.push(TirValue {
        id: function,
        ty: TirType::DynBox,
    });
    body.param_types.push(TirType::DynBox);
    body.param_names.push(String::new());
    if !body.exact_param_types.is_empty() {
        body.exact_param_types.push(TirType::DynBox);
    }
    body.set_parameter_custody(&custody);
    let identity = body.fresh_value();
    body.value_types.insert(identity, TirType::I64);
    let present = body.fresh_value();
    body.value_types.insert(present, TirType::Bool);
    let exact = crate::tir::type_refine::extract_exact_scalar_map(callee);
    let labels = exception_label_to_block(callee);
    let mut fresh_labels = LabelAllocator::after_labels(function_label_ids(callee));
    // Keep the checked invocation and the poll/unlink pair adjacent. A count
    // definition in the activation entry dominates both its success exit and
    // its exceptional landing without adding work between check and unlink.
    let mut depth_constants = Vec::new();
    let mut private_clears = frame.prepare_private(callee, &mut body, &mut depth_constants)?;
    let close_counts: molt_ir::deterministic_hash::HashMap<_, _> = frame
        .nested
        .exit_depths()
        .into_iter()
        .map(|depth| {
            (
                depth,
                spliced_frame::const_int(&mut body, depth, &mut depth_constants),
            )
        })
        .collect();
    let mut ids: Vec<_> = callee.blocks.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let source = body.blocks.get_mut(&id)?;
        let ops = std::mem::take(&mut source.ops);
        let term = source.terminator.clone();
        if frame.nested.dead_unwind(id) {
            body.blocks.get_mut(&id)?.terminator = Terminator::Unreachable;
        }
        let mut out = Vec::new();
        let mut retirement = None;
        let mut polls = Vec::new();
        let mut index = 0;
        while index < ops.len() {
            let op = &ops[index];
            let at = index;
            index += 1;
            if id == callee.entry_block && at == 0 {
                out.push(TirOp::new(OpCode::SplicedFrameSlot, vec![], vec![identity]));
                let mut truth = TirOp::new(OpCode::ConstBool, vec![], vec![present]);
                truth.attrs.insert("value".into(), AttrValue::Bool(true));
                out.push(truth);
                out.append(&mut depth_constants);
                // The compiled frame push and its failure check form one event.
                index += 1;
                continue;
            }
            if op.opcode == OpCode::FrameSetLocation || op.origin == Some(OpKind::Line) {
                continue;
            }
            if op.origin == Some(OpKind::FrameHomeStore) {
                if frame.owns_home(op) {
                    // Split this marker after poll and retirement cuts. Its
                    // successor argument adopts the original store operand.
                    out.push(op.clone());
                } else {
                    let mut copy =
                        TirOp::new(OpCode::Copy, op.operands.clone(), op.results.clone());
                    copy.source_span = op.source_span;
                    out.push(copy);
                }
                continue;
            }
            let Some(point) = frame.point(id, at) else {
                // A structurally retained dead block cannot publish its old
                // frame. Post-retirement code belongs to the physical caller.
                if op.origin != Some(OpKind::TraceExit) {
                    out.push(op.clone());
                }
                continue;
            };
            let level = |point: &super::sync_frame::FramePoint| Level {
                function,
                identity,
                key_space: KeySpace::CodeSlot,
                location: point.location,
                bindings: point
                    .homes
                    .iter()
                    .map(|&(key, value)| Binding {
                        key,
                        value,
                        present,
                    })
                    .collect(),
            };
            if let Some(action) = frame.nested.at(id, at) {
                match action {
                    Action::Open => {
                        spliced_frame::link_level(
                            &mut body,
                            &level(point),
                            op.source_span,
                            &mut out,
                        );
                        out.push(op.clone());
                    }
                    Action::Close { operand, levels } => {
                        let mut close = op.clone();
                        close.operands[operand] = close_counts[&levels];
                        out.push(close);
                    }
                    Action::Finish => {
                        let (enter, leave) =
                            spliced_frame::bracket_ops(&mut body, &[level(point)], op.source_span);
                        out.extend(enter);
                        out.push(op.clone());
                        out.extend(leave);
                    }
                    Action::Linked | Action::CapturingCheck => {
                        let mut nested = op.clone();
                        if nested.opcode == OpCode::CheckException {
                            nested.attrs.remove(FRAME_LOCATION_ATTR);
                        }
                        out.push(nested);
                    }
                }
                continue;
            }
            if op.origin == Some(OpKind::TraceExit) {
                retirement = Some((out.len(), point.refresh_exit.then(|| level(point))));
                // The eager record unlinks before its home releases, so a
                // finalizer runs under the caller on either retirement arm.
                for value in private_clears.remove(&(id, at)).unwrap_or_default() {
                    let mut clear = TirOp::new(OpCode::DecRef, vec![value], vec![]);
                    clear.source_span = op.source_span;
                    out.push(clear);
                }
                continue;
            }
            if op.opcode == OpCode::Raise || op.runtime == Some(RuntimeFn::ExceptionTracePrepend) {
                return None;
            }
            let callback = spliced_frame::may_observe_frames(op, &exact);
            let following = ops
                .get(index)
                .filter(|next| next.opcode == OpCode::CheckException);
            if frame.nested.at(id, index) == Some(Action::CapturingCheck) && callback {
                return None;
            }
            let following = following.filter(|_| frame.nested.at(id, index).is_none());
            let check = if op.opcode == OpCode::CheckException {
                Some((at, op))
            } else {
                following.map(|check| (index, check))
            };
            if let Some((check_at, check)) = check {
                let origin = check.exception_edge_kind()? == ExceptionEdgeKind::Origin;
                let cp = frame.point(id, check_at)?;
                if !origin && !cp.published {
                    return None;
                }
                if check.has_checked_invocation_binding()
                    && (check_at == at || !op.results.contains(&check.checked_invocation_source()?))
                {
                    return None;
                }

                // A live record around both the call and its check can only
                // present one location. Keep the checked producer adjacent.
                let linked = (check_at != at && callback) || check.is_async_work_poll();
                if callback && check_at != at && cp.location != point.location {
                    return None;
                }
                let state = level(cp);
                let (before, after) = if linked {
                    spliced_frame::bracket_ops(
                        &mut body,
                        std::slice::from_ref(&state),
                        check.source_span,
                    )
                } else {
                    (vec![], vec![])
                };
                let split = origin
                    && check.is_async_work_poll()
                    && !check.has_checked_invocation_binding()
                    && (check_at == at || !callback);
                let held_field = if let Some((site, field)) = pending_field
                    && site.block == id
                    && site.index == check_at
                {
                    if !split || field_cut_reached {
                        return None;
                    }
                    field_cut_reached = true;
                    let held = body.fresh_value();
                    body.value_types.insert(held, TirType::DynBox);
                    Some((field, held))
                } else {
                    None
                };
                let mut before = before;
                if !split {
                    out.append(&mut before);
                }
                if check_at != at {
                    out.push(op.clone());
                    index += 1;
                }
                let target = exception_transfer_target(check, &labels)?;
                let mut observation = |body: &mut TirFunction, linked| -> Option<TirOp> {
                    let target_args = body.blocks[&target].args.clone();
                    if target_args.len() != check.exception_payload().len() {
                        return None;
                    }
                    let args: Vec<_> = target_args
                        .iter()
                        .map(|arg| {
                            let id = body.fresh_value();
                            body.value_types.insert(id, arg.ty.clone());
                            TirValue {
                                id,
                                ty: arg.ty.clone(),
                            }
                        })
                        .collect();
                    let payload = args.iter().map(|arg| arg.id).collect();
                    let mut cold = if linked {
                        spliced_frame::linked_origin_block(
                            body,
                            1,
                            origin,
                            target,
                            payload,
                            check.source_span,
                        )
                    } else if !origin {
                        TirBlock {
                            id: body.fresh_block(),
                            args: vec![],
                            ops: vec![],
                            terminator: Terminator::Branch {
                                target,
                                args: payload,
                            },
                        }
                    } else {
                        spliced_frame::origin_cold_block(
                            body,
                            std::slice::from_ref(&state),
                            target,
                            payload,
                            check.source_span,
                        )
                    };
                    if linked && let Some((_, held)) = held_field {
                        // The runtime has captured this Origin and popped its
                        // record. A handler may have replaced the receiver's
                        // field and promoted its storage. Its last release
                        // must still see the getter and its original location.
                        let (enter, leave) = spliced_frame::bracket_ops(
                            body,
                            std::slice::from_ref(&state),
                            check.source_span,
                        );
                        cold.ops.extend(enter);
                        cold.ops
                            .push(TirOp::new(OpCode::DelBoundary, vec![held], vec![]));
                        cold.ops.extend(leave);
                    }
                    cold.args = args;
                    let label = fresh_labels.fresh();
                    body.label_id_map.insert(cold.id.0, label);
                    body.blocks.insert(cold.id, cold);
                    let mut cloned = check.clone();
                    cloned.attrs.remove(FRAME_LOCATION_ATTR);
                    cloned.attrs.insert("value".into(), AttrValue::Int(label));
                    Some(cloned)
                };
                let cloned = observation(&mut body, linked)?;
                if split {
                    let mut fast = observation(&mut body, false)?;
                    fast.clear_async_work_poll();
                    let mut service = before;
                    if let Some((field, held)) = held_field {
                        service.insert(
                            0,
                            crate::tir::passes::ownership_lattice_min::owned_alias(
                                field,
                                held,
                                check.source_span,
                            ),
                        );
                    }
                    service.push(cloned.clone());
                    if let Some((_, held)) = held_field {
                        service.push(TirOp::new(OpCode::DelBoundary, vec![held], vec![]));
                    }
                    service.extend(after);
                    polls.push((out.len(), service, vec![fast]));
                    out.push(cloned);
                } else {
                    out.push(cloned);
                    out.extend(after);
                }
            } else if callback {
                let (enter, exit) =
                    spliced_frame::bracket_ops(&mut body, &[level(point)], op.source_span);
                out.extend(enter);
                out.push(op.clone());
                out.extend(exit);
            } else {
                out.push(op.clone());
            }
        }
        if let Some((at, snapshot)) = retirement {
            let end = body.fresh_block();
            let tail = out.split_off(at);

            body.blocks.insert(
                end,
                TirBlock {
                    id: end,
                    args: vec![],
                    ops: tail,
                    terminator: term,
                },
            );
            body.move_loop_terminator(id, end);
            let source = body.blocks.get_mut(&id)?;
            source.ops = out;
            source.terminator = Terminator::Branch {
                target: end,
                args: vec![],
            };
            spliced_frame::retire_if_observed(&mut body, id, identity, snapshot.as_ref());
        } else {
            body.blocks.get_mut(&id)?.ops = out;
        }
        for (at, service, fast) in polls.into_iter().rev() {
            crate::tir::passes::async_work_poll::split_pending_work(
                &mut body, id, at, service, fast,
            );
        }
    }
    // Finish each private store as an SSA ownership transfer. DropInsertion
    // moves a dead owned source, and retains a live or borrowed one, exactly
    // as it did for the eager store's consuming operand. A transparent copy or
    // an unconditional retain would lose the source's lexical transfer.
    if frame.has_private_homes() {
        let mut stores: Vec<_> = body
            .blocks
            .iter()
            .flat_map(|(&id, block)| {
                block.ops.iter().enumerate().filter_map(move |(at, op)| {
                    (op.origin == Some(OpKind::FrameHomeStore)).then_some((id, at))
                })
            })
            .collect();
        stores.sort_unstable();
        for (id, at) in stores.into_iter().rev() {
            let next = body.fresh_block();
            let block = body.blocks.get_mut(&id)?;
            let tail = block.ops.split_off(at + 1);
            let store = block.ops.pop()?;
            let home = store.results[0];
            let ty = body
                .value_types
                .get(&home)
                .cloned()
                .unwrap_or(TirType::DynBox);
            let term = std::mem::replace(
                &mut block.terminator,
                Terminator::Branch {
                    target: next,
                    args: store.operands,
                },
            );
            body.blocks.insert(
                next,
                TirBlock {
                    id: next,
                    args: vec![TirValue { id: home, ty }],
                    ops: tail,
                    terminator: term,
                },
            );
            body.move_loop_terminator(id, next);
        }
    }
    if pending_field.is_some() && !field_cut_reached {
        return None;
    }
    body.execution_context = ExecutionContextPolicy::None;
    Some(body)
}
