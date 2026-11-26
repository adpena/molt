//! An owned frame-slot read that feeds only an immediate native advance can
//! borrow its independently held slot between observations. The original
//! counted temporary still exists during pending service and Origin capture.
use molt_ir::OpKind;
use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::runtime_fn_generated::RuntimeFn;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::{
    ProgramPointDominance, build_pred_map, exception_label_to_block, exception_successors,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::passes::alias_analysis::build_alias_union_find;
use crate::tir::passes::liveness::compute_liveness;
use crate::tir::passes::ownership_lattice_min::{OwnershipRootFacts, owned_alias};
use crate::tir::passes::spliced_frame::poll_service;
use crate::tir::values::ValueId;

type Site = (BlockId, usize);

fn binding(op: &TirOp) -> Option<ValueId> {
    match (
        op.opcode,
        op.origin,
        op.operands.as_slice(),
        op.results.as_slice(),
    ) {
        (OpCode::Copy, Some(OpKind::BindingAlias), [source], [_]) => Some(*source),
        _ => None,
    }
}

struct Read {
    step: Site,
    header: BlockId,
    position: usize,
    first_edge: BlockId,
    reload_edge: BlockId,
    reload_site: Site,
    service: BlockId,
    initial: ValueId,
    saved: ValueId,
    iterator: ValueId,
    reload: ValueId,
}

fn single_forward(block: &TirBlock) -> Option<BlockId> {
    match &block.terminator {
        Terminator::Branch { target, args } if args.is_empty() => Some(*target),
        _ => None,
    }
}

/// Borrow the original credit on the observation's normal side. A release
/// attached directly to that credit would also hold it across exceptional
/// unwinding, changing its rank relative to other abandoned owners. The
/// distinct normal-only owner preserves ordinary SSA liveness on the check.
fn last_normal_use(func: &mut TirFunction, owner: ValueId, span: Option<(u32, u32)>) -> [TirOp; 2] {
    let end = func.fresh_value();
    func.value_types
        .insert(end, crate::tir::types::TirType::DynBox);
    [
        owned_alias(owner, end, span),
        TirOp::new(OpCode::DecRef, vec![end], vec![]),
    ]
}

impl Read {
    fn prove(func: &TirFunction, item: ValueId) -> Option<Self> {
        let mut defs = HashMap::default();
        for block in func.blocks.values() {
            for (index, op) in block.ops.iter().enumerate() {
                for &value in &op.results {
                    defs.insert(value, (block.id, index));
                }
            }
        }
        let site = |id| defs.get(&id).copied();
        let definition = |id| {
            let (b, i) = site(id)?;
            func.blocks[&b].ops.get(i)
        };
        let step = site(item)?;
        let block = &func.blocks[&step.0];
        let op = &block.ops[step.1];
        let ([iterator], [actual, _]) = (op.operands.as_slice(), op.results.as_slice()) else {
            return None;
        };
        let &iterator = iterator;
        let check = block.ops.get(step.1 + 1)?;
        if block.ops.len() != 2
            || single_forward(block).is_none()
            || step.1 != 0
            || *actual != item
            || op.opcode != OpCode::IterNextUnboxed
            || check.opcode != OpCode::CheckException
            || check.is_async_work_poll()
            || check.exception_edge_kind() != Some(ExceptionEdgeKind::Origin)
            || !check.operands.is_empty()
            || !check.results.is_empty()
        {
            return None;
        }
        let (header, position) = func.blocks.values().find_map(|b| {
            b.args
                .iter()
                .position(|a| a.id == iterator)
                .map(|i| (b.id, i))
        })?;
        if !func.blocks[&header].ops.is_empty()
            || single_forward(&func.blocks[&header]) != Some(step.0)
        {
            return None;
        }
        let predecessors = build_pred_map(func);
        let labels = exception_label_to_block(func);
        let exception_entries: HashSet<_> = func
            .blocks
            .values()
            .flat_map(|b| exception_successors(b, &labels))
            .collect();
        if predecessors.get(&step.0)?.as_slice() != [header]
            || predecessors.get(&header)?.len() != 2
            || exception_entries.contains(&step.0)
            || exception_entries.contains(&header)
        {
            return None;
        }
        let mut first = None;
        let mut back = None;
        for pred in func.blocks.values() {
            let Terminator::Branch { target, args } = &pred.terminator else {
                continue;
            };
            if *target != header {
                continue;
            }
            let value = *args.get(position)?;
            if super::is_get_iter_op(definition(value)?) {
                if first.replace((pred.id, value)).is_some() {
                    return None;
                }
            } else {
                let saved = binding(definition(value)?)?;
                if back.replace((pred.id, value, saved)).is_some() {
                    return None;
                }
            }
        }
        let (first_edge, initial) = first?;
        let (reload_edge, reload, saved) = back?;
        if binding(definition(saved)?) != Some(initial)
            || first_edge == reload_edge
            || !func.blocks[&first_edge].ops.is_empty()
        {
            return None;
        }
        let reload_site = site(reload)?;
        let saved_site = site(saved)?;
        let mut reads = 0;
        let mut feeds = 0;
        for b in func.blocks.values() {
            for operation in &b.ops {
                reads += operation
                    .operands
                    .iter()
                    .filter(|&&v| v == iterator || v == reload)
                    .count();
            }
            b.terminator
                .for_each_value(|v| feeds += usize::from(v == iterator || v == reload));
        }
        // Exactly the advance and the backedge; no locals, exceptional
        // payload, loop exit or second consumer sees either temporary.
        if reads != 1 || feeds != 1 {
            return None;
        }
        let Terminator::Branch { args: carries, .. } = &func.blocks[&reload_edge].terminator else {
            return None;
        };
        let mut current = reload_site.0;
        let mut start = reload_site.1 + 1;
        let mut service = None;
        let mut visited = HashSet::default();
        loop {
            if !visited.insert(current) || exception_entries.contains(&current) {
                return None;
            }
            let body = &func.blocks[&current];
            let rest = &body.ops[start..];
            if let Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } = &body.terminator
            {
                let [test] = rest else { return None };
                if service.is_some()
                    || test.opcode != OpCode::AsyncWorkPending
                    || test.results != [*cond]
                    || !then_args.is_empty()
                    || !else_args.is_empty()
                    || then_block == else_block
                    || predecessors.get(then_block)?.as_slice() != [current]
                    || exception_entries.contains(then_block)
                    || predecessors.get(else_block)?.len() != 2
                {
                    return None;
                }
                let cold = &func.blocks[then_block];
                let poll = poll_service(&cold.ops)?;
                if poll.exception_edge_kind() != Some(ExceptionEdgeKind::Unwind)
                    || !poll.operands.is_empty()
                    || single_forward(cold) != Some(*else_block)
                    || !cold.ops.iter().any(|o| {
                        o.runtime == Some(RuntimeFn::SplicedFrameBind)
                            && o.operands.get(2) == Some(&saved)
                    })
                {
                    return None;
                }
                service = Some(*then_block);
                current = *else_block;
            } else {
                // Other reads must stay independently owned through this
                // same backedge, including the original outer iterator.
                if !rest
                    .iter()
                    .all(|o| binding(o).is_some() && carries.contains(&o.results[0]))
                {
                    return None;
                }
                if current == reload_edge {
                    break;
                }
                let next = single_forward(body)?;
                if predecessors.get(&next)?.as_slice() != [current] {
                    return None;
                }
                current = next;
            }
            start = 0;
        }
        let service = service?;
        let dominance = ProgramPointDominance::compute_executable(func);
        let aliases = build_alias_union_find(func);
        let live = compute_liveness(func);
        if !OwnershipRootFacts::result_holds_own_reference(definition(saved)?, saved, &aliases)
            || ![
                step,
                (service, 0),
                reload_site,
                (first_edge, func.blocks[&first_edge].ops.len()),
            ]
            .into_iter()
            .all(|(b, i)| dominance.definition_available(saved_site.0, Some(saved_site.1), b, i))
            || live.is_live_in(header, initial)
            || ![step.0, service]
                .into_iter()
                .all(|b| live.is_live_in(b, saved))
        {
            return None;
        }
        Some(Self {
            step,
            header,
            position,
            first_edge,
            reload_edge,
            reload_site,
            service,
            initial,
            saved,
            iterator,
            reload,
        })
    }

    fn apply(self, func: &mut TirFunction) {
        // Keep the constructor's original owner up to the first header edge.
        let initial_end = last_normal_use(func, self.initial, None);
        func.blocks
            .get_mut(&self.first_edge)
            .unwrap()
            .ops
            .extend(initial_end);
        let reload = func
            .blocks
            .get_mut(&self.reload_site.0)
            .unwrap()
            .ops
            .remove(self.reload_site.1);
        for id in [self.first_edge, self.reload_edge] {
            let Terminator::Branch { args, .. } = &mut func.blocks.get_mut(&id).unwrap().terminator
            else {
                unreachable!()
            };
            args.remove(self.position);
        }
        func.blocks
            .get_mut(&self.header)
            .unwrap()
            .args
            .remove(self.position);
        let service_end = last_normal_use(func, self.reload, reload.source_span);
        let service = func.blocks.get_mut(&self.service).unwrap();
        service.ops.insert(0, reload);
        service.ops.extend(service_end);

        let cold = func.fresh_block();
        let pending = func.fresh_value();
        func.value_types
            .insert(pending, crate::tir::types::TirType::Bool);
        let source = func.blocks.get_mut(&self.step.0).unwrap();
        let clear = single_forward(source).expect("checked step has a normal continuation");
        source.ops[self.step.1].operands[0] = self.saved;
        let origin = source.ops.pop().unwrap();
        let span = origin.source_span;
        let mut test = TirOp::new(OpCode::ExceptionPending, vec![], vec![pending]);
        test.source_span = span;
        source.ops.push(test);
        source.terminator = Terminator::CondBranch {
            cond: pending,
            then_block: cold,
            then_args: vec![],
            else_block: clear,
            else_args: vec![],
        };
        // The routing check has confirmed a pending exception on this thread.
        // A retain of the independently owned native iterator cannot clear it.
        // Keep the original owner id for normal and exceptional retirement.
        let mut observed = vec![owned_alias(self.saved, self.iterator, span), origin];
        observed.extend(last_normal_use(func, self.iterator, span));
        func.blocks.insert(
            cold,
            TirBlock {
                id: cold,
                args: vec![],
                ops: observed,
                // Preserve the authored normal successor even on this cold arm.
                // Its full original live set is also required at Origin.
                terminator: Terminator::Branch {
                    target: clear,
                    args: vec![],
                },
            },
        );
    }
}

pub(super) fn defer_reads(func: &mut TirFunction, steps: &HashSet<ValueId>) {
    let mut items: Vec<_> = steps.iter().copied().collect();
    items.sort_unstable();
    for item in items {
        if let Some(read) = Read::prove(func, item) {
            read.apply(func);
        }
    }
}
