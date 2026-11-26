//! Existing spliced levels are carried by the Link's function, location and
//! identity operands and its Bind operands. Read the linked identities along
//! executable edges before extending that chain with another enclosing frame.
//! The exceptional edge leaves the check with exactly the stack the check saw.

use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::runtime_fn_generated::RuntimeFn;

use crate::tir::blocks::BlockId;
use crate::tir::dominators::{
    exception_label_to_block, exception_transfer_target, terminator_successors,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ExceptionEdgeKind, opcode_is_spliced_frame_capture_constant_table,
};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    /// Publish the outer level before the first previously spliced level.
    Open,
    /// The enclosing level is already linked.
    Linked,
    /// An original cold capture or pop consumes the entire composed chain.
    Close { operand: usize, levels: i64 },
    /// This unlinked check's original cold arm already publishes the chain.
    CapturingCheck,
    /// Retire a child while its enclosing Python activation is executing.
    Finish,
}

#[derive(Debug, Clone, Default)]
pub(super) struct NestedFrames {
    actions: HashMap<(BlockId, usize), Action>,
    dead_unwinds: HashSet<BlockId>,
}

impl NestedFrames {
    pub(super) fn dead_unwind(&self, id: BlockId) -> bool {
        self.dead_unwinds.contains(&id)
    }

    pub(super) fn at(&self, id: BlockId, at: usize) -> Option<Action> {
        self.actions.get(&(id, at)).copied()
    }

    pub(super) fn exit_depths(&self) -> std::collections::BTreeSet<i64> {
        self.actions
            .values()
            .filter_map(|action| match action {
                Action::Close { levels, .. } => Some(*levels),
                _ => None,
            })
            .collect()
    }

    pub(super) fn read(func: &TirFunction) -> Option<Self> {
        let mut result = Self::default();
        let ops = || func.blocks.values().flat_map(|block| &block.ops);
        if !ops().any(crate::tir::passes::spliced_frame::is_spliced_frame_call) {
            return Some(result);
        }
        let slots: HashSet<_> = ops()
            .filter(|op| op.opcode == OpCode::SplicedFrameSlot)
            .filter_map(|op| op.results.first().copied())
            .collect();
        let counts: HashMap<_, _> = ops()
            .filter_map(|op| {
                let [value] = op.results[..] else {
                    return None;
                };
                let Some(AttrValue::Int(count)) = op.attrs.get("value") else {
                    return None;
                };
                (op.opcode == OpCode::ConstInt).then_some((value, *count))
            })
            .collect();
        let labels = exception_label_to_block(func);
        let mut incoming: HashMap<BlockId, Vec<ValueId>> = HashMap::default();
        incoming.insert(func.entry_block, Vec::new());
        let mut pending = vec![func.entry_block];
        let regions = std::cell::OnceCell::new();
        while let Some(id) = pending.pop() {
            let block = func.blocks.get(&id)?;
            let mut stack = incoming[&id].clone();
            let mut transfer = |target, stack: &Vec<ValueId>| -> Option<()> {
                if let Some(prior) = incoming.get(&target) {
                    if prior != stack {
                        return None;
                    }
                } else {
                    incoming.insert(target, stack.clone());
                    pending.push(target);
                }
                Some(())
            };
            for (at, op) in block.ops.iter().enumerate() {
                let linked = !stack.is_empty();
                if linked
                    && (op.opcode == OpCode::Raise
                        || op.runtime == Some(RuntimeFn::ExceptionTracePrepend))
                {
                    return None;
                }
                let mut action = linked.then_some(Action::Linked);
                if op.opcode == OpCode::CheckException {
                    let target = exception_transfer_target(op, &labels)?;
                    transfer(target, &stack)?;
                    if op.exception_edge_kind() == Some(ExceptionEdgeKind::Origin) {
                        let capture = starts_capture(func, target, linked);
                        if linked && !capture {
                            return None;
                        }
                        if !linked && capture {
                            action = Some(Action::CapturingCheck);
                        }
                    }
                }
                match op.runtime {
                    Some(RuntimeFn::SplicedFrameLink) => {
                        let [_, _, identity] = op.operands[..] else {
                            return None;
                        };
                        if !slots.contains(&identity) || stack.contains(&identity) {
                            return None;
                        }
                        stack.push(identity);
                        if !linked {
                            action = Some(Action::Open);
                        }
                    }
                    Some(RuntimeFn::SplicedFrameBind) if !linked => return None,
                    Some(RuntimeFn::SplicedFrameOrigin | RuntimeFn::SplicedFrameUnlink) => {
                        let operand =
                            usize::from(op.runtime == Some(RuntimeFn::SplicedFrameOrigin));
                        let count = *counts.get(op.operands.get(operand)?)?;
                        if !linked || usize::try_from(count).ok()? != stack.len() {
                            return None;
                        }
                        action = Some(Action::Close {
                            operand,
                            levels: count.checked_add(1)?,
                        });
                        stack.clear();
                    }
                    Some(RuntimeFn::SplicedFrameFinish) => {
                        let [identity] = op.operands[..] else {
                            return None;
                        };
                        if !slots.contains(&identity) || stack.contains(&identity) {
                            return None;
                        }
                        if !linked {
                            action = Some(Action::Finish);
                        }
                    }
                    _ => {}
                }
                if linked
                    && (op.opcode == OpCode::FrameSetLocation
                        || matches!(
                            op.origin,
                            Some(molt_ir::OpKind::FrameHomeStore | molt_ir::OpKind::TraceExit)
                        ))
                {
                    return None;
                }
                if let Some(action) = action {
                    result.actions.insert((id, at), action);
                }
            }
            // The inliner leaves an Unwind fallback after the caller's
            // post-call observer on an inner exception exit. Only the
            // original path-state fact can prove this fallthrough dead.
            if matches!(block.terminator, crate::tir::blocks::Terminator::Unwind)
                && block
                    .ops
                    .last()
                    .is_some_and(|op| op.exception_edge_kind() == Some(ExceptionEdgeKind::Unwind))
            {
                use crate::tir::exception_regions::{
                    ExceptionBoundaryHandler, ExceptionOpPosition, compute_exception_region_facts,
                };
                let facts = regions.get_or_init(|| compute_exception_region_facts(func));
                if matches!(
                    facts.lexical_handler_before(ExceptionOpPosition {
                        block: id,
                        op_index: block.ops.len()
                    }),
                    Ok(ExceptionBoundaryHandler::Unreachable)
                ) {
                    result.dead_unwinds.insert(id);
                }
            }
            if block.terminator.exits_activation() && !stack.is_empty() {
                return None;
            }
            for target in terminator_successors(&block.terminator) {
                transfer(target, &stack)?;
            }
        }
        Some(result)
    }
}

/// An Origin check with its own cold frame capture: only the constant/binding
/// setup and the capture may run before it delegates to the authored handler.
fn starts_capture(func: &TirFunction, mut id: BlockId, already_linked: bool) -> bool {
    let mut seen = HashSet::default();
    let mut linked = already_linked;
    while seen.insert(id) {
        let Some(block) = func.blocks.get(&id) else {
            return false;
        };
        for op in &block.ops {
            match op.runtime {
                Some(RuntimeFn::SplicedFrameLink) => linked = true,
                Some(RuntimeFn::SplicedFrameBind) if linked => {}
                Some(RuntimeFn::SplicedFrameOrigin) => return linked,
                None if opcode_is_spliced_frame_capture_constant_table(op.opcode) => {}
                _ => return false,
            }
        }
        let crate::tir::blocks::Terminator::Branch { target, .. } = block.terminator else {
            return false;
        };
        id = target;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::TirOp;
    use molt_ir::{ExecutionContextPolicy, OpKind};

    fn wrapper() -> TirFunction {
        let ir: crate::ir::FunctionIR = serde_json::from_value(serde_json::json!({
            "return_abi":"void","name":"child","execution_context":"local","params":["p"],"param_types":["Any"],
            "ops":[
                {"kind":"trace_enter_slot","s_value":"child","value":1},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"frame_home_store","value":0,"args":["p"],"out":"h"},
                {"kind":"frame_set_location","value":7},
                {"kind":"call","s_value":"observe","args":["h"],"out":"result"},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"const_str","s_value":"nested","out":"literal"},
                {"kind":"check_exception","exception_edge":"origin","value":9},
                {"kind":"trace_exit"},{"kind":"ret_void"},
                {"kind":"label","value":9},{"kind":"trace_exit"},{"kind":"ret_void"}
            ]})).unwrap();
        let mut func = crate::tir::lower_from_simple::lower_to_tir(&ir);
        func.blocks
            .values_mut()
            .flat_map(|b| &mut b.ops)
            .find(|op| op.origin == Some(OpKind::FrameHomeStore))
            .unwrap()
            .mark_nothrow_proof();
        for block in func.blocks.values_mut() {
            if let Some(at) = block
                .ops
                .iter()
                .position(|op| op.opcode == OpCode::ConstStr)
            {
                assert_eq!(block.ops[at + 1].opcode, OpCode::CheckException);
                block.ops[at + 1].mark_async_work_poll();
            }
        }
        func
    }
    #[test]
    fn nested_callback_captures_each_frame_once_and_finishes_the_inner_in_its_parent() {
        use super::super::{frame_splice::prepare_observable, nested_frames::NestedFrames};
        use crate::tir::blocks::{Terminator, TirBlock};
        use crate::tir::clone_support::{LabelAllocator, function_label_ids};
        use crate::tir::types::TirType;
        let mut parent = prepare_observable(&wrapper()).unwrap();
        // The inner's function object and formal have already become values
        // of its enclosing activation. Supply the enclosing record, a home
        // and its own call site, as the ordinary inliner clone does.
        let inner_function = parent.blocks[&parent.entry_block].args.last().unwrap().id;
        let inner_identity = parent.blocks[&parent.entry_block]
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::SplicedFrameSlot)
            .unwrap()
            .results[0];
        let param = parent.blocks[&parent.entry_block].args[0].id;
        let failure = parent.fresh_block();
        let label = LabelAllocator::after_labels(function_label_ids(&parent)).fresh();
        parent.label_id_map.insert(failure.0, label);
        let source = wrapper();
        let entry = &source.blocks[&source.entry_block].ops;
        let mut check = entry[1].clone();
        check.attrs.insert("value".into(), AttrValue::Int(label));
        let view = parent.fresh_value();
        parent.value_types.insert(view, TirType::DynBox);
        let mut home =
            TirOp::new(OpCode::Copy, vec![param], vec![view]).with_origin(OpKind::FrameHomeStore);
        home.attrs.insert("value".into(), AttrValue::Int(0));
        home.mark_nothrow_proof();
        let mut loc = TirOp::new(OpCode::FrameSetLocation, vec![], vec![]);
        loc.attrs.insert("value".into(), AttrValue::Int(21));
        parent
            .blocks
            .get_mut(&parent.entry_block)
            .unwrap()
            .ops
            .splice(0..0, [entry[0].clone(), check, home, loc]);
        parent.blocks.insert(
            failure,
            TirBlock {
                id: failure,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unwind,
            },
        );
        let normal = crate::tir::dominators::reachable_blocks_with(
            &parent,
            crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
        );
        let mut recheck = entry[1].clone();
        recheck.set_exception_edge_kind(ExceptionEdgeKind::Unwind);
        recheck.attrs.insert("value".into(), AttrValue::Int(label));
        for block in parent
            .blocks
            .values_mut()
            .filter(|block| block.terminator.exits_activation())
        {
            if block.id != failure && !normal.contains(&block.id) {
                // The real splice relays a pending inner exception to the
                // caller's post-call check before this dead fallback.
                block.ops.push(recheck.clone());
                block.terminator = Terminator::Unwind;
            } else {
                block
                    .ops
                    .push(TirOp::new(OpCode::Copy, vec![], vec![]).with_origin(OpKind::TraceExit));
            }
        }
        parent.execution_context = ExecutionContextPolicy::Local;
        let child =
            NestedFrames::read(&parent).expect("balanced child on normal and exceptional edges");
        assert!(
            !child.dead_unwinds.is_empty(),
            "the chained Unwind observer always transfers"
        );
        assert!(
            super::super::sync_frame::SyncFrame::analyze(&parent).is_some(),
            "outer homes reach each child publication"
        );
        let body = prepare_observable(&parent).expect("compose the enclosing activation");
        crate::tir::verify::verify_function(&body).unwrap();
        NestedFrames::read(&body).expect("balanced composed links on normal and exceptional edges");
        let function = body.blocks[&body.entry_block].args.last().unwrap().id;
        let consts: HashMap<_, _> = body
            .blocks
            .values()
            .flat_map(|b| &b.ops)
            .filter_map(|op| {
                if op.opcode != OpCode::ConstInt {
                    return None;
                }
                let Some(AttrValue::Int(v)) = op.attrs.get("value") else {
                    return None;
                };
                Some((op.results[0], *v))
            })
            .collect();
        let mut callbacks = 0;
        let mut child_finishes = 0;
        for block in body.blocks.values() {
            for (at, op) in block.ops.iter().enumerate() {
                if op.opcode == OpCode::Call && op.runtime.is_none() {
                    let chain: Vec<_> = block.ops[..at]
                        .iter()
                        .filter(|op| op.runtime == Some(RuntimeFn::SplicedFrameLink))
                        .map(|op| (op.operands[0], consts[&op.operands[1]]))
                        .collect();
                    assert_eq!(chain, [(function, 21), (inner_function, 7)]);
                    let check = &block.ops[at + 1];
                    let cold =
                        exception_transfer_target(check, &exception_label_to_block(&body)).unwrap();
                    let capture = body.blocks[&cold]
                        .ops
                        .iter()
                        .find(|op| op.runtime == Some(RuntimeFn::SplicedFrameOrigin))
                        .unwrap();
                    assert_eq!(consts[&capture.operands[1]], 2);
                    callbacks += 1;
                }
                if op.runtime == Some(RuntimeFn::SplicedFrameFinish)
                    && op.operands == [inner_identity]
                {
                    let current = block.ops[..at]
                        .iter()
                        .rev()
                        .find(|op| op.runtime == Some(RuntimeFn::SplicedFrameLink))
                        .unwrap();
                    assert_eq!(current.operands[0], function);
                    assert_eq!(
                        block.ops[at + 1].runtime,
                        Some(RuntimeFn::SplicedFrameUnlink)
                    );
                    child_finishes += 1;
                }
            }
        }
        let (gate, at) = body
            .blocks
            .values()
            .find_map(|block| {
                block
                    .ops
                    .iter()
                    .position(|op| op.opcode == OpCode::ConstStr)
                    .map(|at| (block.id, at))
            })
            .unwrap();
        let pred = crate::tir::dominators::build_pred_map(&body);
        let Some(
            crate::tir::passes::exception_observation::PostOperationObservation::PolledOrigin {
                service,
                fast,
                ..
            },
        ) = crate::tir::passes::exception_observation::post_operation_observation(
            &body,
            gate,
            at,
            &pred,
            &crate::tir::passes::check_exception_elim::classify::RaiseFacts::of(&body),
        )
        else {
            panic!("nested canonical pending-work observation")
        };
        let service_levels: Vec<_> = body.blocks[&service.0].ops[..service.1]
            .iter()
            .filter(|op| op.runtime == Some(RuntimeFn::SplicedFrameLink))
            .map(|op| op.operands[0])
            .collect();
        assert_eq!(service_levels, [function, inner_function]);
        assert!(
            !body.blocks[&fast.0]
                .ops
                .iter()
                .any(crate::tir::passes::spliced_frame::is_spliced_frame_call)
        );
        assert_eq!(callbacks, 1);
        assert!(child_finishes > 0);
    }
}
