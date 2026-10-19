use std::collections::{HashMap, HashSet, VecDeque};

use super::super::is_structural;
use super::variables::is_variable;
use super::*;
use crate::tir::op_kinds_generated::opcode_result_is_conditionally_valid_only_on_edge;
use crate::tir::op_semantics::op_instance_effects_for_op;
use crate::tir::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TirOpLocation {
    block: usize,
    op: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CanonicalOrigin {
    Unresolved,
    Unique(ValueId),
    Mixed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PairProvenance {
    iter_pairs: HashSet<ValueId>,
    has_other: bool,
}

#[derive(Clone, Debug)]
struct TirIterProjection {
    location: TirOpLocation,
    result: ValueId,
    name: String,
}

#[derive(Clone, Debug)]
struct TirIterFusePlan {
    producer: TirOpLocation,
    pair: ValueId,
    done: TirIterProjection,
    value: TirIterProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TirIterValidityGuard {
    block: usize,
    done_target: usize,
    not_done_target: usize,
}

const ITER_PATH_NO_STEP: u8 = 1;
const ITER_PATH_AWAITING_GUARD: u8 = 2;
const ITER_PATH_VALID_ITEM: u8 = 4;

fn graph_has_cycle(graph: &[Vec<usize>]) -> bool {
    let mut active = vec![false; graph.len()];
    let mut indegree = vec![0usize; graph.len()];
    for (source, targets) in graph.iter().enumerate() {
        if !targets.is_empty() {
            active[source] = true;
        }
        for &target in targets {
            if target >= graph.len() {
                return true;
            }
            active[target] = true;
            indegree[target] += 1;
        }
    }
    let active_count = active.iter().filter(|&&is_active| is_active).count();
    let mut roots = VecDeque::new();
    for (node, &is_active) in active.iter().enumerate() {
        if is_active && indegree[node] == 0 {
            roots.push_back(node);
        }
    }
    let mut visited = 0usize;
    while let Some(node) = roots.pop_front() {
        visited += 1;
        for &target in &graph[node] {
            indegree[target] -= 1;
            if indegree[target] == 0 {
                roots.push_back(target);
            }
        }
    }
    visited != active_count
}

impl<'a> SsaContext<'a> {
    /// Replace a materialized iterator pair with `IterNextUnboxed` only after
    /// ordinary SSA renaming has made shadowing, merges, and exact uses explicit
    /// as `ValueId`s. Block-argument forwarding is canonicalized separately
    /// from semantic uses, so CFG transport cannot masquerade as a pair read.
    pub(super) fn fuse_iter_next_projections(&mut self, blocks: &mut [TirBlock]) {
        let mut definitions = HashMap::<ValueId, TirOpLocation>::new();
        let mut candidates = Vec::<(TirOpLocation, ValueId)>::new();
        for (block_idx, block) in blocks.iter().enumerate() {
            for (op_idx, op) in block.ops.iter().enumerate() {
                let location = TirOpLocation {
                    block: block_idx,
                    op: op_idx,
                };
                for &result in &op.results {
                    definitions.insert(result, location);
                }
                if op.opcode == OpCode::IterNext && op.operands.len() == 1 && op.results.len() == 1
                {
                    candidates.push((location, op.results[0]));
                }
            }
        }
        if candidates.is_empty() {
            return;
        }

        let mut incoming: Vec<Vec<Vec<ValueId>>> = blocks
            .iter()
            .map(|block| vec![Vec::new(); block.args.len()])
            .collect();
        let mut incomplete: Vec<Vec<bool>> = blocks
            .iter()
            .map(|block| vec![false; block.args.len()])
            .collect();
        let mut unmatched_forwarding = Vec::<ValueId>::new();
        for block in blocks.iter() {
            block.terminator.for_each_edge(|target, args| {
                let target_idx = target.0 as usize;
                let Some(target_incoming) = incoming.get_mut(target_idx) else {
                    unmatched_forwarding.extend(args.iter().copied());
                    return;
                };
                for arg_idx in 0..target_incoming.len() {
                    if let Some(&value) = args.get(arg_idx) {
                        target_incoming[arg_idx].push(value);
                    } else {
                        incomplete[target_idx][arg_idx] = true;
                    }
                }
                unmatched_forwarding.extend(args.iter().skip(target_incoming.len()).copied());
            });
        }

        let candidate_pairs: HashSet<ValueId> = candidates.iter().map(|(_, pair)| *pair).collect();
        let mut origins = HashMap::<ValueId, CanonicalOrigin>::new();
        let mut provenance = HashMap::<ValueId, PairProvenance>::new();
        for block in blocks.iter() {
            for op in &block.ops {
                for &result in &op.results {
                    origins.insert(result, CanonicalOrigin::Unique(result));
                    let mut value_provenance = PairProvenance {
                        has_other: true,
                        ..PairProvenance::default()
                    };
                    if candidate_pairs.contains(&result) {
                        value_provenance.iter_pairs.insert(result);
                        value_provenance.has_other = false;
                    }
                    provenance.insert(result, value_provenance);
                }
            }
        }
        for (block_idx, block) in blocks.iter().enumerate() {
            for (arg_idx, arg) in block.args.iter().enumerate() {
                if incoming[block_idx][arg_idx].is_empty() {
                    origins.insert(arg.id, CanonicalOrigin::Unique(arg.id));
                    provenance.insert(
                        arg.id,
                        PairProvenance {
                            has_other: true,
                            ..PairProvenance::default()
                        },
                    );
                } else {
                    origins.insert(arg.id, CanonicalOrigin::Unresolved);
                    provenance.insert(arg.id, PairProvenance::default());
                }
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for (block_idx, block) in blocks.iter().enumerate() {
                for (arg_idx, arg) in block.args.iter().enumerate() {
                    if incoming[block_idx][arg_idx].is_empty() {
                        continue;
                    }

                    let mut next_origin = CanonicalOrigin::Unresolved;
                    let mut next_provenance = PairProvenance {
                        has_other: incomplete[block_idx][arg_idx],
                        ..PairProvenance::default()
                    };
                    for incoming_value in &incoming[block_idx][arg_idx] {
                        match origins
                            .get(incoming_value)
                            .copied()
                            .unwrap_or(CanonicalOrigin::Mixed)
                        {
                            CanonicalOrigin::Unresolved => {}
                            CanonicalOrigin::Unique(origin) => {
                                next_origin = match next_origin {
                                    CanonicalOrigin::Unresolved => CanonicalOrigin::Unique(origin),
                                    CanonicalOrigin::Unique(current) if current == origin => {
                                        CanonicalOrigin::Unique(current)
                                    }
                                    CanonicalOrigin::Unique(_) | CanonicalOrigin::Mixed => {
                                        CanonicalOrigin::Mixed
                                    }
                                };
                            }
                            CanonicalOrigin::Mixed => next_origin = CanonicalOrigin::Mixed,
                        }
                        if let Some(incoming_provenance) = provenance.get(incoming_value) {
                            next_provenance
                                .iter_pairs
                                .extend(incoming_provenance.iter_pairs.iter().copied());
                            next_provenance.has_other |= incoming_provenance.has_other;
                        } else {
                            next_provenance.has_other = true;
                        }
                    }
                    if incomplete[block_idx][arg_idx] {
                        next_origin = CanonicalOrigin::Mixed;
                    }
                    if origins.get(&arg.id) != Some(&next_origin) {
                        origins.insert(arg.id, next_origin);
                        changed = true;
                    }
                    if provenance.get(&arg.id) != Some(&next_provenance) {
                        provenance.insert(arg.id, next_provenance);
                        changed = true;
                    }
                }
            }
        }
        for origin in origins.values_mut() {
            if *origin == CanonicalOrigin::Unresolved {
                *origin = CanonicalOrigin::Mixed;
            }
        }

        let mut candidate_uses =
            HashMap::<ValueId, Vec<(TirOpLocation, usize)>>::with_capacity(candidates.len());
        let mut invalid_candidates = HashSet::<ValueId>::new();
        for (block_idx, block) in blocks.iter().enumerate() {
            for (arg_idx, arg) in block.args.iter().enumerate() {
                let Some(pair_provenance) = provenance.get(&arg.id) else {
                    continue;
                };
                for pair in &pair_provenance.iter_pairs {
                    let removable = !incomplete[block_idx][arg_idx]
                        && !pair_provenance.has_other
                        && pair_provenance.iter_pairs.len() == 1
                        && origins.get(&arg.id) == Some(&CanonicalOrigin::Unique(*pair));
                    if !removable {
                        invalid_candidates.insert(*pair);
                    }
                }
            }
        }
        for value in unmatched_forwarding {
            if let Some(pair_provenance) = provenance.get(&value) {
                invalid_candidates.extend(pair_provenance.iter_pairs.iter().copied());
            }
        }
        for (block_idx, block) in blocks.iter().enumerate() {
            for (op_idx, op) in block.ops.iter().enumerate() {
                for (operand_idx, operand) in op.operands.iter().enumerate() {
                    let Some(pair_provenance) = provenance.get(operand) else {
                        continue;
                    };
                    for pair in &pair_provenance.iter_pairs {
                        if pair_provenance.iter_pairs.len() == 1
                            && !pair_provenance.has_other
                            && origins.get(operand) == Some(&CanonicalOrigin::Unique(*pair))
                        {
                            candidate_uses.entry(*pair).or_default().push((
                                TirOpLocation {
                                    block: block_idx,
                                    op: op_idx,
                                },
                                operand_idx,
                            ));
                        } else {
                            invalid_candidates.insert(*pair);
                        }
                    }
                }
            }
            block.terminator.for_each_direct_value(|value| {
                if let Some(pair_provenance) = provenance.get(&value) {
                    invalid_candidates.extend(pair_provenance.iter_pairs.iter().copied());
                }
            });
        }

        let mut plans = Vec::<TirIterFusePlan>::new();
        for (producer, pair) in candidates {
            let Some(uses) = candidate_uses.remove(&pair) else {
                continue;
            };
            if invalid_candidates.contains(&pair) || uses.len() != 2 {
                continue;
            }

            let mut done = None;
            let mut value = None;
            for (location, operand_idx) in uses {
                let projection = &blocks[location.block].ops[location.op];
                if projection.opcode != OpCode::Index
                    || operand_idx != 0
                    || projection.operands.len() != 2
                    || projection.results.len() != 1
                {
                    done = None;
                    value = None;
                    break;
                }
                let selector = projection.operands[1];
                let Some(CanonicalOrigin::Unique(selector_definition)) =
                    origins.get(&selector).copied()
                else {
                    done = None;
                    value = None;
                    break;
                };
                let Some(selector_location) = definitions.get(&selector_definition).copied() else {
                    done = None;
                    value = None;
                    break;
                };
                let selector_op = &blocks[selector_location.block].ops[selector_location.op];
                let Some(AttrValue::Int(selector_value)) = selector_op.attrs.get("value") else {
                    done = None;
                    value = None;
                    break;
                };
                let Some(AttrValue::Str(name)) = projection.attrs.get("_simple_out") else {
                    done = None;
                    value = None;
                    break;
                };
                if selector_op.opcode != OpCode::ConstInt
                    || name.is_empty()
                    || name == "none"
                    || !self.tir_op_dominates(selector_location, location)
                {
                    done = None;
                    value = None;
                    break;
                }
                let candidate = TirIterProjection {
                    location,
                    result: projection.results[0],
                    name: name.clone(),
                };
                match *selector_value {
                    1 if done.is_none() => done = Some(candidate),
                    0 if value.is_none() => value = Some(candidate),
                    _ => {
                        done = None;
                        value = None;
                        break;
                    }
                }
            }
            let (Some(done), Some(value)) = (done, value) else {
                continue;
            };
            if done.name == value.name
                || !self.tir_op_dominates(producer, done.location)
                || !self.tir_op_dominates(producer, value.location)
                || !self.tir_op_dominates(done.location, value.location)
                || !self.iter_item_projection_is_valid(
                    blocks,
                    pair,
                    producer,
                    &done,
                    &value,
                    &provenance,
                )
            {
                continue;
            }
            plans.push(TirIterFusePlan {
                producer,
                pair,
                done,
                value,
            });
        }
        if plans.is_empty() {
            return;
        }

        let planned_pairs: HashSet<ValueId> = plans.iter().map(|plan| plan.pair).collect();
        let mut removed_block_args: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
        for plan in &plans {
            let producer = &mut blocks[plan.producer.block].ops[plan.producer.op];
            producer.opcode = OpCode::IterNextUnboxed;
            producer.results = vec![plan.value.result, plan.done.result];
            producer.attrs.remove("_simple_out");
            producer
                .attrs
                .insert("_original_kind".into(), AttrValue::Str("iter_next".into()));
            producer.attrs.insert(
                "_simple_result_0".into(),
                AttrValue::Str(plan.value.name.clone()),
            );
            producer.attrs.insert(
                "_simple_result_1".into(),
                AttrValue::Str(plan.done.name.clone()),
            );
            self.value_types.remove(&plan.pair);
        }
        for (block_idx, block) in blocks.iter().enumerate() {
            for (arg_idx, arg) in block.args.iter().enumerate() {
                let Some(CanonicalOrigin::Unique(pair)) = origins.get(&arg.id).copied() else {
                    continue;
                };
                if planned_pairs.contains(&pair)
                    && provenance.get(&arg.id).is_some_and(|facts| {
                        !facts.has_other
                            && facts.iter_pairs.len() == 1
                            && facts.iter_pairs.contains(&pair)
                    })
                {
                    removed_block_args[block_idx].push(arg_idx);
                }
            }
        }
        for indices in &mut removed_block_args {
            indices.sort_unstable();
            indices.dedup();
        }
        for block in blocks.iter_mut() {
            block.terminator.for_each_edge_mut(|target, args| {
                if let Some(indices) = removed_block_args.get(target.0 as usize) {
                    for &index in indices.iter().rev() {
                        if index < args.len() {
                            args.remove(index);
                        }
                    }
                }
            });
        }
        for (block_idx, indices) in removed_block_args.iter().enumerate() {
            for &index in indices.iter().rev() {
                let removed = blocks[block_idx].args.remove(index);
                self.value_types.remove(&removed.id);
            }
        }

        let mut removed_ops: Vec<HashSet<usize>> = vec![HashSet::new(); blocks.len()];
        for plan in plans {
            removed_ops[plan.done.location.block].insert(plan.done.location.op);
            removed_ops[plan.value.location.block].insert(plan.value.location.op);
        }
        for (block_idx, block) in blocks.iter_mut().enumerate() {
            let mut op_idx = 0usize;
            block.ops.retain(|_| {
                let keep = !removed_ops[block_idx].contains(&op_idx);
                op_idx += 1;
                keep
            });
        }
    }

    /// Prove the generated conditional-result contract for a proposed
    /// `IterNextUnboxed` value result. The bounded state walk consumes the
    /// already-built augmented CFG and exact SSA identities; it is not a
    /// reaching-definition analysis. Each candidate carries one three-bit
    /// state per block and one bounded edge list. The cost per candidate is
    /// `O(blocks + edges + ops + checks * log(edges + 1))`, where checks are
    /// inline pending-error observations. Their handler lookup reuses the
    /// canonical sorted CFG relation without a second handler index or per-op
    /// map cloning.
    fn iter_item_projection_is_valid(
        &self,
        blocks: &[TirBlock],
        pair: ValueId,
        producer: TirOpLocation,
        done: &TirIterProjection,
        value: &TirIterProjection,
        provenance: &HashMap<ValueId, PairProvenance>,
    ) -> bool {
        if !opcode_result_is_conditionally_valid_only_on_edge(OpCode::IterNextUnboxed, 0)
            || opcode_result_is_conditionally_valid_only_on_edge(OpCode::IterNextUnboxed, 1)
            || self.aug_successors.len() != blocks.len()
            || self.cfg.entry >= blocks.len()
        {
            return false;
        }

        let mut guard = None;
        for (block_idx, block) in blocks.iter().enumerate() {
            let Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                ..
            } = &block.terminator
            else {
                continue;
            };
            if *cond != done.result {
                continue;
            }
            let candidate = TirIterValidityGuard {
                block: block_idx,
                done_target: then_block.0 as usize,
                not_done_target: else_block.0 as usize,
            };
            if candidate.done_target == candidate.not_done_target
                || candidate.done_target >= blocks.len()
                || candidate.not_done_target >= blocks.len()
                || then_args.iter().any(|forwarded| {
                    provenance
                        .get(forwarded)
                        .is_some_and(|facts| facts.iter_pairs.contains(&pair))
                })
                || guard.replace(candidate).is_some()
            {
                return false;
            }
        }
        let Some(guard) = guard else {
            return false;
        };
        let guard_terminator = TirOpLocation {
            block: guard.block,
            op: blocks[guard.block].ops.len(),
        };
        if !self.tir_op_dominates(producer, guard_terminator)
            || !self.tir_op_dominates(done.location, guard_terminator)
        {
            return false;
        }

        let mut incoming_states = vec![0u8; blocks.len()];
        incoming_states[self.cfg.entry] = ITER_PATH_NO_STEP;
        let mut regular_edges = HashSet::new();
        for (source, block) in blocks.iter().enumerate() {
            block.terminator.for_each_edge(|target, _| {
                regular_edges.insert((source, target.0 as usize));
            });
        }
        let implicit_edges: HashSet<(usize, usize)> = self
            .cfg
            .exception_edges
            .iter()
            .copied()
            .chain(
                self.cfg
                    .state_resume_edges
                    .iter()
                    .map(|&(from, to, _)| (from, to)),
            )
            .collect();
        let resume_edges: HashSet<(usize, usize)> = self
            .cfg
            .state_resume_edges
            .iter()
            .map(|&(from, to, _)| (from, to))
            .collect();
        let mut worklist = VecDeque::from([self.cfg.entry]);
        let mut queued = vec![false; blocks.len()];
        queued[self.cfg.entry] = true;
        let mut reached_producer = false;
        let mut reached_value = false;
        let mut awaiting_edges = vec![Vec::<usize>::new(); blocks.len()];

        while let Some(block_idx) = worklist.pop_front() {
            queued[block_idx] = false;
            let block = &blocks[block_idx];
            let mut states = incoming_states[block_idx];
            let mut exception_probe = None;
            let mut inline_exception_target = None;
            for (op_idx, op) in block.ops.iter().enumerate() {
                let location = TirOpLocation {
                    block: block_idx,
                    op: op_idx,
                };
                if location == producer {
                    reached_producer = true;
                    states = ITER_PATH_AWAITING_GUARD;
                    continue;
                }
                if location == value.location {
                    reached_value = true;
                    if states == 0 || states & !ITER_PATH_VALID_ITEM != 0 {
                        return false;
                    }
                }
                if states & ITER_PATH_AWAITING_GUARD != 0
                    && location != done.location
                    && location != value.location
                {
                    let effects = op_instance_effects_for_op(op, &self.value_types);
                    let exact_exception_probe =
                        if op.opcode == OpCode::ExceptionPending && op.results.len() == 1 {
                            match &block.terminator {
                                Terminator::CondBranch {
                                    cond,
                                    then_block,
                                    then_args,
                                    else_block,
                                    ..
                                } if *cond == op.results[0]
                                    && then_block != else_block
                                    && !then_args.iter().any(|forwarded| {
                                        provenance
                                            .get(forwarded)
                                            .is_some_and(|facts| facts.iter_pairs.contains(&pair))
                                    }) =>
                                {
                                    Some((then_block.0 as usize, else_block.0 as usize))
                                }
                                _ => None,
                            }
                        } else {
                            None
                        };
                    // The frontend's pending-error check carries its handler
                    // edge inline, rather than spelling a boolean branch. It
                    // only observes pending state: no Python callback can
                    // observe an exhausted pair before its done guard. A fused
                    // eval-breaker poll has a different callback contract.
                    let inline_check_target = (op.opcode == OpCode::CheckException
                        && !op.is_async_work_poll()
                        && op.results.is_empty()
                        && op_idx + 1 == block.ops.len())
                    .then(|| self.cfg.exception_target(block_idx))
                    .flatten();
                    if !(effects.effect_free && effects.nothrow)
                        && exact_exception_probe.is_none()
                        && inline_check_target.is_none()
                    {
                        return false;
                    }
                    if let Some(target) = inline_check_target {
                        inline_exception_target = Some(target);
                    }
                    if let Some(targets) = exact_exception_probe
                        && exception_probe.replace(targets).is_some()
                    {
                        return false;
                    }
                }
            }

            let mut propagated = false;
            for &target in &self.aug_successors[block_idx] {
                if target >= blocks.len() {
                    return false;
                }
                let regular_edge = regular_edges.contains(&(block_idx, target));
                let implicit_edge = implicit_edges.contains(&(block_idx, target));

                // An exact pending-state observation abandons this step on
                // its authored handler edge. All other implicit transfers,
                // including a resume edge to that same target, remain unsafe.
                if states & ITER_PATH_AWAITING_GUARD != 0
                    && implicit_edge
                    && (inline_exception_target != Some(target)
                        || resume_edges.contains(&(block_idx, target)))
                {
                    return false;
                }

                if regular_edge {
                    propagated = true;
                    let next_states = if block_idx == guard.block {
                        if target == guard.not_done_target {
                            let mut next = states & (ITER_PATH_NO_STEP | ITER_PATH_VALID_ITEM);
                            if states & ITER_PATH_AWAITING_GUARD != 0 {
                                next |= ITER_PATH_VALID_ITEM;
                            }
                            next
                        } else if target == guard.done_target {
                            if states != 0 { ITER_PATH_NO_STEP } else { 0 }
                        } else {
                            return false;
                        }
                    } else if let Some((exception_target, continue_target)) = exception_probe {
                        if target == exception_target {
                            if states != 0 { ITER_PATH_NO_STEP } else { 0 }
                        } else if target == continue_target {
                            states
                        } else {
                            return false;
                        }
                    } else {
                        states
                    };
                    if states & ITER_PATH_AWAITING_GUARD != 0
                        && next_states & ITER_PATH_AWAITING_GUARD != 0
                    {
                        awaiting_edges[block_idx].push(target);
                    }
                    if next_states & !incoming_states[target] != 0 {
                        incoming_states[target] |= next_states;
                        if !queued[target] {
                            queued[target] = true;
                            worklist.push_back(target);
                        }
                    }
                }

                if implicit_edge || !regular_edge {
                    propagated = true;
                    let next_states = if states != 0 { ITER_PATH_NO_STEP } else { 0 };
                    if next_states & !incoming_states[target] != 0 {
                        incoming_states[target] |= next_states;
                        if !queued[target] {
                            queued[target] = true;
                            worklist.push_back(target);
                        }
                    }
                }
            }
            if states & ITER_PATH_AWAITING_GUARD != 0 && !propagated {
                return false;
            }
        }

        // Each state bit is admitted once. Revisited edges remain bounded by
        // those three bits; Kahn's indegree walk handles parallel edges without
        // sorting/deduplication, keeping high-fanout switches linear as well.
        if !reached_producer || !reached_value || graph_has_cycle(&awaiting_edges) {
            return false;
        }
        true
    }

    fn tir_op_dominates(&self, definition: TirOpLocation, usage: TirOpLocation) -> bool {
        if definition.block == usage.block {
            return definition.op < usage.op;
        }
        let mut cursor = usage.block;
        while let Some(idom) = self.aug_dominators[cursor] {
            if idom == definition.block {
                return true;
            }
            if idom == cursor {
                break;
            }
            cursor = idom;
        }
        false
    }

    // -- Phase 1: gather variable defs and uses per block --------------------

    pub(super) fn gather_defs_uses(&mut self) {
        for bb in &self.cfg.blocks {
            let mut defs = HashSet::new();
            let mut uses = HashSet::new();
            let mut op_indices = Vec::new();

            for idx in bb.start_op..bb.end_op {
                let op = &self.ops[idx];

                let structural = is_structural(&op.kind);
                if !structural {
                    op_indices.push(idx);
                }

                visit_simple_ir_reads(op, |source| {
                    let name = source.name;
                    if is_variable(name) && !defs.contains(name) {
                        uses.insert(name.to_string());
                    }
                });
                visit_simple_ir_defined_names(op, |name| {
                    if is_variable(name) {
                        let name = name.to_string();
                        defs.insert(name.clone());
                        self.all_vars.insert(name);
                    }
                });
            }

            // Function parameters are implicit definitions in the entry block.
            if bb.id == self.cfg.entry {
                for p in &self.params {
                    if is_variable(p) {
                        defs.insert(p.clone());
                        self.all_vars.insert(p.clone());
                    }
                }
            }
            self.block_info.push(BlockInfo {
                defs,
                uses,
                op_indices,
            });
        }
    }

    // -- Phase 1.5: augmented CFG (regular edges + exception edges) ----------
    //
    // Exception handler blocks are reached via implicit exception edges, not
    // ordinary CFG branches. The regular `cfg.predecessors` array therefore
    // does not list any predecessors for handler blocks. Using only those
    // regular edges to compute dominators makes handler blocks unreachable
    // from the entry, which then makes the dominator analysis miss true
    // join points where a handler's normal exit rejoins the success path
    // (e.g. the merge block after a `try ... finally`).
    //
    // To restore correct SSA dominance, the SSA pass walks an *augmented*
    // CFG that folds the recorded `cfg.exception_edges` into the predecessor
    // relation, then recomputes the dominator tree on top of it. Iterated
    // dominance frontiers and the variable-rename walk both consume the
    // augmented relation. We do not modify `self.cfg` itself: other passes
    // (loop detection, codegen control flow) intentionally treat exception
    // edges as side channels and must keep their own view.
    pub(super) fn build_augmented_cfg(&mut self) {
        let n = self.cfg.blocks.len();
        // Start from regular predecessors.
        let mut aug_preds: Vec<Vec<usize>> = self.cfg.predecessors.clone();
        // Fold exception edges in.
        for &(from_bid, handler_bid) in &self.cfg.exception_edges {
            if from_bid >= n || handler_bid >= n {
                continue;
            }
            if !aug_preds[handler_bid].contains(&from_bid) {
                aug_preds[handler_bid].push(from_bid);
            }
        }
        // Fold state-machine resume (dispatch) edges in: a suspend op `ret`s, so
        // its resume continuation has no *regular* predecessor — exactly like an
        // exception handler block.  The `state_switch` block dispatches to every
        // resume continuation on re-entry; without these edges the SSA pass
        // computes dominance/phi placement on a CFG missing the dispatch, and a
        // resume-reachable block ends up using a value (block arg / phi) defined
        // only on the linear first-entry path, which the dispatch bypasses.
        for &(switch_bid, resume_bid, _state_id) in &self.cfg.state_resume_edges {
            if switch_bid >= n || resume_bid >= n {
                continue;
            }
            if !aug_preds[resume_bid].contains(&switch_bid) {
                aug_preds[resume_bid].push(switch_bid);
            }
        }
        // Sort for determinism.
        for preds in &mut aug_preds {
            preds.sort_unstable();
            preds.dedup();
        }

        // Build augmented successors for the dominator algorithm's RPO walk.
        let mut aug_succs: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (bid, preds) in aug_preds.iter().enumerate() {
            for &p in preds {
                aug_succs[p].push(bid);
            }
        }
        for succs in &mut aug_succs {
            succs.sort_unstable();
            succs.dedup();
        }

        self.aug_dominators = compute_dominators_from(n, &aug_succs, &aug_preds, self.cfg.entry);
        self.aug_predecessors = aug_preds;
        self.aug_successors = aug_succs;
    }

    // -- Phase 2: dominance frontiers ----------------------------------------

    pub(super) fn compute_dominance_frontiers(&mut self) {
        let n = self.cfg.blocks.len();
        for b in 0..n {
            for &pred in &self.aug_predecessors[b] {
                let mut runner = pred;
                // Walk up the (augmented) dominator tree from `pred` until we
                // reach the immediate dominator of `b` (exclusive).
                loop {
                    // `b` is in DF(runner) if runner doesn't strictly dominate b.
                    // runner dominates pred (or runner==pred), and b has pred as
                    // a predecessor. runner strictly dominates b only if
                    // runner == idom chain ancestor strictly.
                    if Some(runner) == self.aug_dominators[b] {
                        // runner strictly dominates b — stop.
                        break;
                    }
                    // runner == b is also possible in loop headers.
                    if runner == b && self.aug_dominators[b].is_none() && b == self.cfg.entry {
                        break;
                    }
                    self.dom_frontier[runner].insert(b);
                    match self.aug_dominators[runner] {
                        Some(idom) if idom != runner => runner = idom,
                        _ => break,
                    }
                }
            }
        }
    }

    // -- Phase 3: insert block arguments (phi placement) ---------------------

    /// The one definition-site census, initially containing only authored
    /// definitions (including entry parameters). Implicit seeding extends it.
    pub(super) fn source_definition_sites(&self, names: &SimpleNameTable) -> Vec<Vec<usize>> {
        let mut sites = vec![Vec::new(); names.len()];
        for (bid, block) in self.block_info.iter().enumerate() {
            for variable in &block.defs {
                sites[names.id(variable).expect("interned SSA definition") as usize].push(bid);
            }
        }
        sites
    }

    /// Index the existing augmented dominator tree once. Unreachable blocks
    /// have no interval; none of the disconnected local-rename protocol changes.
    fn augmented_dominator_intervals(&self) -> Vec<Option<(usize, usize)>> {
        let n = self.cfg.blocks.len();
        let mut children = vec![Vec::new(); n];
        for (bid, idom) in self.aug_dominators.iter().enumerate() {
            if let Some(parent) = idom {
                children[*parent].push(bid);
            }
        }
        let mut intervals = vec![None; n];
        let mut stack = vec![(self.cfg.entry, false)];
        let mut next = 0;
        while let Some((bid, exiting)) = stack.pop() {
            if exiting {
                let (start, _) = intervals[bid].expect("entered dominator node");
                intervals[bid] = Some((start, next));
                continue;
            }
            intervals[bid] = Some((next, next));
            next += 1;
            stack.push((bid, true));
            for &child in children[bid].iter().rev() {
                stack.push((child, false));
            }
        }
        intervals
    }

    pub(super) fn insert_block_arguments(
        &mut self,
        names: &SimpleNameTable,
        live_in: &[SimpleNameSet],
        def_sites: &[Vec<usize>],
    ) {
        // For each variable, compute the iterated dominance frontier of all
        // blocks that define it, then insert a block argument at those blocks.
        // This is pruned SSA: only insert a block argument when the variable is
        // actually live-in to the join block. Otherwise dead branch-local vars
        // create bogus block params and unresolved predecessor values.
        //
        // Liveness is computed over the augmented CFG (regular + exception
        // edges) so that variables propagated through an exception handler's
        // normal exit are considered live at the post-handler merge block.
        // Function parameters are implicit definitions available at the entry
        // block. Add them as entry-block arguments so the rename phase creates
        // proper ValueIds and subsequent ops can resolve them.
        if !self.params.is_empty() && !self.cfg.blocks.is_empty() {
            let entry = self.cfg.entry;
            for p in self.params.clone() {
                if is_variable(&p) && !self.block_arg_vars[entry].contains(&p) {
                    self.block_arg_vars[entry].push(p);
                }
            }
        }

        // Which blocks define which variables.
        let n = self.cfg.blocks.len();

        // Source definitions and only the actually seeded implicit arguments
        // share this canonical census. Entry parameters were already included.
        let mut all_vars: Vec<_> = self.all_vars.iter().cloned().collect();
        all_vars.sort();
        let mut phi_seen = vec![0usize; n];
        let mut work_seen = vec![0usize; n];
        let mut worklist = VecDeque::new();
        let mut phi_blocks = Vec::new();
        for (index, var) in all_vars.into_iter().enumerate() {
            let generation = index + 1;
            let id = names.id(&var).expect("interned SSA variable");
            worklist.clear();
            phi_blocks.clear();
            for &bid in &def_sites[id as usize] {
                work_seen[bid] = generation;
                worklist.push_back(bid);
            }
            // Keep the former full IDF traversal and final liveness pruning:
            // handler/resume definitions and their later rejoins are unchanged.
            while let Some(bid) = worklist.pop_front() {
                for &df_block in &self.dom_frontier[bid] {
                    if phi_seen[df_block] != generation {
                        phi_seen[df_block] = generation;
                        phi_blocks.push(df_block);
                        if work_seen[df_block] != generation {
                            work_seen[df_block] = generation;
                            worklist.push_back(df_block);
                        }
                    }
                }
            }
            phi_blocks.sort_unstable();
            for &bid in &phi_blocks {
                if live_in[bid].contains(id) && !self.block_arg_vars[bid].contains(&var) {
                    self.block_arg_vars[bid].push(var.clone());
                }
            }
        }

        // Block arguments are a serialized ABI between every predecessor edge
        // and its successor. Their order must not inherit HashSet iteration or
        // dominance-frontier discovery order. Entry parameters retain source
        // signature order; every non-entry phi vector is canonical by variable.
        for (bid, vars) in self.block_arg_vars.iter_mut().enumerate() {
            if bid != self.cfg.entry {
                vars.sort();
                vars.dedup();
            }
        }
    }

    /// Keep direct bindings whose sole source-defining block strictly dominates
    /// the implicit target. All other live-ins retain the existing handler and
    /// resume protocol, including mutable names, self-edges and disconnected
    /// roots. Authored exception transfers and state_switch end their blocks,
    /// so a strict ancestor's final active binding is available at every exit.
    pub(super) fn insert_implicit_edge_block_arguments(
        &mut self,
        names: &SimpleNameTable,
        live_in: &[SimpleNameSet],
        def_sites: &mut [Vec<usize>],
    ) {
        let mut targets: Vec<usize> = self
            .cfg
            .exception_edges
            .iter()
            .map(|&(_, target)| target)
            .chain(
                self.cfg
                    .state_resume_edges
                    .iter()
                    .map(|&(_, target, _)| target),
            )
            .collect();
        targets.sort_unstable();
        targets.dedup();
        if targets.is_empty() {
            return;
        }
        // Freeze source-only eligibility before any seeded argument adds a
        // fresh definition. Multiple writes in one block still have one final
        // binding at every exit; writes after a transfer belong to another block.
        let sole_source: Vec<Option<usize>> = def_sites
            .iter()
            .map(|sites| (sites.len() == 1).then(|| sites[0]))
            .collect();
        let intervals = self.augmented_dominator_intervals();
        for bid in targets {
            let mut retained: Vec<_> = live_in[bid]
                .iter()
                .filter(|&id| {
                    let Some(source) = sole_source[id as usize] else {
                        return true;
                    };
                    let (Some((start, end)), Some((target, _))) =
                        (intervals[source], intervals[bid])
                    else {
                        return true;
                    };
                    // Strict ancestry excludes loop/self-entry old values.
                    !(source != bid && start <= target && target < end)
                })
                .map(|id| (names.name(id).to_owned(), id))
                .collect();
            retained.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            for (var, id) in retained {
                if !self.block_arg_vars[bid].contains(&var) {
                    self.block_arg_vars[bid].push(var);
                    def_sites[id as usize].push(bid);
                }
            }
        }
        // Source and seeded sites were each visited in block order. Restore
        // one canonical ordering and remove a possible self-entry duplicate.
        for sites in def_sites {
            sites.sort_unstable();
            sites.dedup();
        }
    }

    pub(super) fn compute_live_in_vars(&self) -> (SimpleNameTable, Vec<SimpleNameSet>) {
        // Use the actual SSA census, including unresolved reads. Parameters are
        // entry definitions here, unlike the native operation-only projection.
        let mut variables: Vec<&str> = self
            .block_info
            .iter()
            .flat_map(|block| block.uses.iter().chain(&block.defs))
            .map(String::as_str)
            .collect();
        variables.sort_unstable();
        variables.dedup();
        let mut names = SimpleNameTable::default();
        for variable in variables {
            names.intern(variable);
        }
        let ids = |set: &HashSet<String>| {
            set.iter()
                .map(|name| names.id(name).expect("interned SSA fact"))
                .collect::<Vec<_>>()
        };
        let uses: Vec<_> = self
            .block_info
            .iter()
            .map(|block| ids(&block.uses))
            .collect();
        let defs: Vec<_> = self
            .block_info
            .iter()
            .map(|block| ids(&block.defs))
            .collect();
        // Preserve the same augmented exception/resume graph used by dominance
        // and argument placement. Only storage and fixed-point scheduling change.
        let (live_in, _) = solve_name_liveness(
            names.len(),
            &self.aug_successors,
            &self.aug_predecessors,
            &uses,
            &defs,
        );
        (names, live_in)
    }
}

fn compute_dominators_from(
    n: usize,
    successors: &[Vec<usize>],
    predecessors: &[Vec<usize>],
    entry: usize,
) -> Vec<Option<usize>> {
    if n == 0 {
        return vec![];
    }

    // RPO over forward (successor) edges from entry.
    let rpo = crate::tir::traversal::indexed_reverse_postorder(successors, entry);
    let mut rpo_number: Vec<usize> = vec![usize::MAX; n];
    for (rpo_idx, &bid) in rpo.iter().enumerate() {
        rpo_number[bid] = rpo_idx;
    }

    let mut idom: Vec<Option<usize>> = vec![None; n];
    idom[entry] = Some(entry);

    let mut changed = true;
    while changed {
        changed = false;
        for &b in &rpo {
            if b == entry {
                continue;
            }
            let mut new_idom: Option<usize> = None;
            for &p in &predecessors[b] {
                if idom[p].is_some() {
                    new_idom = Some(match new_idom {
                        None => p,
                        Some(cur) => intersect_dom_idx(&idom, &rpo_number, cur, p),
                    });
                }
            }
            if new_idom != idom[b] {
                idom[b] = new_idom;
                changed = true;
            }
        }
    }

    idom[entry] = None;
    idom
}

fn intersect_dom_idx(
    idom: &[Option<usize>],
    rpo_number: &[usize],
    mut a: usize,
    mut b: usize,
) -> usize {
    while a != b {
        while rpo_number[a] > rpo_number[b] {
            match idom[a] {
                Some(d) if d != a => a = d,
                _ => break,
            }
        }
        while rpo_number[b] > rpo_number[a] {
            match idom[b] {
                Some(d) if d != b => b = d,
                _ => break,
            }
        }
        if rpo_number[a] == rpo_number[b] && a != b {
            break;
        }
    }
    a
}
