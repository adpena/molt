//! SSA conversion using the iterated dominance frontier algorithm.
//!
//! Converts a [`CFG`] (with mutable variable assignments from SimpleIR) into
//! SSA form using MLIR-style **block arguments** rather than phi nodes.
//!
//! Algorithm outline:
//! 1. Identify variable definition and use sites from `OpIR` ops.
//! 2. Compute dominance frontiers from the dominator tree in `CFG`.
//! 3. Insert block arguments at iterated dominance frontier blocks.
//! 4. Rename variables by walking the dominator tree, maintaining a
//!    per-variable definition stack.
//! 5. Thread renamed values through terminator branch arguments.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ir::OpIR;

use super::blocks::{BlockId, Terminator, TirBlock};
use super::cfg::CFG;
use super::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use super::types::TirType;
use super::values::{TirValue, ValueId};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Result of SSA conversion.
pub struct SsaOutput {
    /// TIR blocks in SSA form, indexed by block ordinal.
    pub blocks: Vec<TirBlock>,
    /// Type of every SSA value produced.
    pub types: HashMap<ValueId, TirType>,
    /// Next free ValueId counter (for subsequent passes).
    pub next_value: u32,
}

/// Convert a CFG together with its underlying `OpIR` slice into SSA-form
/// TIR blocks with MLIR-style block arguments at join points.
///
/// All values are typed as [`TirType::DynBox`] since type refinement is a
/// later pass.
pub fn convert_to_ssa(cfg: &CFG, ops: &[OpIR]) -> SsaOutput {
    convert_to_ssa_with_params(cfg, ops, &[])
}

/// SSA conversion with explicit function parameter names.
/// Parameters are treated as implicit definitions in the entry block.
pub fn convert_to_ssa_with_params(cfg: &CFG, ops: &[OpIR], params: &[String]) -> SsaOutput {
    let mut ctx = SsaContext::new(cfg, ops, params);
    ctx.run();
    ctx.into_output()
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Per-block bookkeeping used during construction.
struct BlockInfo {
    /// Variables that are defined (assigned to) in this CFG block.
    defs: HashSet<String>,
    /// Variables that are used (read) in this CFG block (retained for future liveness analysis).
    #[allow(dead_code)]
    uses: HashSet<String>,
    /// Ordered list of ops (index into the original `ops` slice).
    op_indices: Vec<usize>,
}

struct SsaContext<'a> {
    cfg: &'a CFG,
    ops: &'a [OpIR],
    /// Fresh value counter.
    next_value: u32,
    /// Per-block info.
    block_info: Vec<BlockInfo>,
    /// All variable names observed.
    all_vars: HashSet<String>,
    /// Dominance frontier sets, indexed by block id.
    dom_frontier: Vec<HashSet<usize>>,
    /// For each block, the ordered list of variable names that are block
    /// arguments (inserted during the phi-placement phase).
    block_arg_vars: Vec<Vec<String>>,
    /// The final TIR blocks.
    tir_blocks: Vec<TirBlock>,
    /// Value → type mapping.
    value_types: HashMap<ValueId, TirType>,
    /// Inline constant ops generated during translate_op (drained after each call).
    pending_inline_consts: Vec<super::ops::TirOp>,
    /// Function parameter names (treated as implicit entry-block definitions).
    params: Vec<String>,
    /// Shared `None` value used for known variables without a reaching def.
    undef_value: Option<ValueId>,
    /// Global iter_next fusion map: op_idx → (done_index_idx, val_index_idx,
    /// done_var, val_var).  Built by scanning the raw op stream BEFORE the
    /// CFG splits blocks, so the pattern spans across check_exception boundaries.
    iter_fuse_map: HashMap<usize, (usize, usize, String, String)>,
    /// Op indices to skip globally (fused into IterNextUnboxed).
    iter_fuse_skip: HashSet<usize>,
}

impl<'a> SsaContext<'a> {
    fn new(cfg: &'a CFG, ops: &'a [OpIR], params: &[String]) -> Self {
        let n = cfg.blocks.len();
        Self {
            cfg,
            ops,
            next_value: 0,
            block_info: Vec::with_capacity(n),
            all_vars: HashSet::new(),
            dom_frontier: vec![HashSet::new(); n],
            block_arg_vars: vec![Vec::new(); n],
            tir_blocks: Vec::new(),
            value_types: HashMap::new(),
            pending_inline_consts: Vec::new(),
            params: params.to_vec(),
            undef_value: None,
            iter_fuse_map: HashMap::new(),
            iter_fuse_skip: HashSet::new(),
        }
    }

    fn fresh_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    fn fresh_value_typed(&mut self) -> ValueId {
        let id = self.fresh_value();
        self.value_types.insert(id, TirType::DynBox);
        id
    }

    fn run(&mut self) {
        if self.cfg.blocks.is_empty() {
            return;
        }
        self.build_iter_fuse_map();
        self.gather_defs_uses();
        self.compute_dominance_frontiers();
        self.insert_block_arguments();
        self.insert_exception_handler_arguments();
        self.rename_and_emit();
    }

    /// Scan the raw linear op stream for iter_next → index(pair,1) →
    /// index(pair,0) patterns.  This runs BEFORE the CFG splits blocks at
    /// check_exception boundaries, so the pattern can span across them.
    fn build_iter_fuse_map(&mut self) {
        let ops = self.ops;
        for (i, op) in ops.iter().enumerate() {
            if op.kind != "iter_next" { continue; }
            let pair_var = match &op.out {
                Some(v) if v != "none" => v.clone(),
                _ => continue,
            };
            let mut done_idx = None;
            let mut val_idx = None;
            let scan_end = (i + 20).min(ops.len());
            for j in (i + 1)..scan_end {
                let scan_op = &ops[j];
                if scan_op.kind == "index" {
                    if let Some(args) = &scan_op.args {
                        if args.len() >= 2 && args[0] == pair_var {
                            let idx_name = &args[1];
                            let const_val = ops[..j].iter().rev().take(20)
                                .find_map(|c| {
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
                }
            }
            if let (Some(di), Some(vi)) = (done_idx, val_idx) {
                let done_var = ops[di].out.clone().unwrap_or_default();
                let val_var = ops[vi].out.clone().unwrap_or_default();
                self.iter_fuse_map.insert(i, (di, vi, done_var, val_var));
                // Skip all ops between iter_next and the value index.
                let skip_end = di.max(vi);
                for skip in (i + 1)..=skip_end {
                    self.iter_fuse_skip.insert(skip);
                }
            }
        }
    }

    // -- Phase 1: gather variable defs and uses per block --------------------

    fn gather_defs_uses(&mut self) {
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
                    if op.kind != "store_var" && !defs.contains(v) {
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
                // `store_var` with `var` field is a definition of that variable.
                if op.kind == "store_var"
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

    // -- Phase 2: dominance frontiers ----------------------------------------

    fn compute_dominance_frontiers(&mut self) {
        let n = self.cfg.blocks.len();
        for b in 0..n {
            for &pred in &self.cfg.predecessors[b] {
                let mut runner = pred;
                // Walk up the dominator tree from `pred` until we reach
                // the immediate dominator of `b` (exclusive).
                loop {
                    // `b` is in DF(runner) if runner doesn't strictly dominate b.
                    // runner dominates pred (or runner==pred), and b has pred as
                    // a predecessor. runner strictly dominates b only if
                    // runner == idom chain ancestor strictly.
                    if Some(runner) == self.cfg.dominators[b] {
                        // runner strictly dominates b — stop.
                        break;
                    }
                    // runner == b is also possible in loop headers.
                    if runner == b && self.cfg.dominators[b].is_none() && b == self.cfg.entry {
                        break;
                    }
                    self.dom_frontier[runner].insert(b);
                    match self.cfg.dominators[runner] {
                        Some(idom) if idom != runner => runner = idom,
                        _ => break,
                    }
                }
            }
        }
    }

    // -- Phase 3: insert block arguments (phi placement) ---------------------

    fn insert_block_arguments(&mut self) {
        // For each variable, compute the iterated dominance frontier of all
        // blocks that define it, then insert a block argument at those blocks.
        // This is pruned SSA: only insert a block argument when the variable is
        // actually live-in to the join block. Otherwise dead branch-local vars
        // create bogus block params and unresolved predecessor values.
        let live_in = self.compute_live_in_vars(false);

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
                if self.block_info[bid].defs.contains(&var) {
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
    fn insert_exception_handler_arguments(&mut self) {
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

    // -- Phase 4: rename variables and emit TIR blocks -----------------------

    fn rename_and_emit(&mut self) {
        let n = self.cfg.blocks.len();
        let undef_vid = self.fresh_value_typed();
        self.undef_value = Some(undef_vid);

        // Build dominator tree children.
        let mut dom_children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for bid in 0..n {
            if let Some(idom) = self.cfg.dominators[bid] {
                dom_children[idom].push(bid);
            }
        }

        // Per-variable definition stacks.
        let mut var_stacks: HashMap<String, Vec<ValueId>> = HashMap::new();

        // Pre-allocate TIR blocks with empty terminators. We fill them in during
        // the rename walk.
        let mut tir_blocks: Vec<TirBlock> = Vec::with_capacity(n);
        for bid in 0..n {
            tir_blocks.push(TirBlock {
                id: BlockId(bid as u32),
                args: Vec::new(),
                ops: Vec::new(),
                terminator: Terminator::Unreachable,
            });
        }

        // Track how many definitions each block pushed (for stack cleanup).
        // Map: block_id → vec of (var_name, count_pushed).
        let mut pushed: Vec<Vec<(String, usize)>> = vec![Vec::new(); n];

        // Iterative dominator-tree walk (pre-order DFS).
        let mut stack: Vec<(usize, bool)> = vec![(self.cfg.entry, false)];

        while let Some((bid, is_exit)) = stack.pop() {
            if is_exit {
                // Pop definitions pushed by this block.
                for (var, count) in &pushed[bid] {
                    if let Some(s) = var_stacks.get_mut(var) {
                        for _ in 0..*count {
                            s.pop();
                        }
                    }
                }
                continue;
            }

            // Push exit marker.
            stack.push((bid, true));

            let mut block_pushed: Vec<(String, usize)> = Vec::new();

            // 1. Create ValueIds for block arguments.
            let arg_vars = self.block_arg_vars[bid].clone();
            for var in &arg_vars {
                let vid = self.fresh_value_typed();
                tir_blocks[bid].args.push(TirValue {
                    id: vid,
                    ty: TirType::DynBox,
                });
                // Push onto the variable's definition stack.
                var_stacks.entry(var.clone()).or_default().push(vid);
                // Track push.
                let entry = block_pushed.iter_mut().find(|(v, _)| v == var);
                if let Some((_, c)) = entry {
                    *c += 1;
                } else {
                    block_pushed.push((var.clone(), 1));
                }
            }

            // 2. Process ops in this block.
            if bid == self.cfg.entry {
                tir_blocks[bid].ops.push(TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstNone,
                    operands: vec![],
                    results: vec![undef_vid],
                    attrs: AttrDict::new(),
                    source_span: None,
                });
            }
            let op_indices = self.block_info[bid].op_indices.clone();

            for &op_idx in &op_indices {
                // Skip ops globally fused into IterNextUnboxed.
                if self.iter_fuse_skip.contains(&op_idx) {
                    continue;
                }
                let op = &self.ops[op_idx];

                // Fused iter_next_unboxed: emit a single TIR op with two results
                // (value, done_flag) instead of iter_next + 2x index.
                if let Some(fuse_entry) = self.iter_fuse_map.get(&op_idx) {
                    let done_var = fuse_entry.2.clone();
                    let val_var = fuse_entry.3.clone();
                    let iter_vid = op.args.as_ref()
                        .and_then(|a| a.first())
                        .and_then(|a| Self::resolve_var(a, &var_stacks))
                        .or(self.undef_value)
                        .expect("iter arg not found");
                    let val_vid = self.fresh_value_typed();
                    let done_vid = self.fresh_value_typed();
                    let mut attrs = AttrDict::new();
                    attrs.insert("_original_kind".into(), AttrValue::Str("iter_next".into()));
                    let tir_op = TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::IterNextUnboxed,
                        operands: vec![iter_vid],
                        results: vec![val_vid, done_vid],
                        attrs,
                        source_span: None,
                    };
                    // Push val and done onto their variable stacks.
                    var_stacks.entry(val_var.clone()).or_default().push(val_vid);
                    block_pushed.iter_mut().find(|(v,_)| v == &val_var)
                        .map(|(_,c)| *c += 1)
                        .unwrap_or_else(|| block_pushed.push((val_var.clone(), 1)));
                    var_stacks.entry(done_var.clone()).or_default().push(done_vid);
                    block_pushed.iter_mut().find(|(v,_)| v == &done_var)
                        .map(|(_,c)| *c += 1)
                        .unwrap_or_else(|| block_pushed.push((done_var.clone(), 1)));
                    // Also push the pair var (referenced by loop_break_if_true).
                    let pair_var = op.out.clone().unwrap_or_default();
                    if !pair_var.is_empty() && pair_var != "none" {
                        var_stacks.entry(pair_var.clone()).or_default().push(done_vid);
                        block_pushed.iter_mut().find(|(v,_)| v == &pair_var)
                            .map(|(_,c)| *c += 1)
                            .unwrap_or_else(|| block_pushed.push((pair_var, 1)));
                    }
                    for const_op in self.pending_inline_consts.drain(..) {
                        tir_blocks[bid].ops.push(const_op);
                    }
                    tir_blocks[bid].ops.push(tir_op);
                    continue;
                }

                let tir_op = self.translate_op(op, &var_stacks);

                for (idx, var) in self.get_def_vars(op).iter().enumerate() {
                    let vid = tir_op
                        .results
                        .get(idx)
                        .copied()
                        .unwrap_or_else(|| self.fresh_value_typed());
                    var_stacks.entry(var.clone()).or_default().push(vid);
                    let entry = block_pushed.iter_mut().find(|(v, _)| v == var);
                    if let Some((_, c)) = entry {
                        *c += 1;
                    } else {
                        block_pushed.push((var.clone(), 1));
                    }
                }

                // Push any inline constant ops generated for this op's args
                for const_op in self.pending_inline_consts.drain(..) {
                    tir_blocks[bid].ops.push(const_op);
                }
                tir_blocks[bid].ops.push(tir_op);
            }

            // 3. Build terminator for this block.
            let terminator = self.build_terminator(bid, &var_stacks);
            tir_blocks[bid].terminator = terminator;

            // Save pushed counts for cleanup.
            pushed[bid] = block_pushed;

            // Push dominator-tree children in reverse order for correct DFS.
            for &child in dom_children[bid].iter().rev() {
                stack.push((child, false));
            }
        }

        // Fill unreachable blocks (not visited during dom-tree walk) by
        // translating their ops without SSA renaming.  These are typically
        // exception handler blocks only reachable via implicit edges (e.g.
        // state_label blocks containing state_block_start/end).  Without
        // this, their ops would be silently dropped, causing the native
        // backend to crash on missing state_block_start / check_exception.
        {
            for bid in 0..n {
                // Skip if the block was already processed (has ops or a non-Unreachable
                // terminator, or is the entry block which may legitimately be empty).
                if bid == self.cfg.entry {
                    continue;
                }
                if !matches!(tir_blocks[bid].terminator, Terminator::Unreachable) {
                    continue;
                }
                if !tir_blocks[bid].ops.is_empty() {
                    continue;
                }
                // This block was not visited.  Translate its ops with
                // var_stacks seeded from the block arguments so that
                // (a) incoming branch args match block params, and
                // (b) any references inside the block resolve to the
                //     block's own arg values (avoiding dominance errors).
                let arg_vars = self.block_arg_vars[bid].clone();
                let mut local_stacks: HashMap<String, Vec<ValueId>> = HashMap::new();
                for var in &arg_vars {
                    let vid = self.fresh_value_typed();
                    tir_blocks[bid].args.push(TirValue {
                        id: vid,
                        ty: TirType::DynBox,
                    });
                    local_stacks.entry(var.clone()).or_default().push(vid);
                }
                // Insert a ConstNone "undef" value at the top of this
                // unreachable block.  Any variable reference that cannot be
                // resolved from `local_stacks` will fall back to this value
                // instead of ValueId(0) from ^bb0 (which would violate SSA
                // dominance since ^bb0 does not dominate unreachable blocks).
                let undef_vid = self.fresh_value_typed();
                tir_blocks[bid].ops.push(TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstNone,
                    operands: vec![],
                    results: vec![undef_vid],
                    attrs: AttrDict::new(),
                    source_span: None,
                });

                // Seed local_stacks with the undef value for every known
                // variable that doesn't already have a definition (from block
                // args).  This ensures resolve_var never fails and falls back
                // to ValueId(0).
                for var in &self.all_vars.clone() {
                    local_stacks
                        .entry(var.clone())
                        .or_insert_with(|| vec![undef_vid]);
                }

                let op_indices = self.block_info[bid].op_indices.clone();
                for &op_idx in &op_indices {
                    let op = &self.ops[op_idx];
                    let tir_op = self.translate_op(op, &local_stacks);

                    for (idx, var) in self.get_def_vars(op).iter().enumerate() {
                        let vid = tir_op
                            .results
                            .get(idx)
                            .copied()
                            .unwrap_or_else(|| self.fresh_value_typed());
                        local_stacks.entry(var.clone()).or_default().push(vid);
                    }

                    for const_op in self.pending_inline_consts.drain(..) {
                        tir_blocks[bid].ops.push(const_op);
                    }
                    tir_blocks[bid].ops.push(tir_op);
                }
                // Build terminator for this unreachable block.
                let terminator = self.build_terminator(bid, &local_stacks);
                tir_blocks[bid].terminator = terminator;
            }
        }

        self.tir_blocks = tir_blocks;
    }

    /// Get the variable name being defined by an op, if any.
    ///
    /// Side-effect-only ops (set_attr, store_index, del_attr, etc.) may have
    /// an `out` field in SimpleIR but should NOT produce a TIR result value.
    /// The verifier enforces StoreAttr/StoreIndex/DelAttr have 0 results.
    fn get_def_var(&self, op: &OpIR) -> Option<String> {
        if op.kind == "store_var" {
            return op.var.clone().filter(|v| is_variable(v));
        }
        // Side-effect-only ops: no result value even if `out` is set.
        if matches!(
            op.kind.as_str(),
            "set_attr"
                | "store_attr"
                | "set_attr_name"
                | "set_attr_generic_ptr"
                | "set_attr_generic_obj"
                | "guarded_field_set"
                | "guarded_field_set_init"
                | "store"
                | "store_init"
                | "store_index"
                | "index_set"
                | "del_attr"
                | "del_attr_generic_ptr"
                | "del_attr_generic_obj"
                | "del_index"
                | "raise"
                | "raise_from"
                | "inc_ref"
                | "dec_ref"
        ) {
            return None;
        }
        op.out.clone().filter(|v| is_variable(v))
    }

    fn get_def_vars(&self, op: &OpIR) -> Vec<String> {
        if op.kind == "unpack_sequence" {
            return op
                .args
                .as_ref()
                .map(|args| {
                    args.iter()
                        .skip(1)
                        .filter(|v| is_variable(v))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
        }
        self.get_def_var(op).into_iter().collect()
    }

    /// Resolve a variable name to its current SSA ValueId.
    fn resolve_var(var: &str, var_stacks: &HashMap<String, Vec<ValueId>>) -> Option<ValueId> {
        var_stacks.get(var).and_then(|s| s.last().copied())
    }

    fn resolve_known_var(
        &self,
        var: &str,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> Option<ValueId> {
        Self::resolve_var(var, var_stacks).or_else(|| {
            if self.all_vars.contains(var) {
                self.undef_value
            } else {
                None
            }
        })
    }

    fn block_for_label(&self, label_id: i64) -> Option<usize> {
        self.cfg.blocks.iter().enumerate().find_map(|(bid, block)| {
            let has_label = (block.start_op..block.end_op).any(|op_idx| {
                let op = &self.ops[op_idx];
                matches!(op.kind.as_str(), "label" | "state_label") && op.value == Some(label_id)
            });
            has_label.then_some(bid)
        })
    }

    /// Translate a single SimpleIR op into a TIR op.
    fn translate_op(&mut self, op: &OpIR, var_stacks: &HashMap<String, Vec<ValueId>>) -> TirOp {
        // Resolve operands from args.
        // SimpleIR args can be variable names OR inline constants (e.g., "1", "3.14").
        // Variables resolve via var_stacks; constants get a fresh ConstInt/ConstFloat value.
        let mut operands = Vec::new();
        if let Some(args) = &op.args {
            let args_iter: Box<dyn Iterator<Item = &String> + '_> = if op.kind == "unpack_sequence" {
                Box::new(args.iter().take(1))
            } else {
                Box::new(args.iter())
            };
            for a in args_iter {
                if let Some(vid) = self.resolve_known_var(a, var_stacks) {
                    // Resolved as a variable
                    operands.push(vid);
                } else if let Ok(int_val) = a.parse::<i64>() {
                    // Inline integer constant — emit a ConstInt op before the current op
                    let vid = self.fresh_value_typed();
                    let mut attrs = super::ops::AttrDict::new();
                    attrs.insert("value".into(), super::ops::AttrValue::Int(int_val));
                    self.pending_inline_consts.push(super::ops::TirOp {
                        dialect: super::ops::Dialect::Molt,
                        opcode: super::ops::OpCode::ConstInt,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    });
                    operands.push(vid);
                } else if let Ok(float_val) = a.parse::<f64>() {
                    // Inline float constant
                    let vid = self.fresh_value_typed();
                    let mut attrs = super::ops::AttrDict::new();
                    attrs.insert("f_value".into(), super::ops::AttrValue::Float(float_val));
                    self.pending_inline_consts.push(super::ops::TirOp {
                        dialect: super::ops::Dialect::Molt,
                        opcode: super::ops::OpCode::ConstFloat,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    });
                    operands.push(vid);
                } else {
                    // Unresolved non-numeric arg — treat as string constant
                    // (e.g., class names in isinstance, function names in call)
                    let vid = self.fresh_value_typed();
                    let mut attrs = super::ops::AttrDict::new();
                    attrs.insert("s_value".into(), super::ops::AttrValue::Str(a.clone()));
                    self.pending_inline_consts.push(super::ops::TirOp {
                        dialect: super::ops::Dialect::Molt,
                        opcode: super::ops::OpCode::ConstStr,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    });
                    operands.push(vid);
                }
            }
        }
        // If `var` is an input (not store_var), resolve it too.
        if op.kind != "store_var"
            && let Some(v) = &op.var
            && is_variable(v)
            && let Some(vid) = self.resolve_known_var(v, var_stacks)
        {
            operands.push(vid);
        }
        // For store_var, the source is in args.
        if op.kind == "store_var"
            && let Some(args) = &op.args
        {
            for a in args {
                if is_variable(a)
                    && let Some(vid) = self.resolve_known_var(a, var_stacks)
                {
                    operands.push(vid);
                }
            }
        }

        if op.kind == "check_exception"
            && let Some(label_id) = op.value
            && let Some(target_bid) = self.block_for_label(label_id)
        {
            operands.extend(self.collect_branch_args(target_bid, var_stacks));
        }

        // Create result value if this op produces an output.
        let mut results = Vec::new();
        for _ in self.get_def_vars(op) {
            let vid = self.fresh_value_typed();
            results.push(vid);
        }

        // Build attrs from literal values on the op.
        let mut attrs = AttrDict::new();
        if let Some(v) = op.value {
            attrs.insert("value".into(), AttrValue::Int(v));
        }
        if let Some(v) = op.f_value {
            attrs.insert("f_value".into(), AttrValue::Float(v));
        }
        if let Some(ref v) = op.s_value {
            attrs.insert("s_value".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.bytes {
            attrs.insert("bytes".into(), AttrValue::Bytes(v.clone()));
        }
        // Preserve additional SimpleIR metadata fields that the native backend
        // reads on specific op kinds (task_kind, container_type, ic_index, var,
        // raw_int). Without these, passthrough ops lose critical information.
        if let Some(ref v) = op.task_kind {
            attrs.insert("task_kind".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.container_type {
            attrs.insert("container_type".into(), AttrValue::Str(v.clone()));
        }
        if let Some(v) = op.ic_index {
            attrs.insert("ic_index".into(), AttrValue::Int(v));
        }
        if op.raw_int == Some(true) {
            attrs.insert("raw_int".into(), AttrValue::Bool(true));
        }
        // Preserve fast_int / fast_float / type_hint so that the round-trip does
        // not lose type annotations even without the type-refine pass running.
        if op.fast_int == Some(true) {
            attrs.insert("_fast_int".into(), AttrValue::Bool(true));
        }
        if op.fast_float == Some(true) {
            attrs.insert("_fast_float".into(), AttrValue::Bool(true));
        }
        if let Some(ref th) = op.type_hint {
            attrs.insert("_type_hint".into(), AttrValue::Str(th.clone()));
        }

        let opcode = kind_to_opcode(&op.kind);

        // Opcode-specific attr key aliases: the lowering reads s_value under
        // different keys depending on the opcode (e.g., "module" for Import,
        // "name" for LoadAttr/StoreAttr/DelAttr/ImportFrom/CallBuiltin,
        // "method" for CallMethod).  Store it under the expected key so the
        // back-conversion finds it.
        if let Some(ref v) = op.s_value {
            match opcode {
                OpCode::Import => {
                    attrs.insert("module".into(), AttrValue::Str(v.clone()));
                }
                OpCode::ImportFrom
                | OpCode::LoadAttr
                | OpCode::StoreAttr
                | OpCode::DelAttr
                | OpCode::CallBuiltin => {
                    attrs.insert("name".into(), AttrValue::Str(v.clone()));
                }
                OpCode::CallMethod => {
                    attrs.insert("method".into(), AttrValue::Str(v.clone()));
                }
                _ => {}
            }
        }

        // Preserve the `var` field for passthrough (Copy) ops where `var` carries
        // metadata (a non-variable string) rather than an SSA reference.  For
        // such ops, the var was NOT resolved to an operand above (it was either
        // not a variable or failed resolution), so we store it as an attr.
        if op.kind != "store_var"
            && op.kind != "load_var"
            && op.kind != "copy_var"
            && let Some(ref v) = op.var
        {
            // Only store as attr if it wasn't already resolved as an SSA operand.
            // We can detect this: if the var is a variable name that resolved,
            // it's already in operands; if it's not a variable or didn't resolve,
            // we need to preserve it as an attr.
            attrs.insert("_var".into(), AttrValue::Str(v.clone()));
        }

        // For ops that map to OpCode::Copy as a fallback (unknown ops),
        // preserve the original kind string so the back-conversion can
        // emit the correct SimpleIR op.
        if opcode == OpCode::Copy
            && !matches!(
                op.kind.as_str(),
                "copy" | "store_var" | "load_var" | "copy_var"
            )
        {
            attrs.insert("_original_kind".into(), AttrValue::Str(op.kind.clone()));
        }

        // For call variants that are not literally "call", preserve the
        // original kind so the lowering back to SimpleIR emits the correct
        // op kind (call_func, call_indirect, call_bind, etc.).
        if opcode == OpCode::Call && op.kind != "call" {
            attrs.insert("_original_kind".into(), AttrValue::Str(op.kind.clone()));
        }
        if opcode == OpCode::CallBuiltin && !matches!(op.kind.as_str(), "call_builtin") {
            attrs.insert("_original_kind".into(), AttrValue::Str(op.kind.clone()));
        }
        // Preserve original kind for attr/index ops that have backend-specific
        // variants (get_attr_generic_obj, set_attr_generic_obj, store_index, etc.).
        if matches!(
            opcode,
            OpCode::LoadAttr
                | OpCode::StoreAttr
                | OpCode::Index
                | OpCode::StoreIndex
                | OpCode::DelIndex
                | OpCode::DelAttr
        ) && !matches!(
            op.kind.as_str(),
            "get_attr" | "set_attr" | "index" | "store_index" | "del_index" | "del_attr"
        ) {
            attrs.insert("_original_kind".into(), AttrValue::Str(op.kind.clone()));
        }

        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    /// Build the terminator for a given CFG block.
    fn build_terminator(
        &mut self,
        bid: usize,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> Terminator {
        let bb = &self.cfg.blocks[bid];
        let last_op_idx = bb.end_op.saturating_sub(1);
        let last_op = if bb.start_op < bb.end_op {
            Some(&self.ops[last_op_idx])
        } else {
            None
        };

        let succs = &self.cfg.successors[bid];

        // Determine terminator kind from the last op.
        let kind = last_op.map(|o| o.kind.as_str()).unwrap_or("");

        match kind {
            "ret" | "ret_void" | "return" => {
                let mut values = Vec::new();
                if (kind == "ret" || kind == "return")
                    && let Some(op) = last_op
                {
                    // The frontend emits `ret` with the return value in
                    // `op.var` (not `op.args`).  Check both locations so
                    // the return value is never silently dropped.
                    let mut candidates: Vec<&String> = Vec::new();
                    if let Some(ref v) = op.var {
                        candidates.push(v);
                    }
                    if let Some(ref args) = op.args {
                        for a in args {
                            candidates.push(a);
                        }
                    }
                    for a in candidates {
                        if is_variable(a)
                            && let Some(vid) = self.resolve_known_var(a, var_stacks)
                        {
                            values.push(vid);
                        }
                    }
                }
                Terminator::Return { values }
            }

            "jump" | "goto" | "loop_break" => {
                if let Some(&target_bid) = succs.first() {
                    let args = self.collect_branch_args(target_bid, var_stacks);
                    Terminator::Branch {
                        target: BlockId(target_bid as u32),
                        args,
                    }
                } else {
                    Terminator::Unreachable
                }
            }

            "if" | "br_if" | "loop_break_if_true" | "loop_break_if_false" => {
                // Resolve the condition.
                let cond = last_op
                    .and_then(|op| {
                        op.args.as_ref().and_then(|a| a.first()).and_then(|a| {
                            if is_variable(a) {
                                self.resolve_known_var(a, var_stacks)
                            } else {
                                None
                            }
                        })
                    })
                    .or(self.undef_value)
                    .unwrap_or_else(|| {
                        self.undef_value.expect("SSA undef value must be initialized")
                    });

                if succs.len() >= 2 {
                    let then_bid = succs[0];
                    let else_bid = succs[1];
                    let then_args = self.collect_branch_args(then_bid, var_stacks);
                    let else_args = self.collect_branch_args(else_bid, var_stacks);
                    Terminator::CondBranch {
                        cond,
                        then_block: BlockId(then_bid as u32),
                        then_args,
                        else_block: BlockId(else_bid as u32),
                        else_args,
                    }
                } else if succs.len() == 1 {
                    let target_bid = succs[0];
                    let args = self.collect_branch_args(target_bid, var_stacks);
                    Terminator::Branch {
                        target: BlockId(target_bid as u32),
                        args,
                    }
                } else {
                    Terminator::Unreachable
                }
            }

            "check_exception" => {
                // check_exception terminates the block with an implicit branch
                // to both the fallthrough (no exception) and the exception
                // handler. The check_exception OP itself (emitted in the block
                // body) carries the handler branch args via its operands
                // (see translate_op). The terminator only needs to branch to
                // the fallthrough successor — the handler edge is implicit.
                //
                // We also store args for the handler block here so that when
                // lower_to_simple emits the fallthrough jump, the handler
                // block's arguments are still correct.
                if !succs.is_empty() {
                    let target_bid = succs[0];
                    let args = self.collect_branch_args(target_bid, var_stacks);
                    Terminator::Branch {
                        target: BlockId(target_bid as u32),
                        args,
                    }
                } else {
                    Terminator::Unreachable
                }
            }

            _ => {
                // Default: fall-through to successor(s).
                match succs.len() {
                    0 => Terminator::Unreachable,
                    1 => {
                        let target_bid = succs[0];
                        let args = self.collect_branch_args(target_bid, var_stacks);
                        Terminator::Branch {
                            target: BlockId(target_bid as u32),
                            args,
                        }
                    }
                    _ => {
                        // Multiple successors from a non-branch op — branch to
                        // fallthrough, exception handler args are passed via the
                        // check_exception op's inline branch arguments.
                        let target_bid = succs[0];
                        let args = self.collect_branch_args(target_bid, var_stacks);
                        Terminator::Branch {
                            target: BlockId(target_bid as u32),
                            args,
                        }
                    }
                }
            }
        }
    }

    /// Collect the branch argument values for a target block based on its
    /// block argument variable list and the current variable stacks.
    fn collect_branch_args(
        &self,
        target_bid: usize,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> Vec<ValueId> {
        self.block_arg_vars[target_bid]
            .iter()
            .map(|var| {
                // Use the current stack-top definition for this variable.
                // If the variable has no reaching definition on this path
                // (e.g., a loop-body variable at the loop entry edge), use
                // the shared undef value.  This is correct SSA semantics:
                // on the first iteration the value is undefined, and the
                // loop header's phi merges undef (entry edge) with the
                // actual value (back-edge).
                self.resolve_known_var(var, var_stacks)
                    .or(self.undef_value)
                    .expect("SSA undef value must be initialized")
            })
            .collect()
    }

    fn into_output(self) -> SsaOutput {
        SsaOutput {
            blocks: self.tir_blocks,
            types: self.value_types,
            next_value: self.next_value,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns true if the name looks like a SimpleIR variable (not a special
/// keyword like "none").
fn is_variable(name: &str) -> bool {
    !name.is_empty() && name != "none" && name != "True" && name != "False"
}

// Use shared is_structural from parent module (ensures SSA and lower_from_simple
// always agree on which ops to skip).
use super::is_structural;

/// Map a SimpleIR `kind` string to a TIR `OpCode`.
fn kind_to_opcode(kind: &str) -> OpCode {
    match kind {
        "add" => OpCode::Add,
        "sub" => OpCode::Sub,
        "mul" => OpCode::Mul,
        "inplace_add" => OpCode::InplaceAdd,
        "inplace_sub" => OpCode::InplaceSub,
        "inplace_mul" => OpCode::InplaceMul,
        "div" => OpCode::Div,
        "floor_div" => OpCode::FloorDiv,
        "mod" => OpCode::Mod,
        "pow" => OpCode::Pow,
        "neg" => OpCode::Neg,
        "pos" => OpCode::Pos,
        "eq" => OpCode::Eq,
        "ne" => OpCode::Ne,
        "lt" => OpCode::Lt,
        "le" => OpCode::Le,
        "gt" => OpCode::Gt,
        "ge" => OpCode::Ge,
        "is" => OpCode::Is,
        "is_not" => OpCode::IsNot,
        "in" => OpCode::In,
        "not_in" => OpCode::NotIn,
        "bit_and" => OpCode::BitAnd,
        "bit_or" => OpCode::BitOr,
        "bit_xor" => OpCode::BitXor,
        "bit_not" => OpCode::BitNot,
        "shl" | "lshift" => OpCode::Shl,
        "shr" | "rshift" => OpCode::Shr,
        "and" => OpCode::And,
        "or" => OpCode::Or,
        "not" => OpCode::Not,
        "alloc" => OpCode::Alloc,
        "stack_alloc" => OpCode::StackAlloc,
        "free" => OpCode::Free,
        "get_attr"
        | "get_attr_generic_ptr"
        | "get_attr_generic_obj"
        | "get_attr_name"
        | "guarded_field_get"
        | "load"
        | "load_attr" => OpCode::LoadAttr,
        "set_attr"
        | "store_attr"
        | "set_attr_name"
        | "set_attr_generic_ptr"
        | "set_attr_generic_obj"
        | "guarded_field_set"
        | "guarded_field_set_init"
        | "store"
        | "store_init" => OpCode::StoreAttr,
        "del_attr" | "del_attr_generic_ptr" | "del_attr_generic_obj" => OpCode::DelAttr,
        "index" => OpCode::Index,
        "store_index" | "index_set" => OpCode::StoreIndex,
        "del_index" => OpCode::DelIndex,
        "call" | "call_func" | "call_internal" | "call_indirect" | "call_bind"
        | "call_function" | "call_guarded" | "invoke_ffi" => OpCode::Call,
        "call_method" => OpCode::CallMethod,
        "call_builtin" | "builtin_print" | "print" => OpCode::CallBuiltin,
        "box" | "box_from_raw_int" => OpCode::BoxVal,
        "unbox" | "unbox_to_raw_int" => OpCode::UnboxVal,
        "type_guard" => OpCode::TypeGuard,
        "inc_ref" => OpCode::IncRef,
        "dec_ref" => OpCode::DecRef,
        "build_list" => OpCode::BuildList,
        "build_dict" => OpCode::BuildDict,
        "build_tuple" => OpCode::BuildTuple,
        "build_set" => OpCode::BuildSet,
        "build_slice" => OpCode::BuildSlice,
        "get_iter" => OpCode::GetIter,
        "iter_next" => OpCode::IterNext,
        "for_iter" => OpCode::ForIter,
        "yield" => OpCode::Yield,
        "yield_from" => OpCode::YieldFrom,
        "raise" => OpCode::Raise,
        "check_exception" => OpCode::CheckException,
        "try_start" => OpCode::TryStart,
        "try_end" => OpCode::TryEnd,
        "state_block_start" => OpCode::StateBlockStart,
        "state_block_end" => OpCode::StateBlockEnd,
        "const" | "const_int" | "load_const" => OpCode::ConstInt,
        "const_float" => OpCode::ConstFloat,
        "const_str" => OpCode::ConstStr,
        "const_bool" => OpCode::ConstBool,
        "const_none" => OpCode::ConstNone,
        "const_bytes" => OpCode::ConstBytes,
        "copy" | "store_var" | "load_var" => OpCode::Copy,
        "import" => OpCode::Import,
        "import_from" => OpCode::ImportFrom,
        // Fallback for unknown ops.
        _ => OpCode::Copy,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::OpIR;
    use crate::tir::cfg::CFG;
    use std::collections::HashSet;

    /// Helper to create an `OpIR` with just a `kind`.
    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind`, `args`, and `out`.
    fn op_args_out(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind` and `args`.
    fn op_args(kind: &str, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind` and `value`.
    fn op_val(kind: &str, value: i64) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value: Some(value),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind`, `out`, and `value`.
    fn op_val_out(kind: &str, value: i64, out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value: Some(value),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    // Helper: count block arguments across all blocks.
    fn total_block_args(output: &SsaOutput) -> usize {
        output.blocks.iter().map(|b| b.args.len()).sum()
    }

    // Helper: collect all unique ValueIds from block arguments and op results.
    fn all_value_ids(output: &SsaOutput) -> HashSet<ValueId> {
        let mut ids = HashSet::new();
        for block in &output.blocks {
            for arg in &block.args {
                ids.insert(arg.id);
            }
            for op in &block.ops {
                for &r in &op.results {
                    ids.insert(r);
                }
            }
        }
        ids
    }

    // =======================================================================
    // Test 1: Straight-line code — no block arguments needed
    // =======================================================================
    #[test]
    fn straight_line_no_block_args() {
        // x = 1; y = x + 1
        let ops = vec![
            op_val_out("const", 1, "x"),     // x = 1
            op_args_out("add", &["x"], "y"), // y = x + 1 (simplified)
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        assert_eq!(output.blocks.len(), 1, "straight-line code = 1 block");
        assert_eq!(
            total_block_args(&output),
            0,
            "no join points → no block args"
        );

        // Two ops that define variables → two distinct ValueIds in results.
        let ids = all_value_ids(&output);
        assert!(
            ids.len() >= 2,
            "need at least 2 ValueIds for x and y, got {}",
            ids.len()
        );

        // Check all values are typed.
        for id in &ids {
            assert!(
                output.types.contains_key(id),
                "ValueId {:?} should have a type",
                id
            );
        }
    }

    // =======================================================================
    // Test 2: Join point — if/else assigns x, merge needs block arg
    // =======================================================================
    #[test]
    fn join_point_block_argument() {
        // if c: x = 1 else: x = 2; ret
        //
        // SimpleIR layout:
        //   const c → v0          (block 0)
        //   if [v0]               (block 0, ends it)
        //   const 1 → x           (block 1: then)
        //   else                   (block 2: else)
        //   const 2 → x           (block 2: else body)
        //   end_if                 (block 3: join)
        //   ret [x]               (block 3: join)
        let ops = vec![
            op_val_out("const", 0, "v0"), // 0
            op_args("if", &["v0"]),       // 1 — terminates block 0
            op_val_out("const", 1, "x"),  // 2 — then block
            op("else"),                   // 3 — else block start
            op_val_out("const", 2, "x"),  // 4 — else body
            op("end_if"),                 // 5 — join block
            op_args("ret", &["x"]),       // 6 — return x
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        assert!(
            total_block_args(&output) > 0,
            "SSA conversion should insert a merge arg for x, got 0 total block args"
        );
        assert_eq!(
            total_block_args(&output),
            1,
            "exactly one merged block arg should exist for x"
        );
    }

    // =======================================================================
    // Test 3: Loop variable — block arg at loop header
    // =======================================================================
    #[test]
    fn loop_variable_block_argument() {
        // x = 0; loop { x = x + 1 }
        //
        // SimpleIR:
        //   const 0 → x          (block 0)
        //   loop_start            (block 1: header)
        //   add [x] → x          (block 1: body, or block 2)
        //   loop_end              (block 2/3: back-edge)
        //   ret_void              (after loop)
        let ops = vec![
            op_val_out("const", 0, "x"),     // 0
            op("loop_start"),                // 1 — loop header
            op_args_out("add", &["x"], "x"), // 2 — x = x + 1
            op("loop_end"),                  // 3 — back-edge
            op("ret_void"),                  // 4 — after
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        // The loop header block should have a block argument for x.
        let header_bid = cfg
            .blocks
            .iter()
            .position(|b| b.start_op <= 1 && b.end_op > 1)
            .unwrap();
        let header_block = &output.blocks[header_bid];

        assert!(
            !header_block.args.is_empty(),
            "loop header should have block arg for x, got {} args",
            header_block.args.len()
        );
    }

    // =======================================================================
    // Test 4: Multiple variables — two block args at join
    // =======================================================================
    #[test]
    fn multiple_variables_at_join() {
        // if c: x = 1; y = 2 else: x = 3; y = 4; use x, y
        let ops = vec![
            op_val_out("const", 0, "v0"), // 0
            op_args("if", &["v0"]),       // 1
            op_val_out("const", 1, "x"),  // 2 then
            op_val_out("const", 2, "y"),  // 3 then
            op("else"),                   // 4 else
            op_val_out("const", 3, "x"),  // 5 else
            op_val_out("const", 4, "y"),  // 6 else
            op("end_if"),                 // 7 join
            op_args("ret", &["x", "y"]),  // 8
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        assert_eq!(
            total_block_args(&output),
            2,
            "SSA conversion should insert exactly 2 merged block args (x and y), got {}",
            total_block_args(&output)
        );

        // All block arg ValueIds should be unique.
        let arg_ids: HashSet<ValueId> = output
            .blocks
            .iter()
            .flat_map(|block| block.args.iter().map(|arg| arg.id))
            .collect();
        assert_eq!(arg_ids.len(), 2, "block arg ValueIds should be unique");
    }

    // =======================================================================
    // Test 5: Empty CFG
    // =======================================================================
    #[test]
    fn empty_cfg_produces_no_blocks() {
        let cfg = CFG::build(&[]);
        let output = convert_to_ssa(&cfg, &[]);
        assert!(output.blocks.is_empty());
    }

    // =======================================================================
    // Test 6: SSA property — each ValueId defined exactly once
    // =======================================================================
    #[test]
    fn ssa_property_unique_definitions() {
        let ops = vec![
            op_val_out("const", 0, "v0"),
            op_args("if", &["v0"]),
            op_val_out("const", 1, "x"),
            op("else"),
            op_val_out("const", 2, "x"),
            op("end_if"),
            op_args("ret", &["x"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        // Collect all definition sites (op results + block args).
        let mut def_ids = Vec::new();
        for block in &output.blocks {
            for arg in &block.args {
                def_ids.push(arg.id);
            }
            for op in &block.ops {
                for &r in &op.results {
                    def_ids.push(r);
                }
            }
        }

        // All ValueIds should be unique (SSA property).
        let unique: HashSet<ValueId> = def_ids.iter().copied().collect();
        assert_eq!(
            def_ids.len(),
            unique.len(),
            "SSA property violated: {} definitions but only {} unique ValueIds",
            def_ids.len(),
            unique.len()
        );
    }

    // =======================================================================
    // Test 7: Branch args match block arg count
    // =======================================================================
    #[test]
    fn branch_args_match_block_arg_count() {
        let ops = vec![
            op_val_out("const", 0, "v0"),
            op_args("if", &["v0"]),
            op_val_out("const", 1, "x"),
            op("else"),
            op_val_out("const", 2, "x"),
            op("end_if"),
            op_args("ret", &["x"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        // For every branch to a block, the number of passed args must match
        // the target block's argument count.
        let block_map: HashMap<BlockId, &TirBlock> =
            output.blocks.iter().map(|b| (b.id, b)).collect();

        for block in &output.blocks {
            match &block.terminator {
                Terminator::Branch { target, args } => {
                    if let Some(target_block) = block_map.get(target) {
                        assert_eq!(
                            args.len(),
                            target_block.args.len(),
                            "Branch from {} to {} passes {} args but target expects {}",
                            block.id,
                            target,
                            args.len(),
                            target_block.args.len()
                        );
                    }
                }
                Terminator::CondBranch {
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                    ..
                } => {
                    if let Some(tb) = block_map.get(then_block) {
                        assert_eq!(
                            then_args.len(),
                            tb.args.len(),
                            "CondBranch then from {} to {} passes {} args but target expects {}",
                            block.id,
                            then_block,
                            then_args.len(),
                            tb.args.len()
                        );
                    }
                    if let Some(eb) = block_map.get(else_block) {
                        assert_eq!(
                            else_args.len(),
                            eb.args.len(),
                            "CondBranch else from {} to {} passes {} args but target expects {}",
                            block.id,
                            else_block,
                            else_args.len(),
                            eb.args.len()
                        );
                    }
                }
                _ => {}
            }
        }
    }

    // =======================================================================
    // Test 8: Exception handlers should not receive dead future vars
    // =======================================================================
    #[test]
    fn check_exception_does_not_capture_future_dead_vars() {
        let ops = vec![
            op_val_out("const", 1, "x"),
            op_val("try_start", 100),
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(100),
                ..OpIR::default()
            },
            op_val_out("const", 2, "y"),
            op_val("try_end", 100),
            op("ret_void"),
            op_val("label", 100),
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let handler_bid = cfg
            .blocks
            .iter()
            .position(|b| b.start_op <= 6 && b.end_op > 6)
            .expect("handler block should exist");
        let handler_block = &output.blocks[handler_bid];
        assert!(
            handler_block.args.is_empty(),
            "handler block should not receive dead future vars, got {} args",
            handler_block.args.len()
        );

        let check_op = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.opcode == OpCode::CheckException)
            .expect("check_exception op should survive SSA conversion");
        assert!(
            check_op.operands.is_empty(),
            "check_exception should not carry dead handler args, got operands {:?}",
            check_op.operands
        );
    }

    #[test]
    fn check_exception_in_multi_block_try_does_not_capture_dead_vars() {
        let ops = vec![
            op_val_out("const", 1, "c"),
            op_val("try_start", 100),
            op_args("if", &["c"]),
            op_val_out("const", 1, "x"),
            op("else"),
            op_val_out("const", 2, "x"),
            op("end_if"),
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(100),
                ..OpIR::default()
            },
            op_val_out("const", 3, "y"),
            op_val("try_end", 100),
            op("ret_void"),
            op_val("label", 100),
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let handler_bid = cfg
            .blocks
            .iter()
            .position(|b| b.start_op <= 11 && b.end_op > 11)
            .expect("handler block should exist");
        let handler_block = &output.blocks[handler_bid];
        assert!(
            handler_block.args.is_empty(),
            "multi-block handler should not receive dead vars, got {} args",
            handler_block.args.len()
        );

        let check_ops: Vec<_> = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| op.opcode == OpCode::CheckException)
            .collect();
        assert_eq!(check_ops.len(), 1, "expected exactly one check_exception op");
        assert!(
            check_ops[0].operands.is_empty(),
            "check_exception should not carry dead handler args, got operands {:?}",
            check_ops[0].operands
        );
    }

    #[test]
    fn join_block_does_not_receive_dead_branch_only_vars() {
        let ops = vec![
            op_val_out("const", 1, "c"),
            op_args("if", &["c"]),
            op_val_out("const", 1, "x"),
            op("else"),
            op_val_out("const", 2, "y"),
            op("end_if"),
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let join_bid = cfg
            .blocks
            .iter()
            .position(|b| b.start_op <= 5 && b.end_op > 5)
            .expect("join block should exist");
        let join_block = &output.blocks[join_bid];
        assert!(
            join_block.args.is_empty(),
            "join block must not receive dead branch-only vars, got {} args",
            join_block.args.len()
        );
    }

    #[test]
    fn branch_defined_live_var_gets_none_on_missing_edge() {
        let ops = vec![
            op_val_out("const", 1, "c"),
            op_args("if", &["c"]),
            op_val_out("const", 1, "x"),
            op("end_if"),
            op_args("ret", &["x"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        assert_eq!(
            total_block_args(&output),
            1,
            "live branch-defined var should still require one merge arg"
        );

        let undef_vid = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.opcode == OpCode::ConstNone)
            .and_then(|op| op.results.first().copied())
            .expect("SSA should materialize a shared undef None value");

        let has_undef_branch_arg = output.blocks.iter().any(|block| match &block.terminator {
            Terminator::Branch { args, .. } => args.contains(&undef_vid),
            Terminator::CondBranch {
                then_args,
                else_args,
                ..
            } => then_args.contains(&undef_vid) || else_args.contains(&undef_vid),
            _ => false,
        });
        assert!(
            has_undef_branch_arg,
            "missing branch edge must pass the explicit undef None value"
        );
    }

    // =======================================================================
    // Test 9: All output values are typed as DynBox
    // =======================================================================
    #[test]
    fn all_values_typed_dynbox() {
        let ops = vec![
            op_val_out("const", 1, "x"),
            op_args_out("add", &["x"], "y"),
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        for (&vid, ty) in &output.types {
            assert_eq!(
                *ty,
                TirType::DynBox,
                "ValueId {:?} should be DynBox, got {:?}",
                vid,
                ty
            );
        }
    }
}
