//! Continue after a committed packed read at the original observation sites.

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::clone_support::LabelAllocator;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::ownership_lattice_min::owned_alias;
use crate::tir::passes::spliced_frame::{self, Level};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::activation::Activations;
use super::packed_splice::{ordinary_call, test_block};
use super::packed_wrapper::{Check, PackedWrapper};
use super::packed_wrapper_prefix::{Prefix, ReadState, Snapshot};
use crate::tir::call_graph::StaticCallSite;

fn insert(func: &mut TirFunction, ops: Vec<TirOp>, terminator: Terminator) -> BlockId {
    let id = func.fresh_block();
    func.blocks.insert(
        id,
        TirBlock {
            id,
            args: vec![],
            ops,
            terminator,
        },
    );
    id
}

fn jump(target: BlockId) -> Terminator {
    Terminator::Branch {
        target,
        args: vec![],
    }
}

fn word(func: &mut TirFunction, ty: TirType) -> ValueId {
    let id = func.fresh_value();
    func.value_types.insert(id, ty);
    id
}

fn push(func: &mut TirFunction, ops: &mut Vec<TirOp>) -> ValueId {
    let id = word(func, TirType::I64);
    ops.push(TirOp::new(OpCode::RecursionCount, vec![], vec![]));
    ops.push(TirOp::new(OpCode::SplicedFrameSlot, vec![], vec![id]));
    id
}

fn snapshot(check: &Check) -> Snapshot<'_> {
    Snapshot {
        location: check.location,
        homes: &check.homes,
    }
}

fn pop() -> TirOp {
    TirOp::new(OpCode::RecursionRelease, vec![], vec![])
}

fn service_if_pending(func: &mut TirFunction, service: BlockId, next: BlockId) -> BlockId {
    let flag = word(func, TirType::Bool);
    insert(
        func,
        vec![TirOp::new(OpCode::AsyncWorkPending, vec![], vec![flag])],
        Terminator::CondBranch {
            cond: flag,
            then_block: service,
            then_args: vec![],
            else_block: next,
            else_args: vec![],
        },
    )
}

struct Resume<'a, 'p> {
    func: &'a mut TirFunction,
    state: ReadState<'p>,
    labels: LabelAllocator,
    caller_location: Option<i64>,
    span: Option<(u32, u32)>,
}

struct Poll {
    next: BlockId,
    failed: BlockId,
    origin: ExceptionEdgeKind,
    release: ValueId,
}

#[derive(Clone, Copy)]
struct WrapperScope {
    identity: ValueId,
    present: ValueId,
    callable: ValueId,
    method: ValueId,
}

impl Resume<'_, '_> {
    fn present(&mut self, ops: &mut Vec<TirOp>) -> ValueId {
        if let Some(location) = self.caller_location {
            let mut op = TirOp::new(OpCode::FrameSetLocation, vec![], vec![]);
            op.attrs.insert("value".into(), AttrValue::Int(location));
            ops.push(op);
        }
        let id = word(self.func, TirType::Bool);
        let mut op = TirOp::new(OpCode::ConstBool, vec![], vec![id]);
        op.attrs.insert("value".into(), AttrValue::Bool(true));
        ops.push(op);
        id
    }

    fn hold(&mut self, ops: &mut Vec<TirOp>, value: ValueId) -> ValueId {
        let held = word(self.func, TirType::DynBox);
        ops.push(owned_alias(value, held, self.span));
        held
    }

    fn release(held: ValueId) -> TirOp {
        TirOp::new(OpCode::DelBoundary, vec![held], vec![])
    }

    fn poll(&mut self, mut ops: Vec<TirOp>, levels: &[Level], poll: Poll) -> BlockId {
        let mut failed = spliced_frame::linked_origin_block(
            self.func,
            levels.len() as i64,
            poll.origin == ExceptionEdgeKind::Origin,
            poll.failed,
            vec![],
            self.span,
        );
        // A callback can substitute the field. Capture the exception before
        // releasing the original binding under its original physical frame.
        let (enter, exit) = spliced_frame::bracket_ops(self.func, levels, self.span);
        failed.ops.extend(enter);
        failed.ops.push(Self::release(poll.release));
        failed.ops.extend(exit);
        let label = self.labels.fresh();
        self.func.label_id_map.insert(failed.id.0, label);
        self.func.blocks.insert(failed.id, failed);
        let (enter, exit) = spliced_frame::bracket_ops(self.func, levels, self.span);
        ops.extend(enter);
        let mut check = TirOp::new(OpCode::CheckException, vec![], vec![]);
        check.source_span = self.span;
        check.attrs.insert("value".into(), AttrValue::Int(label));
        check.set_exception_edge_kind(poll.origin);
        check.mark_async_work_poll();
        ops.push(check);
        ops.push(Self::release(poll.release));
        ops.extend(exit);
        insert(self.func, ops, jump(poll.next))
    }

    fn retire(
        &mut self,
        identity: ValueId,
        exit: Option<&Level>,
        callers: &[Level],
        callable: ValueId,
        next: BlockId,
    ) -> BlockId {
        let release = insert(self.func, vec![Self::release(callable), pop()], jump(next));
        let retire = insert(self.func, vec![], jump(release));
        spliced_frame::retire_if_observed_under(self.func, retire, identity, exit, callers);
        retire
    }

    fn wrapper_service(
        &mut self,
        ops: Vec<TirOp>,
        scope: WrapperScope,
        ready: BlockId,
        outward: BlockId,
        origin: ExceptionEdgeKind,
    ) -> (BlockId, BlockId) {
        let WrapperScope {
            identity,
            present,
            callable,
            method,
        } = scope;
        let frame = self
            .state
            .wrapper
            .level(snapshot(self.state.wrapper_check), identity, present);
        let final_frame = self
            .state
            .wrapper
            .level(self.state.wrapper_retire, identity, present);
        let failed = self.retire(identity, None, &[], callable, outward);
        let next = if origin == ExceptionEdgeKind::Unwind {
            failed
        } else {
            self.retire(identity, Some(&final_frame), &[], callable, ready)
        };
        let service = self.poll(
            ops,
            &[frame],
            Poll {
                next,
                failed,
                origin,
                release: method,
            },
        );
        (service, next)
    }

    fn emit(mut self, field: ValueId, raw: ValueId, ready: BlockId, mut outward: TirOp) -> BlockId {
        let ready = insert(
            self.func,
            vec![],
            Terminator::Branch {
                target: ready,
                args: vec![raw],
            },
        );
        outward.set_exception_edge_kind(ExceptionEdgeKind::Unwind);
        let outward = insert(self.func, vec![outward], Terminator::Unwind);

        let mut ops = Vec::new();
        let wrapper_id = push(self.func, &mut ops);
        let present = self.present(&mut ops);
        let method = self.hold(&mut ops, self.state.leaf.function);
        let scope = WrapperScope {
            identity: wrapper_id,
            present,
            callable: self.hold(&mut ops, self.state.wrapper.function),
            method,
        };
        let (wrapper_service, _) =
            self.wrapper_service(ops, scope, ready, outward, ExceptionEdgeKind::Origin);
        let without_leaf = service_if_pending(self.func, wrapper_service, ready);

        let mut ops = Vec::new();
        let wrapper_id = push(self.func, &mut ops);
        let leaf_id = push(self.func, &mut ops);
        let present = self.present(&mut ops);
        // LOAD_METHOD's callable credit spans the leaf's poll and the
        // wrapper's return poll. Each physical frame separately owns its
        // callable. The linked homes hold the receivers.
        let method = self.hold(&mut ops, self.state.leaf.function);
        let leaf_callable = self.hold(&mut ops, self.state.leaf.function);
        let field = self.hold(&mut ops, field);
        let scope = WrapperScope {
            identity: wrapper_id,
            present,
            callable: self.hold(&mut ops, self.state.wrapper.function),
            method,
        };
        let wrapper = self
            .state
            .wrapper
            .level(self.state.wrapper_at_leaf, wrapper_id, present);
        let leaf = self
            .state
            .leaf
            .level(snapshot(self.state.leaf_check), leaf_id, present);
        let final_leaf = self
            .state
            .leaf
            .level(self.state.leaf_retire, leaf_id, present);

        let (service, next) =
            self.wrapper_service(vec![], scope, ready, outward, ExceptionEdgeKind::Origin);
        let release = insert(self.func, vec![Self::release(method)], jump(next));
        let normal = service_if_pending(self.func, service, release);
        let (service, next) =
            self.wrapper_service(vec![], scope, ready, outward, ExceptionEdgeKind::Unwind);
        let release = insert(self.func, vec![Self::release(method)], jump(next));
        let failed = service_if_pending(self.func, service, release);
        let normal = self.retire(
            leaf_id,
            Some(&final_leaf),
            std::slice::from_ref(&wrapper),
            leaf_callable,
            normal,
        );
        let failed = self.retire(
            leaf_id,
            None,
            std::slice::from_ref(&wrapper),
            leaf_callable,
            failed,
        );
        let leaf_service = self.poll(
            ops,
            &[wrapper, leaf],
            Poll {
                next: normal,
                failed,
                origin: ExceptionEdgeKind::Origin,
                release: field,
            },
        );
        service_if_pending(self.func, leaf_service, without_leaf)
    }
}

/// Build the successor of the inline-fit test. The caller provides an
/// exception observer whose failure payload cannot read the invocation result.
fn emit(
    func: &mut TirFunction,
    state: ReadState<'_>,
    field: ValueId,
    raw: ValueId,
    ready: BlockId,
    outward: TirOp,
) -> BlockId {
    let labels = LabelAllocator::for_function(func);
    let caller_location = outward.frame_location();
    let span = outward.source_span;
    Resume {
        func,
        state,
        labels,
        caller_location,
        span,
    }
    .emit(field, raw, ready, outward)
}

/// Project the composed wrapper, guarded by the original literal provider's
/// output placement. The original invocation handles every pre-read miss.
pub(super) fn splice(
    caller: &mut TirFunction,
    callee: &TirFunction,
    site: &StaticCallSite,
    summary: &PackedWrapper,
    activations: &mut Activations,
) -> Option<BlockId> {
    if caller.execution_context != molt_ir::ExecutionContextPolicy::Local
        || caller.frame_locals_exposed()
    {
        return None;
    }
    let block = caller.blocks.get(&site.block)?;
    let call = block.ops.get(site.op_index)?.clone();
    let [joined] = call.results[..] else {
        return None;
    };
    let prefix = Prefix::prepare(summary, callee, &call)?;
    let binding = crate::tir::call_targets::static_call(&call)?;
    let args = binding.bind(callee)?;
    let captured = binding.callable?;
    let mut observer = block.ops.get(site.op_index + 1)?.clone();
    let _ = activations.bindings(caller, callee, site, args.positional, 0);
    let success_only = activations.checked_result_is_success_only(joined);
    let receiver_owner =
        activations.frame_parameter_outlives(site, args.positional[summary.receiver]);
    let capture_owner = activations.immutable_cell_holds(captured)
        || activations.caller_keeps_alive(site, captured);
    if !success_only
        || observer.opcode != OpCode::CheckException
        || observer.exception_edge_kind() != Some(ExceptionEdgeKind::Origin)
        || observer.has_checked_invocation_binding()
        || !receiver_owner
        || !capture_owner
    {
        return None;
    }
    if crate::tir::passes::async_work_poll::is_call_return_poll(&call) {
        observer.mark_async_work_poll();
    }
    let ty = caller.value_types.get(&joined)?.clone();
    let span = call.source_span;
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
    let miss = caller.fresh_block();
    let entry = prefix.emit(caller, miss, |caller, state| {
        let field = word(caller, TirType::DynBox);
        let raw = word(caller, TirType::I64);
        let inline = word(caller, TirType::I64);
        let read_args = [field, state.row, state.column];
        let mut probe = TirOp::new(OpCode::ProbeInlineField, vec![state.receiver], vec![field]);
        probe
            .attrs
            .insert("value".into(), AttrValue::Int(state.offset));
        probe.source_span = span;
        let resume = emit(caller, state, field, inline, ready, observer);
        let mut scalar = TirOp::new(OpCode::Copy, vec![raw], vec![inline]);
        scalar.mark_inline_int_proof();
        let fit = insert(caller, vec![scalar], jump(resume));
        let flag = caller.fresh_value();
        let fit = test_block(
            caller,
            vec![],
            TirOp::new(OpCode::IsInlineInt, vec![raw], vec![flag]),
            fit,
            miss,
        );
        let flag = caller.fresh_value();
        let load = TirOp::new(
            OpCode::TryBuffer2dLoadI64,
            read_args.to_vec(),
            vec![raw, flag],
        );
        test_block(caller, vec![probe], load, fit, miss)
    });
    original.terminator = jump(entry);
    caller.blocks.insert(site.block, original);
    caller.move_loop_terminator(site.block, ready);
    ordinary_call(caller, miss, ready, call);
    Some(ready)
}
