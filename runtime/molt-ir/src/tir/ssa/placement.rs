use std::collections::{HashMap, HashSet, VecDeque};

use super::super::is_structural;
use super::variables::is_variable;
use super::*;
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

        self.aug_predecessors = aug_preds;
        self.aug_dominators =
            compute_dominators_from(n, &aug_succs, &self.aug_predecessors, self.cfg.entry);
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

    pub(super) fn insert_block_arguments(&mut self) {
        // For each variable, compute the iterated dominance frontier of all
        // blocks that define it, then insert a block argument at those blocks.
        // This is pruned SSA: only insert a block argument when the variable is
        // actually live-in to the join block. Otherwise dead branch-local vars
        // create bogus block params and unresolved predecessor values.
        //
        // Liveness is computed over the augmented CFG (regular + exception
        // edges) so that variables propagated through an exception handler's
        // normal exit are considered live at the post-handler merge block.
        let live_in = self.compute_live_in_vars(true);

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

        let mut all_vars: Vec<_> = self.all_vars.iter().cloned().collect();
        all_vars.sort();
        for var in all_vars {
            let mut def_blocks: HashSet<usize> = HashSet::new();
            for bid in 0..n {
                // A block is a definition site for `var` when its ops define
                // it — OR when it is a handler block that already carries
                // `var` as a block argument (established by
                // `insert_exception_handler_arguments`): along the exception
                // edge the handler introduces a fresh SSA value for the
                // variable. It must seed the iterated dominance frontier so
                // that every block where the handler's normal exit rejoins the
                // protected region's control flow receives a phi merging the
                // handler's version with the protected-region version. Without
                // this, a value defined in the protected region and used past
                // such a rejoin is dominated only on the normal path, not on
                // the handler path — a genuine SSA-dominance violation that
                // LLVM's verifier rejects once the handler blocks are lowered.
                if self.block_info[bid].defs.contains(&var)
                    || self.block_arg_vars[bid].contains(&var)
                {
                    def_blocks.insert(bid);
                }
            }

            // Iterated dominance frontier.
            let mut phi_blocks: HashSet<usize> = HashSet::new();
            let mut ordered_def_blocks: Vec<_> = def_blocks.iter().copied().collect();
            ordered_def_blocks.sort_unstable();
            let mut worklist: VecDeque<usize> = ordered_def_blocks.into();
            let mut ever_on_worklist: HashSet<usize> = def_blocks.clone();

            while let Some(bid) = worklist.pop_front() {
                for &df_block in &self.dom_frontier[bid] {
                    if phi_blocks.insert(df_block) {
                        // Also add df_block to worklist if not already processed.
                        if ever_on_worklist.insert(df_block) {
                            worklist.push_back(df_block);
                        }
                    }
                }
            }

            // Record that these blocks need a block argument for this variable.
            let mut ordered_phi_blocks: Vec<_> = phi_blocks.into_iter().collect();
            ordered_phi_blocks.sort_unstable();
            for bid in ordered_phi_blocks {
                if live_in[bid].contains(&var) && !self.block_arg_vars[bid].contains(&var) {
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

    /// Exception handlers are reached via implicit `check_exception` edges,
    /// not ordinary block terminators. Preserve a conservative environment
    /// vector for those targets based on true live-in variables across normal
    /// and exceptional edges. Threading every variable into every handler is
    /// both expensive and unsound: unresolved future vars collapse to
    /// `ValueId(0)` and can corrupt downstream lowering.
    pub(super) fn insert_exception_handler_arguments(&mut self) {
        let mut handler_blocks: HashSet<usize> = HashSet::new();
        for &(_, handler_bid) in &self.cfg.exception_edges {
            handler_blocks.insert(handler_bid);
        }
        if handler_blocks.is_empty() {
            return;
        }

        let live_in = self.compute_live_in_vars(true);
        for bid in handler_blocks {
            let mut vars: Vec<String> = live_in[bid].iter().cloned().collect();
            vars.sort();
            for var in &vars {
                if !self.block_arg_vars[bid].contains(var) {
                    self.block_arg_vars[bid].push(var.clone());
                }
            }
        }
    }

    /// State-machine resume continuations are reached via the implicit
    /// `state_switch` dispatch edge, not an ordinary block terminator — exactly
    /// like exception handler blocks.  Seed each resume block with its true
    /// live-in variables as block arguments so the dispatch edge can supply them
    /// (mirror `insert_exception_handler_arguments`).  This both (a) makes the
    /// resume block a fresh SSA definition site for each live-across-suspend
    /// variable, seeding the IDF so every rejoin past the resume gets a phi, and
    /// (b) gives the `StateDispatch` terminator a concrete block-arg list to fill
    /// from the var stacks live at the dispatch point.
    ///
    /// Variables that the frontend spilled to the frame (the common
    /// live-across-yield case) are reloaded via fresh `closure_load` defs inside
    /// the resume block and are NOT live-in there, so they are not seeded — only
    /// the values genuinely threaded across the suspend (the frame `self`
    /// pointer, exception-stack bookkeeping values) are.
    pub(super) fn insert_state_resume_block_arguments(&mut self) {
        let mut resume_blocks: HashSet<usize> = HashSet::new();
        for &(_, resume_bid, _) in &self.cfg.state_resume_edges {
            resume_blocks.insert(resume_bid);
        }
        if resume_blocks.is_empty() {
            return;
        }

        let live_in = self.compute_live_in_vars(true);
        for bid in resume_blocks {
            let mut vars: Vec<String> = live_in[bid].iter().cloned().collect();
            vars.sort();
            for var in &vars {
                if !self.block_arg_vars[bid].contains(var) {
                    self.block_arg_vars[bid].push(var.clone());
                }
            }
        }
    }

    fn compute_live_in_vars(&self, include_exception_edges: bool) -> Vec<HashSet<String>> {
        let n = self.cfg.blocks.len();
        let mut succs = self.cfg.successors.clone();
        if include_exception_edges {
            for &(from_bid, handler_bid) in &self.cfg.exception_edges {
                if from_bid >= n || handler_bid >= n {
                    continue;
                }
                if !succs[from_bid].contains(&handler_bid) {
                    succs[from_bid].push(handler_bid);
                }
            }
            // The `state_switch` dispatch supplies each resume continuation's
            // live-in on re-entry (the live-across-suspend values that were
            // spilled to the frame and reloaded after the dispatch).  Model the
            // dispatch as a liveness successor of the `state_switch` block so
            // those values are seen as live across the suspend — mirror the
            // exception-handler edge.
            for &(switch_bid, resume_bid, _state_id) in &self.cfg.state_resume_edges {
                if switch_bid >= n || resume_bid >= n {
                    continue;
                }
                if !succs[switch_bid].contains(&resume_bid) {
                    succs[switch_bid].push(resume_bid);
                }
            }
        }
        for block_succs in &mut succs {
            block_succs.sort_unstable();
            block_succs.dedup();
        }

        let mut live_in: Vec<HashSet<String>> = vec![HashSet::new(); n];
        let mut live_out: Vec<HashSet<String>> = vec![HashSet::new(); n];
        let mut changed = true;
        while changed {
            changed = false;
            for bid in (0..n).rev() {
                let mut new_live_out: HashSet<String> = HashSet::new();
                for succ_bid in &succs[bid] {
                    new_live_out.extend(live_in[*succ_bid].iter().cloned());
                }

                let mut new_live_in = self.block_info[bid].uses.clone();
                for var in &new_live_out {
                    if !self.block_info[bid].defs.contains(var) {
                        new_live_in.insert(var.clone());
                    }
                }

                if new_live_out != live_out[bid] || new_live_in != live_in[bid] {
                    live_out[bid] = new_live_out;
                    live_in[bid] = new_live_in;
                    changed = true;
                }
            }
        }

        live_in
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
