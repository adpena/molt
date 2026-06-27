use std::collections::{HashSet, VecDeque};

use super::super::is_structural;
use super::variables::is_variable;
use super::*;

impl<'a> SsaContext<'a> {
    /// Scan the raw linear op stream for iter_next → index(pair,1) →
    /// index(pair,0) patterns.  This runs BEFORE the CFG splits blocks at
    /// check_exception boundaries, so the pattern can span across them.
    pub(super) fn build_iter_fuse_map(&mut self) {
        let ops = self.ops;
        for (i, op) in ops.iter().enumerate() {
            if op.kind != "iter_next" {
                continue;
            }
            let pair_var = match &op.out {
                Some(v) if v != "none" => v.clone(),
                _ => continue,
            };
            let mut done_idx = None;
            let mut val_idx = None;
            let scan_end = (i + 20).min(ops.len());
            for j in (i + 1)..scan_end {
                let scan_op = &ops[j];
                if scan_op.kind == "index"
                    && let Some(args) = &scan_op.args
                    && args.len() >= 2
                    && args[0] == pair_var
                {
                    let idx_name = &args[1];
                    let const_val = ops[..j].iter().rev().take(20).find_map(|c| {
                        if c.kind == "const" && c.out.as_deref() == Some(idx_name) {
                            c.value
                        } else {
                            None
                        }
                    });
                    if const_val == Some(1) && done_idx.is_none() {
                        done_idx = Some(j);
                    } else if const_val == Some(0) && val_idx.is_none() {
                        val_idx = Some(j);
                    }
                }
            }
            if std::env::var("MOLT_DEBUG_ITER_FUSE").is_ok() {
                eprintln!(
                    "ITER_FUSE iter_next@{i} pair={pair_var} done_idx={done_idx:?} val_idx={val_idx:?}"
                );
            }
            if let (Some(di), Some(vi)) = (done_idx, val_idx) {
                let done_var = ops[di].out.clone().unwrap_or_default();
                let val_var = ops[vi].out.clone().unwrap_or_default();
                self.iter_fuse_map.insert(i, (di, vi, done_var, val_var));
                // Skip all ops between iter_next and the value index — EXCEPT a
                // `loop_break_if_exception` control op.  That op is a second
                // conditional loop break (gated on the runtime exception flag,
                // emitted after ITER_NEXT in iterator-consumer loops compiled
                // without the function exception stack) and MUST survive fusion:
                // adding it to the skip set would silently drop it and
                // re-introduce the infinite-loop/OOM bug on a mid-iteration
                // raise.  It is `is_structural`, so it becomes a block
                // terminator (CondBranch on the materialized `ExceptionPending`
                // flag) rather than a fused body op — fully compatible with the
                // fused `iter_next_unboxed` value/done extraction that precedes
                // it.  Keeping fusion preserves the per-iteration tuple-alloc
                // elision (the perf-critical fast path).
                let skip_end = di.max(vi);
                for skip in (i + 1)..=skip_end {
                    if ops[skip].kind == "loop_break_if_exception" {
                        continue;
                    }
                    self.iter_fuse_skip.insert(skip);
                }
            }
        }
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

                // Uses: args and var (when used as input).
                if op.kind == "unpack_sequence" {
                    if let Some(args) = &op.args {
                        if let Some(seq) = args.first()
                            && is_variable(seq)
                            && !defs.contains(seq)
                        {
                            uses.insert(seq.clone());
                        }
                        for out in args.iter().skip(1) {
                            if is_variable(out) {
                                defs.insert(out.clone());
                                self.all_vars.insert(out.clone());
                            }
                        }
                    }
                } else if let Some(args) = &op.args {
                    for a in args {
                        if is_variable(a) && !defs.contains(a) {
                            uses.insert(a.clone());
                        }
                    }
                }
                // `var` is used as an input for load-style ops (e.g. "load_var").
                // For store-style ops the `var` names the target.
                // Heuristic: if `out` is set, `var` is likely an input;
                // if only `var` is set with no `out`, it could be a store target.
                if let Some(v) = &op.var
                    && is_variable(v)
                {
                    // For "store_var" the var is the destination and args[0] is input.
                    // For most ops, var is read.
                    if !matches!(op.kind.as_str(), "store_var" | "delete_var") && !defs.contains(v)
                    {
                        uses.insert(v.clone());
                    }
                }

                // Definitions: `out` field names the variable being assigned.
                if let Some(out) = &op.out
                    && is_variable(out)
                {
                    defs.insert(out.clone());
                    self.all_vars.insert(out.clone());
                }
                // `store_var`/`delete_var` with `var` field defines that variable.
                if matches!(op.kind.as_str(), "store_var" | "delete_var")
                    && let Some(v) = &op.var
                    && is_variable(v)
                {
                    defs.insert(v.clone());
                    self.all_vars.insert(v.clone());
                }
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

        for var in self.all_vars.clone() {
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
            let mut worklist: VecDeque<usize> = def_blocks.iter().copied().collect();
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
            for &bid in &phi_blocks {
                if live_in[bid].contains(&var) && !self.block_arg_vars[bid].contains(&var) {
                    self.block_arg_vars[bid].push(var.clone());
                }
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
    let rpo = rpo_from(n, entry, successors);
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

fn rpo_from(n: usize, entry: usize, successors: &[Vec<usize>]) -> Vec<usize> {
    let mut visited = vec![false; n];
    let mut postorder = Vec::with_capacity(n);
    let mut stack: Vec<(usize, bool)> = vec![(entry, false)];
    while let Some((node, processed)) = stack.pop() {
        if processed {
            postorder.push(node);
            continue;
        }
        if visited[node] {
            continue;
        }
        visited[node] = true;
        stack.push((node, true));
        for &succ in successors[node].iter().rev() {
            if !visited[succ] {
                stack.push((succ, false));
            }
        }
    }
    postorder.reverse();
    postorder
}
