//! Project a checked scalar forwarder without linking a frame on its clean
//! edge. The ordinary invocation owns every miss and all real handlers.

use molt_ir::deterministic_hash::HashSet;
use molt_ir::frame_entry::{FrameEntryKind, FramePrefix};
use molt_ir::{ExecutionContextPolicy, OpKind};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::call_graph::StaticCallSite;
use crate::tir::call_targets::static_call;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::{frame_location_placement, value_identity};
use crate::tir::type_refine::ExactScalarPlan;
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::activation::Activations;
use super::packed_splice::{ordinary_call, test_block};

pub(super) struct CheckedScalar {
    formal: usize,
    owner: String,
    reservation: FramePrefix,
}

#[derive(Clone, Copy)]
enum Scope {
    Entry,
    Baseline(ValueId),
    Saved { baseline: ValueId, depth: ValueId },
    Pushed { baseline: ValueId, depth: ValueId },
    Open { baseline: ValueId, depth: ValueId },
    Retired { baseline: ValueId, depth: ValueId },
    Restored,
    Exited,
}

impl CheckedScalar {
    pub(super) fn summarize(body: &TirFunction) -> Option<Self> {
        if body.execution_context != ExecutionContextPolicy::Local
            || body.return_type != TirType::I64
            || !body.has_exception_handlers()
            || body.frame_locals_exposed()
            || body.has_closure_parameter()
            || body.has_state_machine()
            || body.is_codegen_partition()
        {
            return None;
        }
        let formal = ExactScalarPlan::new(body).forwarded_return_parameter()?;
        let entry = &body.blocks[&body.entry_block];
        let mut inputs = HashSet::default();
        for (i, arg) in entry.args.iter().enumerate() {
            // Nonforwarded formals may not acquire a potentially finalizing
            // Python owner or require allocation to enter the omitted frame.
            let exact = if i == formal {
                TirType::I64
            } else {
                TirType::Str
            };
            if body.exact_param_types.get(i) != Some(&exact) {
                return None;
            }
            inputs.insert(arg.id);
        }
        let (head, _) = entry.ops.split_first()?;
        if head.origin != Some(OpKind::TraceEnterSlot)
            || !head.operands.is_empty()
            || !head.results.is_empty()
            || !matches!(head.attrs.get("value"), Some(AttrValue::Int(_)))
        {
            return None;
        }
        let (Some(AttrValue::Int(encoded)), Some(AttrValue::Str(owner))) =
            (head.attrs.get("value"), head.attrs.get("s_value"))
        else {
            return None;
        };
        let shape = FrameEntryKind::from_entry_value(*encoded).shape()?;
        if shape.entry_value() != *encoded || owner.is_empty() {
            return None;
        }
        let mut scope = Scope::Entry;
        let mut homes = HashSet::default();
        let mut visited = HashSet::default();
        let mut serviced = false;
        let mut id = body.entry_block;
        while visited.insert(id) {
            let block = &body.blocks[&id];
            for op in block.ops.iter().skip(usize::from(id == body.entry_block)) {
                if op.runtime.is_some() || op.callee.is_some() || !op.has_valid_shape() {
                    return None;
                }
                match (
                    &op.opcode,
                    op.origin,
                    op.operands.as_slice(),
                    op.results.as_slice(),
                ) {
                    (OpCode::Copy, Some(OpKind::ExceptionStackEnter), [], [baseline])
                        if matches!(scope, Scope::Entry) =>
                    {
                        scope = Scope::Baseline(*baseline);
                    }
                    (OpCode::Copy, Some(OpKind::ExceptionStackDepth), [], [depth])
                        if let Scope::Baseline(baseline) = scope =>
                    {
                        scope = Scope::Saved {
                            baseline,
                            depth: *depth,
                        };
                    }
                    (OpCode::Copy, Some(OpKind::FrameHomeStore), [source], [view])
                        if matches!(scope, Scope::Saved { .. }) && inputs.contains(source) =>
                    {
                        let slot = frame_location_placement::home_slot(op)?;
                        if slot >= shape.homes as usize || !homes.insert(slot) {
                            return None;
                        }
                        inputs.insert(*view);
                    }
                    (OpCode::Copy, Some(OpKind::ExceptionPush), [], [])
                        if let Scope::Saved { baseline, depth } = scope =>
                    {
                        scope = Scope::Pushed { baseline, depth };
                    }
                    (OpCode::TryStart, _, [], [])
                        if let Scope::Pushed { baseline, depth } = scope =>
                    {
                        scope = Scope::Open { baseline, depth };
                    }
                    (OpCode::Copy, Some(OpKind::ExceptionPop), [], [])
                        if let Scope::Open { baseline, depth } = scope =>
                    {
                        scope = Scope::Retired { baseline, depth };
                    }
                    (OpCode::Copy, Some(OpKind::ExceptionStackSetDepth), [target], []) if matches!(scope, Scope::Retired { depth, .. } if depth == *target) =>
                        {}
                    (OpCode::Copy, Some(OpKind::ExceptionStackExit), [target], []) if matches!(scope, Scope::Retired { baseline, .. } if baseline == *target) =>
                    {
                        scope = Scope::Restored;
                    }
                    (OpCode::Copy, Some(OpKind::TraceExit), [], [])
                        if matches!(scope, Scope::Restored) =>
                    {
                        scope = Scope::Exited;
                    }
                    (OpCode::CheckException, _, _, []) if !matches!(scope, Scope::Exited) => {
                        if op.is_async_work_poll() {
                            if serviced || !matches!(scope, Scope::Open { .. }) {
                                return None;
                            }
                            serviced = true;
                        }
                    }
                    (OpCode::FrameSetLocation, _, [], [])
                    | (OpCode::Copy, Some(OpKind::Line), [], [])
                        if !matches!(scope, Scope::Exited) => {}
                    (OpCode::Copy, kind, [from], [to])
                        if !matches!(scope, Scope::Exited)
                            && inputs.contains(from)
                            && (kind == Some(OpKind::BindingAlias)
                                || value_identity::copy_value_source(op) == Some(*from)) =>
                    {
                        inputs.insert(*to);
                    }
                    _ => return None,
                }
            }
            match &block.terminator {
                Terminator::Branch { target, args } if args.is_empty() => id = *target,
                Terminator::Return { values }
                    if matches!(scope, Scope::Exited) && values.len() == 1 && serviced =>
                {
                    return Some(Self {
                        formal,
                        owner: owner.clone(),
                        reservation: FramePrefix::default().enter(shape)?,
                    });
                }
                Terminator::Branch { .. }
                | Terminator::CondBranch { .. }
                | Terminator::Switch { .. }
                | Terminator::StateDispatch { .. }
                | Terminator::Return { .. }
                | Terminator::Unwind
                | Terminator::Unreachable => return None,
            }
        }
        None
    }

    pub(super) fn splice(
        &self,
        caller: &mut TirFunction,
        callee: &TirFunction,
        site: &StaticCallSite,
        activations: &mut Activations,
    ) -> Option<BlockId> {
        if caller.execution_context != ExecutionContextPolicy::Local
            || caller.frame_locals_exposed()
        {
            return None;
        }
        let block = caller.blocks.get(&site.block)?;
        let call = block.ops.get(site.op_index)?.clone();
        let [joined] = call.results[..] else {
            return None;
        };
        let binding = static_call(&call)?;
        let captured = binding.callable?;
        let args = binding.bind(callee)?;
        let actual = *args.positional.get(self.formal)?;
        let observer = block.ops.get(site.op_index + 1)?;
        if observer.opcode != OpCode::CheckException
            || observer.exception_edge_kind() != Some(ExceptionEdgeKind::Origin)
            || observer.has_checked_invocation_binding()
        {
            return None;
        }
        let _ = activations.bindings(caller, callee, site, args.positional, 0);
        if !activations.checked_result_is_success_only(joined)
            || !(activations.immutable_cell_holds(captured)
                || activations.caller_keeps_alive(site, captured))
        {
            return None;
        }
        let ty = caller.value_types.get(&joined)?.clone();
        let mut original = caller.blocks.remove(&site.block)?;
        let rest = original.ops.split_off(site.op_index + 1);
        original.ops.pop();
        let ready = caller.fresh_block();
        caller.blocks.insert(
            ready,
            TirBlock {
                id: ready,
                args: vec![TirValue { id: joined, ty }],
                ops: rest,
                terminator: original.terminator,
            },
        );
        let raw = caller.fresh_value();
        caller.value_types.insert(raw, TirType::I64);
        let hit = caller.fresh_block();
        caller.blocks.insert(
            hit,
            TirBlock {
                id: hit,
                args: vec![],
                ops: vec![TirOp::new(OpCode::UnboxInlineInt, vec![actual], vec![raw])],
                terminator: Terminator::Branch {
                    target: ready,
                    args: vec![raw],
                },
            },
        );
        let miss = caller.fresh_block();
        let pending = caller.fresh_value();
        let service = test_block(
            caller,
            vec![],
            TirOp::new(OpCode::AsyncWorkPending, vec![], vec![pending]),
            miss,
            hit,
        );
        let admitted = caller.fresh_value();
        let mut frame = TirOp::new(OpCode::FrameEntryAdmits, vec![captured], vec![admitted]);
        frame
            .attrs
            .insert("s_value".into(), AttrValue::Str(self.owner.clone()));
        frame.attrs.extend(
            self.reservation
                .reservation_attrs()
                .map(|(key, v)| (key.into(), v)),
        );
        FramePrefix::require_virtual_level(&mut frame.attrs);
        let entry = test_block(caller, vec![], frame, service, miss);
        let inline = caller.fresh_value();
        let test = TirOp::new(OpCode::IsInlineInt, vec![actual], vec![inline]);
        let entry = test_block(caller, vec![], test, entry, miss);
        original.terminator = Terminator::Branch {
            target: entry,
            args: vec![],
        };
        caller.blocks.insert(site.block, original);
        caller.move_loop_terminator(site.block, ready);
        ordinary_call(caller, miss, ready, call);
        Some(ready)
    }
}
