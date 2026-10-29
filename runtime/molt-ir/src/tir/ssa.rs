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
use super::cfg_liveness::{SimpleNameSet, SimpleNameTable, solve_name_liveness};
use super::ops::{AttrDict, AttrValue, Dialect, OpCode, SourceSite, TirOp};
use super::types::TirType;
use super::values::{TirValue, ValueId};

#[path = "ssa/op_lowering.rs"]
mod op_lowering;
#[path = "ssa/placement.rs"]
mod placement;
#[path = "ssa/terminators.rs"]
mod terminators;
#[cfg(test)]
#[path = "ssa/undefined_tests.rs"]
mod undefined_tests;
#[path = "ssa/variables.rs"]
mod variables;

use self::variables::visit_simple_ir_ssa_definitions;

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
///
/// `cfg` must describe these same operations with the block and edge invariants
/// established by [`CFG::build`]. In particular, exception edges are sorted by
/// source block, with at most one authored transfer target per source. The
/// parameter-aware entry points below use this same contract.
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
    crate::ir::validate_op_kinds(func_name, ops).unwrap_or_else(|error| panic!("{error}"));
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
    /// Upward-exposed reads consumed by liveness and pruned argument placement.
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
    /// Construction-only placeholder for known variables without a reaching def.
    /// Materialized only at actual uses, never eagerly at entry.
    undef_value: Option<ValueId>,
    /// Source-site fact active at each SimpleIR op index, derived once from
    /// frontend `line` markers plus per-op source fields.
    source_sites: Vec<Option<SourceSite>>,
    /// Augmented predecessors: regular predecessors plus exception and state
    /// resume edges. Used for SSA-correct dominance analysis. Handler/resume
    /// blocks can be reached only through implicit edges; without folding those
    /// into the predecessor relation, the dominator tree treats them as
    /// unreachable and misses their later join points.
    aug_predecessors: Vec<Vec<usize>>,
    /// Augmented successors paired with `aug_predecessors`. Iterator result
    /// validity consumes this executable graph directly so exception/resume
    /// routes cannot be mistaken for an ordinary not-done edge.
    aug_successors: Vec<Vec<usize>>,
    /// The augmented dominator tree over `aug_successors`, numbered once:
    /// immediate dominators by block id (`None` at the entry) and O(1)
    /// dominance queries.
    aug_dominance: crate::tir::dominators::IndexedDominance,
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
            source_sites: Self::build_source_sites(ops),
            aug_predecessors: vec![Vec::new(); n],
            aug_successors: vec![Vec::new(); n],
            aug_dominance: crate::tir::dominators::IndexedDominance::compute(&[], 0),
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

    fn source_op_index_for_op(&self, op_idx: usize) -> usize {
        self.ops
            .get(op_idx)
            .map_or(op_idx, |op| op.source_op_index_or(op_idx))
    }

    fn stamp_source_identity(&self, tir_op: &mut TirOp, op_idx: usize) {
        tir_op.set_source_op_index(self.source_op_index_for_op(op_idx));
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
        let traced_name = crate::backend_environment::var(
            crate::backend_environment::BackendEnv::MOLT_TIR_TRACE_FUNC,
        )
        .ok()
        .is_some_and(|filter| filter == "1" || self.func_name.contains(&filter))
        .then(|| self.func_name.clone());
        let trace_stage = |stage: &str| {
            if let Some(name) = traced_name.as_ref() {
                eprintln!("[TIR-SSA] {name} {stage}");
            }
        };
        trace_stage("before_defs_uses");
        self.gather_defs_uses();
        trace_stage("after_defs_uses");
        trace_stage("before_augmented_cfg");
        self.build_augmented_cfg();
        trace_stage("after_augmented_cfg");
        trace_stage("before_dominance_frontiers");
        self.compute_dominance_frontiers();
        trace_stage("after_dominance_frontiers");
        // All argument placement consumes one liveness solution. Seeding SSA
        // arguments changes neither source defs/uses nor the executable graph.
        trace_stage("before_liveness");
        let (live_names, live_in) = self.compute_live_in_vars();
        trace_stage("after_liveness");
        // One source census serves implicit admission and IDF. Actual seeded
        // arguments extend it only after source-only eligibility is captured.
        let mut def_sites = self.source_definition_sites(&live_names);
        // A direct dominating binding needs no fresh handler/resume identity.
        // Remaining implicit arguments are fresh definitions, so seed them
        // before IDF to preserve every normal/exception/resume rejoin phi.
        trace_stage("before_implicit_arguments");
        self.insert_implicit_edge_block_arguments(&live_names, &live_in, &mut def_sites);
        trace_stage("after_implicit_arguments");
        if let Some(name) = traced_name.as_ref() {
            let arguments: usize = self.block_arg_vars.iter().map(Vec::len).sum();
            eprintln!("[TIR-SSA] {name} implicit_arguments={arguments}");
        }
        trace_stage("before_phi_arguments");
        self.insert_block_arguments(&live_names, &live_in, &def_sites);
        trace_stage("after_phi_arguments");
        if let Some(name) = traced_name.as_ref() {
            let arguments: usize = self.block_arg_vars.iter().map(Vec::len).sum();
            eprintln!("[TIR-SSA] {name} total_arguments={arguments}");
        }
        trace_stage("before_rename");
        self.rename_and_emit();
        trace_stage("after_rename");
    }

    // -- Phase 4: rename variables and emit TIR blocks -----------------------

    fn rename_and_emit(&mut self) {
        let n = self.cfg.blocks.len();
        let undef_vid = self.fresh_value();
        self.undef_value = Some(undef_vid);

        // Build dominator tree children from the *augmented* dominator tree.
        // Handler blocks are reached only via exception edges, so the regular
        // CFG sees them as unreachable (no idom). The augmented relation
        // makes them reachable and gives them a real idom, so the rename
        // walk visits them in dominance order alongside the success path.
        let mut dom_children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for bid in 0..n {
            if let Some(idom) = self.aug_dominance.immediate_dominator(bid) {
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

        // Iterative dominator-tree walk (pre-order DFS).
        let mut stack: Vec<(usize, bool)> = vec![(self.cfg.entry, false)];

        while let Some((bid, is_exit)) = stack.pop() {
            if is_exit {
                // Immutable source metadata enumerates exactly the definitions
                // pushed below, including repeated names and binding aliases.
                for var in &self.block_arg_vars[bid] {
                    if let Some(s) = var_stacks.get_mut(var) {
                        s.pop();
                    }
                }
                for &op_idx in &self.block_info[bid].op_indices {
                    visit_simple_ir_ssa_definitions(&self.ops[op_idx], |var, _| {
                        if let Some(s) = var_stacks.get_mut(var) {
                            s.pop();
                        }
                    });
                }
                continue;
            }

            // Push exit marker.
            stack.push((bid, true));

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
            }

            // 2. Process ops in this block.
            let op_indices = self.block_info[bid].op_indices.clone();

            for &op_idx in &op_indices {
                let op = &self.ops[op_idx];

                let tir_op = self.translate_op(bid, op_idx, op, &var_stacks);

                visit_simple_ir_ssa_definitions(op, |var, result_idx| {
                    let vid = tir_op
                        .results
                        .get(result_idx)
                        .copied()
                        .expect("translated SimpleIR definition must have an SSA result");
                    var_stacks.entry(var.to_string()).or_default().push(vid);
                });

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

            // Branch arguments already use these active stack tops. Ancestor
            // definitions remain pushed until all dominated children finish;
            // an absent definition stays a placeholder for local materialization.

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
                // Missing reaching definitions use the same construction-only
                // placeholder as reachable blocks. Final materialization is
                // local to each consuming block, including disconnected roots.

                let op_indices = self.block_info[bid].op_indices.clone();
                for &op_idx in &op_indices {
                    let op = &self.ops[op_idx];
                    let tir_op = self.translate_op(bid, op_idx, op, &local_stacks);

                    visit_simple_ir_ssa_definitions(op, |var, result_idx| {
                        let vid = tir_op
                            .results
                            .get(result_idx)
                            .copied()
                            .expect("translated SimpleIR definition must have an SSA result");
                        local_stacks.entry(var.to_string()).or_default().push(vid);
                    });

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

        self.fuse_iter_next_projections(&mut tir_blocks);
        self.materialize_undefined_uses(&mut tir_blocks, undef_vid);
        self.undef_value = None;
        self.tir_blocks = tir_blocks;
    }

    /// Emit None only where a missing reaching definition remains after SSA
    /// construction. A block-local definition immediately before its first use
    /// dominates all remaining uses without assuming entry reaches that block.
    /// This also leaves checked lifecycle entry operations first when no value
    /// is needed there. Edge arguments are uses in the predecessor, not in the
    /// receiving block; the canonical terminator visitor includes all of them.
    fn materialize_undefined_uses(&mut self, blocks: &mut [TirBlock], undef: ValueId) {
        for block in blocks {
            let first_use = block.ops.iter().position(|op| op.operands.contains(&undef));
            let mut terminator_uses_undef = false;
            block.terminator.for_each_value(|value| {
                terminator_uses_undef |= value == undef;
            });
            let Some(insert_at) =
                first_use.or_else(|| terminator_uses_undef.then_some(block.ops.len()))
            else {
                continue;
            };
            let value = self.fresh_value_typed();
            for op in &mut block.ops[insert_at..] {
                for operand in &mut op.operands {
                    if *operand == undef {
                        *operand = value;
                    }
                }
            }
            block.terminator.for_each_value_mut(|operand| {
                if *operand == undef {
                    *operand = value;
                }
            });
            block.ops.insert(
                insert_at,
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstNone,
                    operands: vec![],
                    results: vec![value],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
            );
        }
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "ssa/tests.rs"]
mod tests;
