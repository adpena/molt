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

use std::collections::{HashMap, HashSet};

use crate::ir::OpIR;

use super::blocks::{BlockId, Terminator, TirBlock};
use super::cfg::CFG;
use super::op_kinds_generated::{
    kind_to_opcode_table, opcode_ssa_s_value_attr_key_table,
    simpleir_kind_preserves_original_kind_for_ssa,
};
use super::ops::{AttrDict, AttrValue, Dialect, OpCode, SourceSite, TirOp};
use super::types::TirType;
use super::values::{TirValue, ValueId};

#[path = "ssa/placement.rs"]
mod placement;
#[path = "ssa/terminators.rs"]
mod terminators;

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
    convert_to_ssa_with_name_and_params("<unknown>", cfg, ops, &[])
}

/// SSA conversion with explicit function parameter names.
/// Parameters are treated as implicit definitions in the entry block.
pub fn convert_to_ssa_with_params(cfg: &CFG, ops: &[OpIR], params: &[String]) -> SsaOutput {
    convert_to_ssa_with_name_and_params("<unknown>", cfg, ops, params)
}

pub fn convert_to_ssa_with_name_and_params(
    func_name: &str,
    cfg: &CFG,
    ops: &[OpIR],
    params: &[String],
) -> SsaOutput {
    let mut ctx = SsaContext::new(func_name, cfg, ops, params);
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
    func_name: String,
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
    /// Source-site fact active at each SimpleIR op index, derived once from
    /// frontend `line` markers plus per-op source fields.
    source_sites: Vec<Option<SourceSite>>,
    /// Augmented predecessors: regular predecessors ∪ exception edges. Used
    /// for SSA-correct dominance analysis. Exception handler blocks are
    /// reached only via implicit exception edges; without folding those into
    /// the predecessor relation, the dominator tree treats them as
    /// unreachable and any post-handler join block (where handler exit
    /// merges back into normal flow) is not recognized as a true join.
    aug_predecessors: Vec<Vec<usize>>,
    /// Augmented immediate dominators computed from `aug_predecessors`.
    /// Indexed by block id; entry block's idom is `None`.
    aug_dominators: Vec<Option<usize>>,
}

impl<'a> SsaContext<'a> {
    fn new(func_name: &str, cfg: &'a CFG, ops: &'a [OpIR], params: &[String]) -> Self {
        let n = cfg.blocks.len();
        Self {
            func_name: func_name.to_string(),
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
            source_sites: Self::build_source_sites(ops),
            aug_predecessors: vec![Vec::new(); n],
            aug_dominators: vec![None; n],
        }
    }

    fn build_source_sites(ops: &[OpIR]) -> Vec<Option<SourceSite>> {
        let mut sites = Vec::with_capacity(ops.len());
        let mut active: Option<SourceSite> = None;
        for op in ops {
            let explicit_line = op
                .source_line
                .or_else(|| if op.kind == "line" { op.value } else { None });
            let mut site = explicit_line
                .and_then(|line| SourceSite::from_line_col(line, op.col_offset, op.end_col_offset))
                .or(active);
            if let (Some(current), Some(active_site)) = (&mut site, active)
                && current.line == active_site.line
            {
                if current.col.is_none() {
                    current.col = active_site.col;
                }
                if current.end_col.is_none() {
                    current.end_col = active_site.end_col;
                }
            }
            if op.kind == "line" && site.is_some() {
                active = site;
            }
            sites.push(site);
        }
        sites
    }

    fn source_site_for_op(&self, op_idx: usize) -> Option<SourceSite> {
        self.source_sites.get(op_idx).copied().flatten()
    }

    fn stamp_source_site(&self, tir_op: &mut TirOp, op_idx: usize) {
        if let Some(site) = self.source_site_for_op(op_idx) {
            tir_op.set_source_site(site);
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
        self.build_augmented_cfg();
        self.compute_dominance_frontiers();
        // Handler-block arguments must be established BEFORE the iterated
        // dominance frontier runs: a handler block that receives a live-in
        // variable as a block argument is a *new* SSA definition of that
        // variable (the handler re-enters the variable's live range along the
        // exception edge), so its dominance frontier — every point where the
        // handler's normal exit rejoins the protected region's control flow —
        // needs a phi. Seeding the IDF with handler blocks (below) is what
        // places those rejoin phis; running it afterward would leave the
        // rejoin merges phi-less and produce values that are defined on the
        // normal path but undefined on the handler path.
        self.insert_exception_handler_arguments();
        // State-machine resume continuations are reached via the implicit
        // `state_switch` dispatch edge; seed their block arguments before the
        // IDF runs, for the same reason handler-block arguments are seeded above
        // (each becomes a fresh SSA def whose rejoin frontier needs a phi).
        self.insert_state_resume_block_arguments();
        self.insert_block_arguments();
        self.rename_and_emit();
    }

    // -- Phase 4: rename variables and emit TIR blocks -----------------------

    fn rename_and_emit(&mut self) {
        let n = self.cfg.blocks.len();
        let undef_vid = self.fresh_value_typed();
        self.undef_value = Some(undef_vid);

        // Build dominator tree children from the *augmented* dominator tree.
        // Handler blocks are reached only via exception edges, so the regular
        // CFG sees them as unreachable (no idom). The augmented relation
        // makes them reachable and gives them a real idom, so the rename
        // walk visits them in dominance order alongside the success path.
        let mut dom_children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for bid in 0..n {
            if let Some(idom) = self.aug_dominators[bid] {
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

        // Per-block exit variable snapshots for second-pass branch arg resolution.
        let mut block_exit_vars: HashMap<usize, HashMap<String, ValueId>> = HashMap::new();

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
                    let iter_vid = op
                        .args
                        .as_ref()
                        .and_then(|a| a.first())
                        .and_then(|a| Self::resolve_var(a, &var_stacks))
                        .or(self.undef_value)
                        .expect("iter arg not found");
                    let val_vid = self.fresh_value_typed();
                    let done_vid = self.fresh_value_typed();
                    let mut attrs = AttrDict::new();
                    attrs.insert("_original_kind".into(), AttrValue::Str("iter_next".into()));
                    attrs.insert("_simple_result_0".into(), AttrValue::Str(val_var.clone()));
                    attrs.insert("_simple_result_1".into(), AttrValue::Str(done_var.clone()));
                    let mut tir_op = TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::IterNextUnboxed,
                        operands: vec![iter_vid],
                        results: vec![val_vid, done_vid],
                        attrs,
                        source_span: None,
                    };
                    self.stamp_source_site(&mut tir_op, op_idx);
                    // Push val and done onto their variable stacks.
                    var_stacks.entry(val_var.clone()).or_default().push(val_vid);
                    block_pushed
                        .iter_mut()
                        .find(|(v, _)| v == &val_var)
                        .map(|(_, c)| *c += 1)
                        .unwrap_or_else(|| block_pushed.push((val_var.clone(), 1)));
                    var_stacks
                        .entry(done_var.clone())
                        .or_default()
                        .push(done_vid);
                    block_pushed
                        .iter_mut()
                        .find(|(v, _)| v == &done_var)
                        .map(|(_, c)| *c += 1)
                        .unwrap_or_else(|| block_pushed.push((done_var.clone(), 1)));
                    // Also push the pair var (referenced by loop_break_if_true).
                    let pair_var = op.out.clone().unwrap_or_default();
                    if !pair_var.is_empty() && pair_var != "none" {
                        var_stacks
                            .entry(pair_var.clone())
                            .or_default()
                            .push(done_vid);
                        block_pushed
                            .iter_mut()
                            .find(|(v, _)| v == &pair_var)
                            .map(|(_, c)| *c += 1)
                            .unwrap_or_else(|| block_pushed.push((pair_var, 1)));
                    }
                    for const_op in self.pending_inline_consts.drain(..) {
                        tir_blocks[bid].ops.push(const_op);
                    }
                    tir_blocks[bid].ops.push(tir_op);
                    continue;
                }

                let tir_op = self.translate_op(op_idx, op, &var_stacks);

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

            // 3. Build terminator for this block.  `build_terminator` may
            //    append a pre-terminator body op (e.g. the `ExceptionPending`
            //    flag read consumed by a `loop_break_if_exception` CondBranch),
            //    so it borrows the block's op list mutably.
            let terminator = self.build_terminator(bid, &var_stacks, &mut tir_blocks[bid].ops);
            tir_blocks[bid].terminator = terminator;

            // Save the variable stacks snapshot for this block (used in
            // the second pass to resolve branch args from sibling branches).
            let mut block_vars: HashMap<String, ValueId> = HashMap::new();
            for (var, stack) in &var_stacks {
                if let Some(&top) = stack.last() {
                    block_vars.insert(var.clone(), top);
                }
            }

            // Save pushed counts for cleanup.
            pushed[bid] = block_pushed;

            // Push dominator-tree children in reverse order for correct DFS.
            for &child in dom_children[bid].iter().rev() {
                stack.push((child, false));
            }

            // Store the block's exit variable state for second-pass resolution.
            block_exit_vars.insert(bid, block_vars);
        }

        // ── Second pass: fix branch args that used undef_value ──
        // The dom-tree walk may have produced undef_value for branch args
        // when a sibling predecessor hadn't been visited yet.  Now that all
        // blocks are processed, resolve them using predecessor exit states.
        // Second pass: resolve undef branch args by walking the dominator
        // tree from each block upward.  The block_exit_vars snapshot contains
        // all variables visible at each block's exit (which dominate the block).
        for bid in 0..n {
            let terminator = &mut tir_blocks[bid].terminator;
            let fix_args = |args: &mut Vec<ValueId>, target_bid: usize| {
                let target_vars = &self.block_arg_vars[target_bid];
                for (i, arg) in args.iter_mut().enumerate() {
                    if *arg == undef_vid
                        && let Some(var_name) = target_vars.get(i)
                    {
                        // Walk dominator chain from bid upward to find
                        // the nearest dominator that defines this variable.
                        // Values in block_exit_vars[d] dominate d, and
                        // since d dominates bid, they also dominate bid.
                        // Use the augmented dominator tree so handler blocks
                        // (reached only via exception edges) participate.
                        let mut d = Some(bid);
                        while let Some(dom_bid) = d {
                            if let Some(vars) = block_exit_vars.get(&dom_bid)
                                && let Some(&val) = vars.get(var_name)
                                && val != undef_vid
                            {
                                *arg = val;
                                break;
                            }
                            let next = self.aug_dominators[dom_bid];
                            if next == Some(dom_bid) || next == d {
                                break; // entry block or cycle
                            }
                            d = next;
                        }
                    }
                }
            };
            match terminator {
                Terminator::Branch { target, args } => {
                    fix_args(args, target.0 as usize);
                }
                Terminator::CondBranch {
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                    ..
                } => {
                    fix_args(then_args, then_block.0 as usize);
                    fix_args(else_args, else_block.0 as usize);
                }
                _ => {}
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
                    let tir_op = self.translate_op(op_idx, op, &local_stacks);

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
                let terminator =
                    self.build_terminator(bid, &local_stacks, &mut tir_blocks[bid].ops);
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
        if matches!(op.kind.as_str(), "store_var" | "delete_var") {
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
                | "guarded_field_init"
                | "module_cache_set"
                | "module_cache_del"
                | "module_set_attr"
                | "module_del_global"
                | "module_del_global_if_present"
                | "store"
                | "store_init"
                | "store_index"
                | "index_set"
                | "del_attr"
                | "del_attr_name"
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
        // Two-result ops: `var` = results[0], `out` = results[1] (the
        // IterNextUnboxed transport convention; CheckedAdd/CheckedMul carry
        // var = wrapping sum/product, out = overflow flag).
        if op.kind == "iter_next_unboxed" || op.kind == "checked_add" || op.kind == "checked_mul" {
            let mut out = Vec::new();
            if let Some(var) = &op.var
                && is_variable(var)
            {
                out.push(var.clone());
            }
            if let Some(done) = &op.out
                && is_variable(done)
            {
                out.push(done.clone());
            }
            return out;
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

    /// Translate a single SimpleIR op into a TIR op.
    fn translate_op(
        &mut self,
        op_idx: usize,
        op: &OpIR,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> TirOp {
        // Resolve operands from args.
        // SimpleIR args can be variable names OR inline constants (e.g., "1", "3.14").
        // Variables resolve via var_stacks; constants get a fresh ConstInt/ConstFloat value.
        let mut operands = Vec::new();
        if let Some(args) = &op.args {
            let args_iter: Box<dyn Iterator<Item = &String> + '_> = if op.kind == "unpack_sequence"
            {
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
                    let mut const_op = super::ops::TirOp {
                        dialect: super::ops::Dialect::Molt,
                        opcode: super::ops::OpCode::ConstInt,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    };
                    self.stamp_source_site(&mut const_op, op_idx);
                    self.pending_inline_consts.push(const_op);
                    operands.push(vid);
                } else if let Ok(float_val) = a.parse::<f64>() {
                    // Inline float constant
                    let vid = self.fresh_value_typed();
                    let mut attrs = super::ops::AttrDict::new();
                    attrs.insert("f_value".into(), super::ops::AttrValue::Float(float_val));
                    let mut const_op = super::ops::TirOp {
                        dialect: super::ops::Dialect::Molt,
                        opcode: super::ops::OpCode::ConstFloat,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    };
                    self.stamp_source_site(&mut const_op, op_idx);
                    self.pending_inline_consts.push(const_op);
                    operands.push(vid);
                } else {
                    // Unresolved non-numeric arg — treat as string constant
                    // (e.g., class names in isinstance, function names in call)
                    let vid = self.fresh_value_typed();
                    let mut attrs = super::ops::AttrDict::new();
                    attrs.insert("s_value".into(), super::ops::AttrValue::Str(a.clone()));
                    let mut const_op = super::ops::TirOp {
                        dialect: super::ops::Dialect::Molt,
                        opcode: super::ops::OpCode::ConstStr,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    };
                    self.stamp_source_site(&mut const_op, op_idx);
                    self.pending_inline_consts.push(const_op);
                    operands.push(vid);
                }
            }
        }
        // If `var` is an input (not a local-slot mutation target or transport
        // spelling), resolve it too. For `copy_var`/`load_var`, an explicit
        // args[0] is the value source and `var` is local-name transport.
        if simple_var_field_is_value_operand(op)
            && let Some(v) = &op.var
            && is_variable(v)
            && let Some(vid) = self.resolve_known_var(v, var_stacks)
        {
            operands.push(vid);
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
            // ConstBool values must be stored as AttrValue::Bool so that
            // downstream passes (SCCP, canonicalize, GVN) can read the
            // boolean constant correctly.  The SSA lift previously stored
            // all values as AttrValue::Int, which made ConstBool(True)
            // and ConstBool(False) indistinguishable to passes that only
            // pattern-matched on AttrValue::Bool.
            if op.kind == "const_bool" {
                attrs.insert("value".into(), AttrValue::Bool(v != 0));
            } else {
                attrs.insert("value".into(), AttrValue::Int(v));
            }
        }
        if let Some(v) = op.f_value {
            attrs.insert("f_value".into(), AttrValue::Float(v));
        }
        if let Some(ref v) = op.s_value {
            attrs.insert("s_value".into(), AttrValue::Str(v.clone()));
        }
        if op.s_value.is_none()
            && let Some(symbol) = gpu_runtime_symbol_for_simple_kind(op.kind.as_str())
        {
            attrs.insert("s_value".into(), AttrValue::Str(symbol.to_string()));
        }
        if let Some(ref v) = op.bytes {
            attrs.insert("bytes".into(), AttrValue::Bytes(v.clone()));
        }
        // Preserve additional SimpleIR metadata fields that the native backend
        // reads on specific op kinds (task_kind, container_type, ic_index, var).
        // Without these, passthrough ops lose critical information.
        if let Some(ref v) = op.task_kind {
            attrs.insert("task_kind".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.container_type {
            attrs.insert("container_type".into(), AttrValue::Str(v.clone()));
        }
        if let Some(v) = op.ic_index {
            attrs.insert("ic_index".into(), AttrValue::Int(v));
        }
        if let Some(ref v) = op.effect_proof {
            attrs.insert("effect_proof".into(), AttrValue::Str(v.clone()));
        }
        // Finalizer fact for `object_new_bound`: the instance's class defines
        // `__del__` (frontend-resolved through the MRO, excluding `object`). The
        // escape pass reads this to keep the instance heap-allocated with a live
        // refcount — never stack-promoting it to an IMMORTAL object and never
        // stripping its IncRef/DecRef — so the finalizer-aware `dec_ref_ptr`
        // dispatches `__del__` at the last reference drop.
        if op.defines_del == Some(true) {
            attrs.insert("defines_del".into(), AttrValue::Bool(true));
        }
        // Named-local fact (#58): generic lift, same shape as `defines_del`.
        if op.bound_local == Some(true) {
            attrs.insert("bound_local".into(), AttrValue::Bool(true));
        }
        if let Some(ref out) = op.out {
            attrs.insert("_simple_out".into(), AttrValue::Str(out.clone()));
        }
        if op.kind == "iter_next_unboxed" || op.kind == "checked_add" || op.kind == "checked_mul" {
            if let Some(ref value_out) = op.var {
                attrs.insert("_simple_result_0".into(), AttrValue::Str(value_out.clone()));
            }
            if let Some(ref done_out) = op.out {
                attrs.insert("_simple_result_1".into(), AttrValue::Str(done_out.clone()));
            }
        }
        // Preserve only the structural class-id hint needed by object
        // allocation round-trips. Scalar `fast_int` / `fast_float` flags are
        // SimpleIR transport metadata and must not become TIR attributes; TIR
        // scalar authority lives in `value_types` and the refined LIR facts.
        if let Some(ref th) = op.type_hint {
            attrs.insert("_type_hint".into(), AttrValue::Str(th.clone()));
            // Type-refine result values from the frontend's hint.
            // Currently we only refine to `UserClass` at SSA lift; builtin
            // scalar refinement is the responsibility of the type-refine pass
            // and function-owned `value_types`, not legacy transport hints.
            //
            // UserClass refinement is the *live* use of
            // `TirType::UserClass` — every typed-class allocation
            // (`OBJECT_NEW_BOUND`, dataclass instantiation, etc.)
            // carries a `type_hint` whose value is the qualified
            // class name.  Refining DynBox → UserClass(name) lets
            // downstream passes (escape analysis, devirt, GVN)
            // reason about class identity without parsing the
            // attr string at every call site.
            //
            // Soundness: `from_type_hint` returns DynBox for any
            // non-identifier or built-in tag, so we only refine
            // when the hint is a plain class name.  Joining a
            // UserClass with DynBox at a phi collapses to DynBox
            // (covered by the `meet` lattice), so type-erased
            // exception handler args stay sound.
            let refined = TirType::from_type_hint(th);
            if matches!(refined, TirType::UserClass(_)) {
                for &result in &results {
                    self.value_types.insert(result, refined.clone());
                }
            }
        }
        let opcode = kind_to_opcode(&op.kind);

        if std::env::var("MOLT_TRACE_SSA_IMPORT").as_deref() == Ok("1") && opcode == OpCode::Import
        {
            eprintln!(
                "SSA import trace: func={} kind={} args={:?} var={:?} out={:?} operands={:?}",
                self.func_name, op.kind, op.args, op.var, op.out, operands
            );
        }

        // Opcode-specific attr key aliases: the lowering reads SimpleIR's
        // `s_value` under generated stable names. The registry owns opcode
        // membership; SSA owns copying the live attr payload.
        if let Some(ref v) = op.s_value {
            if let Some(attr_key) = opcode_ssa_s_value_attr_key_table(opcode) {
                attrs.insert(attr_key.into(), AttrValue::Str(v.clone()));
            }
        }

        // range_new maps to CallBuiltin but has no s_value to provide the
        // callee name.  Set it explicitly so downstream passes (range_devirt)
        // can pattern-match on name = "range".
        if op.kind == "range_new" && !attrs.contains_key("name") {
            attrs.insert("name".into(), AttrValue::Str("range".into()));
        }

        // Preserve the SimpleIR `var` spelling as transport metadata for
        // re-emission. For `copy_var`/`load_var` it is both resolved into an SSA
        // operand above and carried here as the original local-name fact; the
        // operand is value authority, `_var` is stream-identity authority.
        if simple_var_field_is_transport_fact(op.kind.as_str())
            && let Some(ref v) = op.var
        {
            attrs.insert("_var".into(), AttrValue::Str(v.clone()));
        }

        // Preserve `_original_kind` for unknown Copy fallbacks and for mapped
        // spellings whose non-canonical name is semantically visible to
        // round-trip/backends. The generated predicate owns the mapped spelling
        // set; unknown fallback preservation stays here because SSA is the
        // backstop for kinds with no first-class opcode.
        let mapped_kind = kind_to_opcode_table(op.kind.as_str()).is_some();
        if (opcode == OpCode::Copy && !mapped_kind)
            || simpleir_kind_preserves_original_kind_for_ssa(op.kind.as_str())
        {
            attrs.insert("_original_kind".into(), AttrValue::Str(op.kind.clone()));
        }

        // The concrete class authoring a typed-slot field op's byte-offset
        // (`store`/`store_init`/`load`/`guarded_field_*`). Carried through TIR so
        // the alias oracle (`region_of`) can assign a class+offset `TypedField`
        // memory region. The frontend emits these offset-based forms only when
        // the object's class is proven at the op (runtime version-guard for the
        // `guarded_field_*` forms, static type inference for the plain forms), so
        // the class is the layout authority for `value` (the offset).
        if let Some(ref class) = op.class_name {
            attrs.insert("_class".into(), AttrValue::Str(class.clone()));
        }

        let mut tir_op = TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs,
            source_span: None,
        };
        self.stamp_source_site(&mut tir_op, op_idx);
        tir_op
    }

    /// Build the terminator for a given CFG block.
    /// Collect the branch argument values for a target block based on its
    /// block argument variable list and the current variable stacks.
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

fn simple_var_field_is_transport_fact(kind: &str) -> bool {
    !matches!(kind, "checked_add" | "checked_mul" | "iter_next_unboxed")
}

fn simple_var_field_is_value_operand(op: &OpIR) -> bool {
    if matches!(
        op.kind.as_str(),
        "store_var" | "delete_var" | "checked_add" | "checked_mul" | "iter_next_unboxed"
    ) {
        return false;
    }
    if matches!(op.kind.as_str(), "copy_var" | "load_var")
        && op.args.as_ref().is_some_and(|args| !args.is_empty())
    {
        return false;
    }
    true
}

/// Compute immediate dominators for a CFG given by `successors` and
/// `predecessors`, rooted at `entry`. Mirrors `cfg::compute_dominators` but
/// operates on free-form predecessor/successor slices so the SSA pass can
/// build a dominator tree over an *augmented* graph (regular edges + exception
/// edges). Returns `idom[entry] = None` and `idom[bid] = Some(...)` for every
/// reachable block; unreachable blocks get `None`.
// Use shared is_structural from parent module (ensures SSA and lower_from_simple
// always agree on which ops to skip).
use super::is_structural;

/// Map a SimpleIR `kind` string to a TIR `OpCode`.
///
/// The kind→opcode table is the single-source-of-truth op-kind registry
/// (`runtime/molt-tir/src/tir/op_kinds.toml`, generated into
/// [`crate::tir::op_kinds_generated::kind_to_opcode_table`]; see
/// `docs/design/foundation/25_op_kind_registry.md`). A kind with no first-class
/// opcode falls back to `OpCode::Copy` (carrying its spelling in
/// `_original_kind`), exactly as before — this is the runtime backstop the
/// registry's sync test (`tests/test_gen_op_kinds.py`) and the drift audit
/// (`tools/audit_op_kinds.py --check`) keep statically total for known kinds.
fn kind_to_opcode(kind: &str) -> OpCode {
    kind_to_opcode_table(kind).unwrap_or(OpCode::Copy)
}

fn gpu_runtime_symbol_for_simple_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "gpu_thread_id" => Some("molt_gpu_thread_id"),
        "gpu_block_id" => Some("molt_gpu_block_id"),
        "gpu_block_dim" => Some("molt_gpu_block_dim"),
        "gpu_grid_dim" => Some("molt_gpu_grid_dim"),
        "gpu_barrier" => Some("molt_gpu_barrier"),
        _ => None,
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

    #[test]
    fn module_get_attr_does_not_lower_as_fallback_copy() {
        let ops = vec![
            op_args_out("module_cache_get", &["mod_name"], "module"),
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("Point".to_string()),
                out: Some("name".to_string()),
                ..OpIR::default()
            },
            op_args_out("module_get_attr", &["module", "name"], "class_ref"),
            op_args("ret", &["class_ref"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let module_lookup_count = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| op.opcode == OpCode::ModuleGetAttr)
            .count();
        let fallback_count = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| {
                op.opcode == OpCode::Copy
                    && matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if kind == "module_get_attr"
                    )
            })
            .count();

        assert_eq!(
            module_lookup_count, 1,
            "module_get_attr must lower to its first-class TIR opcode"
        );
        assert_eq!(
            fallback_count, 0,
            "module_get_attr must be a first-class TIR op, not Copy[_original_kind]"
        );
    }

    #[test]
    fn active_line_marker_stamps_tir_source_site() {
        let ops = vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(17),
                source_line: Some(17),
                col_offset: Some(4),
                end_col_offset: Some(13),
                ..OpIR::default()
            },
            op_val_out("const", 5, "x"),
            op_args("ret", &["x"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let attributed_ops: Vec<_> = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter_map(|op| op.source_site().map(|site| (op.opcode, site)))
            .collect();

        assert!(
            attributed_ops.iter().any(|(opcode, site)| {
                *opcode == OpCode::ConstInt
                    && site.line == 17
                    && site.col == Some(4)
                    && site.end_col == Some(13)
            }),
            "SSA lift must stamp executable ops from the active source line marker"
        );
    }

    #[test]
    fn store_var_survives_ssa_as_local_lifetime_marker() {
        let ops = vec![
            OpIR {
                kind: "list_new".to_string(),
                args: Some(vec![]),
                out: Some("list".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("bag".to_string()),
                args: Some(vec!["list".to_string()]),
                ..OpIR::default()
            },
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let store_markers: Vec<_> = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| {
                op.opcode == OpCode::Copy
                    && matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if kind == "store_var"
                    )
                    && matches!(op.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "bag")
            })
            .collect();
        assert!(
            !store_markers.is_empty(),
            "store_var must survive SSA as a local lifetime marker for finalizer ordering"
        );
        let marker = store_markers[0];
        assert_eq!(marker.operands.len(), 1);
        assert_eq!(marker.results.len(), 1);
        assert!(
            matches!(marker.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "bag"),
            "store_var marker must preserve the Python local name"
        );
    }

    #[test]
    fn copy_var_preserves_source_local_name_as_transport_attr() {
        let ops = vec![
            OpIR {
                kind: "const_none".to_string(),
                out: Some("seed".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("x".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy_var".to_string(),
                var: Some("x".to_string()),
                out: Some("read_x".to_string()),
                ..OpIR::default()
            },
            op_args("ret", &["read_x"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let copy = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .find(|op| {
                op.opcode == OpCode::Copy
                    && !op.attrs.contains_key("_original_kind")
                    && matches!(op.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "x")
            })
            .expect("copy_var must carry its original SimpleIR source-local name");
        assert_eq!(
            copy.operands.len(),
            1,
            "copy_var local-name metadata must not replace the SSA value operand"
        );
        assert_eq!(
            copy.results.len(),
            1,
            "copy_var must keep the copied SSA result while carrying local-name metadata"
        );
    }

    #[test]
    fn copy_var_with_args_treats_var_as_transport_not_second_operand() {
        let ops = vec![
            OpIR {
                kind: "const_none".to_string(),
                out: Some("seed".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("x".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy_var".to_string(),
                var: Some("x".to_string()),
                args: Some(vec!["seed".to_string()]),
                out: Some("alias".to_string()),
                ..OpIR::default()
            },
            op_args("ret", &["alias"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let copy = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .find(|op| {
                op.opcode == OpCode::Copy
                    && !op.attrs.contains_key("_original_kind")
                    && matches!(op.attrs.get("_var"), Some(AttrValue::Str(var)) if var == "x")
            })
            .expect("args-based copy_var must still carry its local-name metadata");

        assert_eq!(
            copy.operands.len(),
            1,
            "args-based copy_var must not also resolve var as a second value operand"
        );
    }

    #[test]
    fn ord_at_lowers_to_first_class_tir_opcode() {
        let ops = vec![
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("Aé".to_string()),
                out: Some("text".to_string()),
                ..OpIR::default()
            },
            op_val_out("const", 1, "idx"),
            op_args_out("ord_at", &["text", "idx"], "code"),
            op_args("ret", &["code"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let ord_at_count = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| op.opcode == OpCode::OrdAt)
            .count();
        let fallback_count = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| {
                op.opcode == OpCode::Copy
                    && matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if kind == "ord_at"
                    )
            })
            .count();

        assert_eq!(ord_at_count, 1, "ord_at must be a first-class TIR opcode");
        assert_eq!(
            fallback_count, 0,
            "ord_at must not lower as Copy[_original_kind]"
        );
    }

    fn assert_first_class_module_lookup(simple_kind: &str, expected_opcode: OpCode, args: &[&str]) {
        let mut ops = vec![
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("mod".to_string()),
                out: Some("module_name".to_string()),
                ..OpIR::default()
            },
            op_args_out("module_cache_get", &["module_name"], "module"),
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("answer".to_string()),
                out: Some("name".to_string()),
                ..OpIR::default()
            },
            op_args_out(simple_kind, args, "resolved"),
            op_args("ret", &["resolved"]),
        ];
        if simple_kind == "module_cache_get" {
            ops = vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("mod".to_string()),
                    out: Some("module_name".to_string()),
                    ..OpIR::default()
                },
                op_args_out("module_cache_get", &["module_name"], "resolved"),
                op_args("ret", &["resolved"]),
            ];
        }

        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let first_class_count = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| op.opcode == expected_opcode)
            .count();
        let fallback_count = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| {
                op.opcode == OpCode::Copy
                    && matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if kind == simple_kind
                    )
            })
            .count();

        assert_eq!(
            first_class_count, 1,
            "{simple_kind} must lower to its first-class TIR opcode"
        );
        assert_eq!(
            fallback_count, 0,
            "{simple_kind} must not lower as Copy[_original_kind]"
        );
    }

    #[test]
    fn module_cache_get_lowers_to_first_class_tir_opcode() {
        assert_first_class_module_lookup(
            "module_cache_get",
            OpCode::ModuleCacheGet,
            &["module_name"],
        );
    }

    #[test]
    fn module_get_global_lowers_to_first_class_tir_opcode() {
        assert_first_class_module_lookup(
            "module_get_global",
            OpCode::ModuleGetGlobal,
            &["module", "name"],
        );
    }

    #[test]
    fn module_get_name_lowers_to_first_class_tir_opcode() {
        assert_first_class_module_lookup(
            "module_get_name",
            OpCode::ModuleGetName,
            &["module", "name"],
        );
    }

    fn assert_first_class_module_mutation(
        simple_kind: &str,
        expected_opcode: OpCode,
        args: &[&str],
    ) {
        let ops = vec![
            op_args_out("module_import", &["builtins"], "module"),
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("answer".to_string()),
                out: Some("name".to_string()),
                ..OpIR::default()
            },
            op_val_out("const", 42, "value"),
            OpIR {
                kind: simple_kind.to_string(),
                args: Some(args.iter().map(|arg| arg.to_string()).collect()),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let first_class_count = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| op.opcode == expected_opcode)
            .count();
        let fallback_count = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| {
                op.opcode == OpCode::Copy
                    && matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if kind == simple_kind
                    )
            })
            .count();

        assert_eq!(
            first_class_count, 1,
            "{simple_kind} must lower to its first-class TIR opcode"
        );
        assert_eq!(
            fallback_count, 0,
            "{simple_kind} must not lower as Copy[_original_kind]"
        );
    }

    #[test]
    fn module_cache_set_lowers_to_first_class_tir_opcode() {
        assert_first_class_module_mutation(
            "module_cache_set",
            OpCode::ModuleCacheSet,
            &["name", "module"],
        );
    }

    #[test]
    fn module_cache_del_lowers_to_first_class_tir_opcode() {
        assert_first_class_module_mutation("module_cache_del", OpCode::ModuleCacheDel, &["name"]);
    }

    #[test]
    fn module_set_attr_lowers_to_first_class_tir_opcode() {
        assert_first_class_module_mutation(
            "module_set_attr",
            OpCode::ModuleSetAttr,
            &["module", "name", "value"],
        );
    }

    #[test]
    fn module_del_global_lowers_to_first_class_tir_opcode() {
        assert_first_class_module_mutation(
            "module_del_global",
            OpCode::ModuleDelGlobal,
            &["module", "name"],
        );
    }

    #[test]
    fn module_del_global_if_present_lowers_to_first_class_tir_opcode() {
        assert_first_class_module_mutation(
            "module_del_global_if_present",
            OpCode::ModuleDelGlobalIfPresent,
            &["module", "name"],
        );
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

    #[test]
    fn import_name_lowers_to_import_opcode_instead_of_copy_fallback() {
        let ops = vec![
            op_args_out("import_name", &["pathlib"], "mod"),
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);
        let entry = &output.blocks[0];
        assert!(
            entry.ops.iter().any(|op| op.opcode == OpCode::Import),
            "expected import_name to lower to OpCode::Import, got {:?}",
            entry.ops.iter().map(|op| op.opcode).collect::<Vec<_>>()
        );
    }

    #[test]
    fn module_import_lowers_to_import_opcode_instead_of_copy_fallback() {
        let ops = vec![
            op_args_out("module_import", &["pathlib"], "mod"),
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);
        let entry = &output.blocks[0];
        assert!(
            entry.ops.iter().any(|op| op.opcode == OpCode::Import),
            "expected module_import to lower to OpCode::Import, got {:?}",
            entry.ops.iter().map(|op| op.opcode).collect::<Vec<_>>()
        );
    }

    #[test]
    fn module_set_attr_does_not_lower_as_store_attr_transport() {
        let ops = vec![
            op_args_out("module_import", &["builtins"], "mod"),
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("answer".to_string()),
                out: Some("name".to_string()),
                ..OpIR::default()
            },
            op_val_out("const", 42, "value"),
            OpIR {
                kind: "module_set_attr".to_string(),
                args: Some(vec![
                    "mod".to_string(),
                    "name".to_string(),
                    "value".to_string(),
                ]),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);
        let entry = &output.blocks[0];
        let module_store_count = entry
            .ops
            .iter()
            .filter(|op| {
                op.opcode == OpCode::StoreAttr
                    && op.attrs.get("_original_kind").and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    }) == Some("module_set_attr")
            })
            .count();
        assert_eq!(
            module_store_count, 0,
            "module_set_attr must remain a module op instead of StoreAttr transport"
        );
    }

    #[test]
    fn module_import_preserves_variable_operand_through_ssa() {
        let ops = vec![
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("builtins".to_string()),
                out: Some("v62".to_string()),
                ..OpIR::default()
            },
            op_args_out("module_import", &["v62"], "v63"),
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);
        let entry = &output.blocks[0];
        let import_op = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Import)
            .expect("expected import op");
        assert_eq!(import_op.operands.len(), 1, "{:?}", import_op.operands);
    }

    #[test]
    fn module_import_preserves_operand_with_module_obj_param_and_checks() {
        let ops = vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("builtins".to_string()),
                out: Some("v62".to_string()),
                ..OpIR::default()
            },
            op_args_out("module_import", &["v62"], "v63"),
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(3),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("_builtins".to_string()),
                out: Some("v64".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".to_string(),
                args: Some(vec![
                    "__molt_module_obj__".to_string(),
                    "v64".to_string(),
                    "v63".to_string(),
                ]),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa_with_params(&cfg, &ops, &["__molt_module_obj__".into()]);
        let import_op = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.opcode == OpCode::Import)
            .expect("expected import op");
        assert_eq!(import_op.operands.len(), 1, "{:?}", import_op.operands);
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
    // Test 7b: Empty THEN (`if cond: pass / else: <body>`) must NOT swap
    //          THEN/ELSE bodies in CondBranch.  Regression for module-level
    //          `except*` residual ExceptionGroup propagation: the frontend
    //          emits `if exc IS None: pass / else: <recovery>` after every
    //          guarded `del` in `_emit_module_global_del_safe` and the SSA
    //          builder used to mis-route the TRUE edge into the recovery
    //          body, inverting the guard.
    // =======================================================================
    #[test]
    fn empty_then_with_else_does_not_swap_branch_bodies() {
        // Pattern:
        //   if cond:
        //       pass
        //   else:
        //       y = 7   <-- recovery body must run when cond is FALSE
        let ops = vec![
            op_val_out("const_bool", 1, "cond"), // 0: cond
            op_args("if", &["cond"]),            // 1: if (empty THEN)
            op("else"),                          // 2: else
            op_val_out("const", 7, "y"),         // 3: ELSE body
            op("end_if"),                        // 4: join
            op("ret_void"),                      // 5
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        // Locate the `if` block.
        let if_block_id = {
            let mut found = None;
            for block in &output.blocks {
                if matches!(&block.terminator, Terminator::CondBranch { .. }) {
                    found = Some(block.id);
                    break;
                }
            }
            found.expect("if block must exist")
        };
        let if_block = output
            .blocks
            .iter()
            .find(|b| b.id == if_block_id)
            .expect("if block lookup");

        let (then_block, else_block) = match &if_block.terminator {
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => (*then_block, *else_block),
            _ => unreachable!("verified above"),
        };

        // The ELSE block (`y = 7`) must be the FALSE-path destination.
        // Before the fix, ssa.rs treated fall-through as TRUE, which
        // routed the recovery body into the THEN edge and broke
        // `if cond: pass / else: <recovery>` semantics.
        let else_target = output
            .blocks
            .iter()
            .find(|b| b.id == else_block)
            .expect("else block lookup");
        let else_has_recovery = else_target
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::ConstInt);
        assert!(
            else_has_recovery,
            "FALSE path (else_block) must contain the recovery `const 7`; \
             builder is mis-routing branches"
        );

        // The THEN target must NOT contain the recovery body.
        if then_block != else_block {
            let then_target = output
                .blocks
                .iter()
                .find(|b| b.id == then_block)
                .expect("then block lookup");
            let then_has_recovery = then_target
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::ConstInt);
            assert!(
                !then_has_recovery,
                "TRUE path (then_block) must NOT contain the FALSE-only recovery body"
            );
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
        assert_eq!(
            check_ops.len(),
            1,
            "expected exactly one check_exception op"
        );
        assert!(
            check_ops[0].operands.is_empty(),
            "check_exception should not carry dead handler args, got operands {:?}",
            check_ops[0].operands
        );
    }

    #[test]
    fn check_exception_threads_live_cleanup_state_for_method_guarded_field_set() {
        let ops = vec![
            OpIR {
                kind: "exception_stack_enter".to_string(),
                out: Some("v88".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_depth".to_string(),
                out: Some("v89".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("self".to_string()),
                args: Some(vec!["self".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".to_string(),
                value: Some(3),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            op_val_out("const", 1, "v90"),
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("C".to_string()),
                out: Some("v91".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("method_trace".to_string()),
                out: Some("v92".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_get".to_string(),
                args: Some(vec!["v92".to_string()]),
                out: Some("v93".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_attr".to_string(),
                args: Some(vec!["v93".to_string(), "v91".to_string()]),
                out: Some("v94".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            op_val_out("const", 3, "v95"),
            OpIR {
                kind: "guarded_field_set".to_string(),
                args: Some(vec![
                    "self".to_string(),
                    "v94".to_string(),
                    "v95".to_string(),
                    "v90".to_string(),
                ]),
                s_value: Some("x".to_string()),
                value: Some(0),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            op_val_out("const", 0, "v96"),
            OpIR {
                kind: "ret".to_string(),
                var: Some("v96".to_string()),
                ..OpIR::default()
            },
            op_val("label", 1),
            OpIR {
                kind: "exception_stack_set_depth".to_string(),
                args: Some(vec!["v89".to_string()]),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_stack_exit".to_string(),
                args: Some(vec!["v88".to_string()]),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let handler_bid = cfg
            .blocks
            .iter()
            .position(|b| b.start_op <= 17 && b.end_op > 17)
            .expect("handler block should exist");
        let handler_block = &output.blocks[handler_bid];
        assert!(
            handler_block.args.len() >= 2,
            "handler block must receive cleanup state args, got {:?}",
            handler_block.args
        );

        let check_ops: Vec<_> = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| op.opcode == OpCode::CheckException)
            .collect();
        let rich_check = check_ops
            .iter()
            .find(|op| !op.operands.is_empty())
            .expect("at least one check_exception must carry handler args");
        assert!(
            rich_check.operands.len() >= 2,
            "method guarded-field path must thread cleanup vars into handler edge: {:?}",
            rich_check.operands
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

    #[test]
    fn post_if_store_var_reaches_following_load() {
        let ops = vec![
            OpIR {
                kind: "missing".to_string(),
                out: Some("seed".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("v".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("cond".to_string()),
                ..OpIR::default()
            },
            op_args("if", &["cond"]),
            op("else"),
            op_val_out("const", 1, "one"),
            op_args_out("or", &["cond", "one"], "picked"),
            op("end_if"),
            op_args_out("phi", &["cond", "picked"], "joined"),
            OpIR {
                kind: "store_var".to_string(),
                var: Some("v".to_string()),
                args: Some(vec!["joined".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("v".to_string()),
                out: Some("loaded".to_string()),
                ..OpIR::default()
            },
            op_args("ret", &["loaded"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let seed_values: HashSet<ValueId> = output
            .blocks
            .iter()
            .flat_map(|block| block.ops.iter())
            .filter(|op| {
                op.opcode == OpCode::ConstNone
                    || matches!(
                        op.attrs.get("_original_kind"),
                        Some(AttrValue::Str(kind)) if kind == "missing"
                    )
            })
            .flat_map(|op| op.results.iter().copied())
            .collect();
        let returned = output
            .blocks
            .iter()
            .find_map(|block| match &block.terminator {
                Terminator::Return { values } if !values.is_empty() => values.first().copied(),
                _ => None,
            })
            .expect("expected return value");

        assert!(
            !seed_values.contains(&returned),
            "load_var after the if/join must not collapse back to any seed missing/None value"
        );
    }

    #[test]
    fn delete_var_uses_explicit_old_operand_not_target_metadata() {
        let ops = vec![
            OpIR {
                kind: "missing".to_string(),
                out: Some("seed".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("item".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                s_value: Some("old".to_string()),
                out: Some("old_value".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("item".to_string()),
                args: Some(vec!["old_value".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("item".to_string()),
                out: Some("old_loaded".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "missing".to_string(),
                out: Some("gone".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "delete_var".to_string(),
                var: Some("item".to_string()),
                args: Some(vec!["gone".to_string(), "old_loaded".to_string()]),
                ..OpIR::default()
            },
            op("ret_void"),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let entry = &output.blocks[cfg.entry];
        let undef = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::ConstNone)
            .and_then(|op| op.results.first().copied())
            .expect("entry undef must exist");
        let seed = entry
            .ops
            .iter()
            .find(|op| {
                matches!(
                    op.attrs.get("_simple_out"),
                    Some(AttrValue::Str(name)) if name == "seed"
                )
            })
            .and_then(|op| op.results.first().copied())
            .expect("seed missing value must exist");
        let gone = entry
            .ops
            .iter()
            .find(|op| {
                matches!(
                    op.attrs.get("_simple_out"),
                    Some(AttrValue::Str(name)) if name == "gone"
                )
            })
            .and_then(|op| op.results.first().copied())
            .expect("delete missing value must exist");
        let delete = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::DeleteVar)
            .expect("delete_var must lower to first-class TIR");

        assert_eq!(delete.operands.len(), 2);
        assert_eq!(delete.operands[0], gone);
        assert_ne!(
            delete.operands[1], undef,
            "old-slot operand must not be the entry undef"
        );
        assert_ne!(
            delete.operands[1], seed,
            "old-slot operand must not collapse to the initial missing store"
        );
    }

    // =======================================================================
    // Regression: try/finally body assignment must thread through the
    // post-handler join via a phi (block argument).
    //
    // Shape (mirrors `copy.deepcopy`'s try/finally that exposed the bug):
    //
    //   r = missing            // pre-init in entry
    //   try_start L_handler
    //     check_exception L_handler
    //     r = ...               // assignment inside try body
    //     check_exception L_handler
    //   try_end
    //   ret r                   // use of r AFTER the try/finally
    //   L_handler:              // implicit handler entry (exception edge only)
    //     ret
    //
    // Without the augmented dominator construction, the post-handler join
    // block sees only the success-path predecessor in the regular CFG, and
    // the handler-side definition of `r` is invisible. The result-using
    // block then references `r` defined on a path that does not dominate
    // it, which the LIR verifier flags as an SSA dominance violation.
    // =======================================================================
    #[test]
    fn try_finally_threads_assignment_through_post_handler_join() {
        // CPython try/finally lowering, simplified:
        //
        //   r = missing                      // entry-block initializer
        //   try_start L_handler
        //     ... if c:                       // structured branch inside try
        //       r = 1                         // try-body assignment
        //     else:
        //       r = 2
        //   try_end
        //   jump L_join                      // skip handler on success
        //   L_handler:                        // exception path enters here
        //     // (handler runs cleanup, then merges to L_join)
        //   L_join:
        //     ret r                           // use of r AFTER handler merges
        //
        // The structured `if` inside the try forces a block split, so the
        // try body is multiple blocks. Without the augmented dominance
        // analysis, the post-handler `L_join` block sees only the
        // success-path predecessor (because the handler block has no
        // regular CFG predecessors) and the dominator analysis treats the
        // try body as dominating the join — producing a return that
        // references `r` on a path that does not actually dominate it
        // through every runtime control flow.
        let ops = vec![
            // r = missing
            op_val_out("const", 0, "r"),
            // c = 1
            op_val_out("const", 1, "c"),
            // try_start L100  (handler at label 100)
            op_val("try_start", 100),
            // if c:
            op_args("if", &["c"]),
            //   r = 1
            op_val_out("const", 1, "r"),
            // else:
            op("else"),
            //   r = 2
            op_val_out("const", 2, "r"),
            // end_if
            op("end_if"),
            // check_exception L100
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(100),
                ..OpIR::default()
            },
            // try_end
            op_val("try_end", 100),
            // jump L_join
            op_val("jump", 200),
            // L_handler:
            op_val("label", 100),
            // (handler cleanup body — no assignment to r)
            // jump L_join (handler merges back into the post-try control flow)
            op_val("jump", 200),
            // L_join:
            op_val("label", 200),
            // ret r
            op_args("ret", &["r"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        // Locate the return block.
        let return_block = output
            .blocks
            .iter()
            .find(|b| matches!(b.terminator, Terminator::Return { .. }))
            .expect("must produce at least one return block");
        let returned_value = match &return_block.terminator {
            Terminator::Return { values } => {
                *values.first().expect("return must carry one value (r)")
            }
            _ => unreachable!(),
        };

        // Collect which block each value is defined in.
        let mut def_block: HashMap<ValueId, BlockId> = HashMap::new();
        for block in &output.blocks {
            for arg in &block.args {
                def_block.insert(arg.id, block.id);
            }
            for op in &block.ops {
                for &r in &op.results {
                    def_block.insert(r, block.id);
                }
            }
        }
        let def_bid = *def_block
            .get(&returned_value)
            .expect("returned value must be defined somewhere");

        // Build the augmented predecessor relation used at runtime: regular
        // CFG predecessors PLUS exception edges. The returned value's
        // defining block must dominate the return block under THIS
        // relation. The bug being regressed is precisely that the
        // dominance check using only regular predecessors says the def
        // block dominates the return, but the augmented (runtime)
        // relation reveals it does not.
        let n = cfg.blocks.len();
        let mut aug_preds: Vec<Vec<usize>> = cfg.predecessors.clone();
        for &(from_bid, handler_bid) in &cfg.exception_edges {
            if !aug_preds[handler_bid].contains(&from_bid) {
                aug_preds[handler_bid].push(from_bid);
            }
        }
        // BFS from each block to determine which blocks each block is
        // reachable from in the augmented CFG (i.e., what its augmented
        // predecessors transitively reach to). Then for every block on a
        // path from entry to the return block, the def block must lie on
        // every such path — i.e., must be a dominator. Compute the set of
        // blocks that strictly dominate the return block under augmented
        // edges by a simple intersection-of-paths algorithm.
        let return_bid_idx = return_block.id.0 as usize;
        // Build augmented successors.
        let mut aug_succs: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (bid, preds) in aug_preds.iter().enumerate() {
            for &p in preds {
                aug_succs[p].push(bid);
            }
        }
        // For every path from entry to return_bid in aug graph, def_bid
        // must appear. Equivalent: removing def_bid from the graph makes
        // return_bid unreachable from entry.
        let def_bid_idx = def_bid.0 as usize;
        let entry_bid = output.blocks[0].id.0 as usize;
        let dominates = if def_bid_idx == return_bid_idx || def_bid_idx == entry_bid {
            true
        } else {
            // Reachability of return_bid from entry, skipping def_bid.
            let mut visited = vec![false; n];
            visited[def_bid_idx] = true; // mark blocked
            let mut stack = vec![entry_bid];
            visited[entry_bid] = true;
            let mut reaches_return = false;
            while let Some(b) = stack.pop() {
                if b == return_bid_idx {
                    reaches_return = true;
                    break;
                }
                for &succ in &aug_succs[b] {
                    if !visited[succ] {
                        visited[succ] = true;
                        stack.push(succ);
                    }
                }
            }
            !reaches_return
        };

        assert!(
            dominates,
            "returned `r` (value {returned_value:?}) is defined in bb{} but \
             that block does not dominate the return block bb{} under the \
             *augmented* CFG (regular edges + exception edges). This is the \
             deepcopy try/finally SSA dominance regression: without \
             folding exception edges into the dominance analysis, a \
             try-body assignment leaks past the handler-side control \
             flow into a return that the success-path definition does \
             not actually dominate at runtime.",
            def_bid.0, return_block.id.0,
        );
    }

    // =======================================================================
    // Test 9: All output values are typed as DynBox (when no hint)
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

    // =======================================================================
    // Test 10: type_hint="<UserClass>" refines result type to UserClass
    // =======================================================================
    /// SSA lift refines a result value's type from DynBox to
    /// `UserClass(class_id)` when the SimpleIR op carries
    /// `type_hint="<class_id>"` and the hint is identifier-shaped.
    /// This is the live use of `TirType::UserClass` — without it,
    /// the type system has no way to distinguish typed-class
    /// instances from arbitrary boxed values, and downstream
    /// passes (escape, devirt, GVN) cannot reason about class
    /// identity.
    #[test]
    fn user_class_type_hint_refines_result_type() {
        let mut alloc_op = op_args_out("object_new_bound", &["cls"], "p");
        alloc_op.type_hint = Some("Point".to_string());
        // Carry the size attr so the op is well-formed for the
        // Phase 5 step 3 lowering (24 bytes = 2 ints + __dict__
        // slot).  Not strictly required for the type-refine test
        // but matches what the frontend actually emits.
        alloc_op.value = Some(24);
        let ops = vec![op_val_out("const", 0, "cls"), alloc_op, op("ret_void")];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        // Find the ObjectNewBound op's result and verify its type.
        let entry = &output.blocks[0];
        let alloc_result = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::ObjectNewBound)
            .and_then(|op| op.results.first().copied())
            .expect("expected ObjectNewBound op with a result");
        assert_eq!(
            output.types[&alloc_result],
            TirType::UserClass("Point".into()),
            "type_hint=Point on object_new_bound must refine the \
             result value's type from DynBox to UserClass(\"Point\")"
        );
    }

    /// Builtin-tagged hints (`type_hint=int`, etc.) are intentionally NOT
    /// refined by this lift logic. Scalar representation must flow through
    /// type-refine and function-owned value facts, not TIR attrs copied from
    /// SimpleIR transport hints.
    #[test]
    fn builtin_type_hint_does_not_refine_at_lift() {
        let mut const_op = op_val_out("const", 42, "x");
        const_op.type_hint = Some("int".to_string());
        let ops = vec![const_op, op("ret_void")];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);

        let entry = &output.blocks[0];
        let const_result = entry
            .ops
            .iter()
            .find(|op| matches!(op.opcode, OpCode::ConstInt))
            .and_then(|op| op.results.first().copied())
            .expect("expected const op with a result");
        assert_eq!(
            output.types[&const_result],
            TirType::DynBox,
            "builtin type hint `int` must NOT refine at lift — \
             the refinement path for scalars goes through type-refine \
             and function-owned value facts, not the lift itself"
        );
    }
}
