//! SSA repair after a CFG edit gives one variable several definitions.
//!
//! Loop versioning, tail duplication and jump threading copy blocks. A value
//! and its copy then define the same source variable on different paths. A use
//! that its definition no longer dominates must read the definition that
//! reaches it. This module is that repair, the block-argument form of LLVM's
//! `SSAUpdater`:
//!
//! 1. A variable is a set of SSA names, each defined once.
//! 2. Pruned SSA placement (Cytron et al. 1991; Choi, Cytron and Ferrante
//!    1991): a block argument goes on each block of the iterated dominance
//!    frontier of the definition blocks where the variable is live in.
//! 3. Renaming walks the dominator tree. Every use of any name reads the
//!    definition that reaches it, and every edge into a new argument carries
//!    the definition at the end of its source block.
//!
//! Executable exception edges carry the definition before the observing
//! operation. Region registrations cannot carry new arguments. On refusal the
//! function is left partly rewritten: the caller restores its own snapshot.

use molt_ir::deterministic_hash::{HashMap, HashSet};
use std::collections::BTreeSet;

use super::blocks::BlockId;
use super::dominators::{
    CfgEdgePolicy, build_dom_children, build_pred_map, build_pred_map_with,
    compute_dominance_frontiers, compute_dominator_tree, dom_tree_preorder,
    exception_edge_binds_handler_arguments, exception_label_to_block, exception_transfer_target,
    executable_reachable_blocks, iterated_dominance_frontier,
};
use super::function::TirFunction;
use super::types::TirType;
use super::values::{TirValue, ValueId};

/// One source variable: every SSA name that defines it, and its type.
#[derive(Debug, Clone)]
pub struct Variable {
    /// Each name is defined exactly once in the function, as an operation
    /// result or a block argument. No name belongs to two variables.
    pub names: Vec<ValueId>,
    /// The type of every new block argument that merges the names.
    pub ty: TirType,
}

/// Why a repair was refused. The function is then partly rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairRefusal {
    /// A name has no definition or belongs to two variables.
    MalformedName,
    /// The caller has not authorized a new payload, or a region registration
    /// blocks the required handler argument.
    JoinAtExceptionTarget,
    /// A path reaches a use, or a needed merge, with no definition.
    UndefinedUse,
    /// An exception edge carries a definition other than the one its target
    /// block reads.
    ExceptionEdgeMismatch,
}

/// What a successful repair changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepairStats {
    /// Block arguments added.
    pub joins: usize,
    /// Operand and terminator uses that now read a different definition.
    pub renamed_uses: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DefPos {
    Arg,
    Op(usize),
}

impl DefPos {
    /// Whether a definition here precedes a use at operation `index`.
    fn precedes_op(self, index: usize) -> bool {
        match self {
            DefPos::Arg => true,
            DefPos::Op(def) => def < index,
        }
    }
}

#[derive(Clone, Copy)]
enum UseSite {
    Op(usize),
    Terminator,
}

/// The CFG facts every variable's repair reads. The repair adds block
/// arguments and edge arguments only, so these facts stay valid throughout.
struct Cfg {
    entry: BlockId,
    reachable: HashSet<BlockId>,
    tree: crate::tir::dominators::DominatorTree,
    preorder: Vec<BlockId>,
    frontiers: HashMap<BlockId, HashSet<BlockId>>,
    terminator_preds: HashMap<BlockId, Vec<BlockId>>,
    /// target -> (source block, operation index) of each exception edge.
    exception_preds: HashMap<BlockId, Vec<(BlockId, usize)>>,
    /// source block -> (operation index, target) of each exception edge.
    exception_succs: HashMap<BlockId, Vec<(usize, BlockId)>>,
    registrations: HashSet<BlockId>,
    defs: HashMap<ValueId, (BlockId, DefPos)>,
    uses: HashMap<ValueId, Vec<(BlockId, UseSite)>>,
}

impl Cfg {
    fn build(func: &TirFunction) -> Self {
        let reachable = executable_reachable_blocks(func);
        let pred_map = build_pred_map(func);
        let tree = compute_dominator_tree(func, &pred_map);
        let preorder = dom_tree_preorder(func.entry_block, &build_dom_children(&tree));
        let frontiers = compute_dominance_frontiers(&tree, &pred_map);
        let terminator_preds = build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly);
        let labels = exception_label_to_block(func);
        let mut exception_preds: HashMap<BlockId, Vec<(BlockId, usize)>> = HashMap::default();
        let mut exception_succs: HashMap<BlockId, Vec<(usize, BlockId)>> = HashMap::default();
        let mut registrations = HashSet::default();
        let mut defs = HashMap::default();
        let mut uses: HashMap<ValueId, Vec<(BlockId, UseSite)>> = HashMap::default();
        for (&bid, block) in &func.blocks {
            for arg in &block.args {
                defs.insert(arg.id, (bid, DefPos::Arg));
            }
            for (index, op) in block.ops.iter().enumerate() {
                for &result in &op.results {
                    defs.insert(result, (bid, DefPos::Op(index)));
                }
                let exception = exception_transfer_target(op, &labels);
                if let Some(target) = exception
                    && !exception_edge_binds_handler_arguments(op.opcode)
                {
                    registrations.insert(target);
                }
                if !reachable.contains(&bid) {
                    continue;
                }
                for &operand in &op.operands {
                    uses.entry(operand)
                        .or_default()
                        .push((bid, UseSite::Op(index)));
                }
                if let Some(target) = exception {
                    exception_preds
                        .entry(target)
                        .or_default()
                        .push((bid, index));
                    exception_succs
                        .entry(bid)
                        .or_default()
                        .push((index, target));
                }
            }
            if reachable.contains(&bid) {
                block.terminator.for_each_value(|value| {
                    uses.entry(value)
                        .or_default()
                        .push((bid, UseSite::Terminator));
                });
            }
        }
        Self {
            entry: func.entry_block,
            reachable,
            tree,
            preorder,
            frontiers,
            terminator_preds,
            exception_preds,
            exception_succs,
            registrations,
            defs,
            uses,
        }
    }
}

/// Repair every variable: each use of a name reads the definition of its
/// variable that reaches it.
pub fn repair(
    func: &mut TirFunction,
    variables: &[Variable],
) -> Result<RepairStats, RepairRefusal> {
    repair_impl(func, variables, false)
}

/// The caller validates the new transport after representation selection.
/// Preparing an exception payload must not allocate on the continuing path.
pub(crate) fn repair_with_exception_payloads(
    func: &mut TirFunction,
    variables: &[Variable],
) -> Result<RepairStats, RepairRefusal> {
    repair_impl(func, variables, true)
}

fn repair_impl(
    func: &mut TirFunction,
    variables: &[Variable],
    carry_exception_payloads: bool,
) -> Result<RepairStats, RepairRefusal> {
    if variables.is_empty() {
        return Ok(RepairStats::default());
    }
    let cfg = Cfg::build(func);
    let mut plan = RenamePlan::default();
    for (index, variable) in variables.iter().enumerate() {
        for &name in &variable.names {
            if plan.name_variables.insert(name, index).is_some() || !cfg.defs.contains_key(&name) {
                return Err(RepairRefusal::MalformedName);
            }
        }
    }
    let mut stats = RepairStats::default();
    for (index, variable) in variables.iter().enumerate() {
        plan.place_joins(
            func,
            &cfg,
            variable,
            index,
            carry_exception_payloads,
            &mut stats,
        )?;
    }
    plan.rename(func, &cfg, variables.len(), &mut stats)?;
    Ok(stats)
}

#[derive(Default)]
struct RenamePlan {
    name_variables: HashMap<ValueId, usize>,
    /// Join args are appended in input variable order on both sides of an edge.
    joins: HashMap<BlockId, Vec<usize>>,
    handler_live_ins: HashMap<BlockId, Vec<usize>>,
}

impl RenamePlan {
    fn place_joins(
        &mut self,
        func: &mut TirFunction,
        cfg: &Cfg,
        variable: &Variable,
        variable_index: usize,
        carry_exception_payloads: bool,
        stats: &mut RepairStats,
    ) -> Result<(), RepairRefusal> {
        // Only the first definition can kill a live-in requirement. Arg
        // positions sort before operation results, which sort by index.
        let mut first_defs: HashMap<BlockId, DefPos> = HashMap::default();
        for name in &variable.names {
            let (block, pos) = cfg.defs[name];
            if cfg.reachable.contains(&block) {
                first_defs
                    .entry(block)
                    .and_modify(|first| *first = (*first).min(pos))
                    .or_insert(pos);
            }
        }
        let first_def_before = |block: BlockId, index: usize| {
            first_defs
                .get(&block)
                .is_some_and(|pos| pos.precedes_op(index))
        };

        let mut live_in: HashSet<BlockId> = HashSet::default();
        let mut work: Vec<BlockId> = Vec::new();
        for name in &variable.names {
            for &(block, site) in cfg.uses.get(name).into_iter().flatten() {
                let exposed = match site {
                    UseSite::Op(index) => !first_def_before(block, index),
                    UseSite::Terminator => !first_defs.contains_key(&block),
                };
                if exposed && live_in.insert(block) {
                    work.push(block);
                }
            }
        }
        while let Some(block) = work.pop() {
            for &pred in cfg.terminator_preds.get(&block).into_iter().flatten() {
                if cfg.reachable.contains(&pred)
                    && !first_defs.contains_key(&pred)
                    && live_in.insert(pred)
                {
                    work.push(pred);
                }
            }
            for &(pred, index) in cfg.exception_preds.get(&block).into_iter().flatten() {
                if !first_def_before(pred, index) && live_in.insert(pred) {
                    work.push(pred);
                }
            }
        }
        if live_in.contains(&cfg.entry) {
            return Err(RepairRefusal::UndefinedUse);
        }

        let def_blocks = first_defs.keys().copied().collect();
        let joins: BTreeSet<BlockId> = iterated_dominance_frontier(&def_blocks, &cfg.frontiers)
            .into_iter()
            .filter(|block| {
                live_in.contains(block) && !matches!(first_defs.get(block), Some(DefPos::Arg))
            })
            .collect();
        for &block in &joins {
            if cfg.registrations.contains(&block)
                || (!carry_exception_payloads && cfg.exception_preds.contains_key(&block))
            {
                return Err(RepairRefusal::JoinAtExceptionTarget);
            }
            let arg = func.fresh_value();
            func.blocks
                .get_mut(&block)
                .expect("join block exists")
                .args
                .push(TirValue {
                    id: arg,
                    ty: variable.ty.clone(),
                });
            func.value_types.insert(arg, variable.ty.clone());
            self.name_variables.insert(arg, variable_index);
            self.joins.entry(block).or_default().push(variable_index);
            stats.joins += 1;
        }
        for block in live_in {
            if cfg.exception_preds.contains_key(&block) && !joins.contains(&block) {
                self.handler_live_ins
                    .entry(block)
                    .or_default()
                    .push(variable_index);
            }
        }
        Ok(())
    }

    fn rename(
        &self,
        func: &mut TirFunction,
        cfg: &Cfg,
        variables: usize,
        stats: &mut RepairStats,
    ) -> Result<(), RepairRefusal> {
        // The walk visits each operand and definition once. Restoring only
        // assignments made within a subtree avoids copying the reaching table
        // at a branch or for every variable at the next block.
        let mut reaching = vec![None; variables];
        let mut undo: Vec<(usize, Option<ValueId>)> = Vec::new();
        let mut ancestors = Vec::new();
        let mut exception_carried = HashMap::default();
        let mut exception_expected = HashMap::default();
        for &bid in &cfg.preorder {
            let parent = cfg.tree.idom(bid);
            while ancestors.last().map(|&(block, _)| block) != parent {
                let (_, cursor) = ancestors.pop().expect("ancestor precedes its child");
                while undo.len() > cursor {
                    let (variable, previous) = undo.pop().unwrap();
                    reaching[variable] = previous;
                }
            }
            ancestors.push((bid, undo.len()));
            for &variable in self.handler_live_ins.get(&bid).into_iter().flatten() {
                exception_expected.insert((bid, variable), reaching[variable]);
            }
            let block = func
                .blocks
                .get_mut(&bid)
                .expect("dominator tree block exists");
            for arg in &block.args {
                self.define(arg.id, &mut reaching, &mut undo);
            }
            let mut exception_edges = cfg
                .exception_succs
                .get(&bid)
                .into_iter()
                .flatten()
                .peekable();
            for (index, op) in block.ops.iter_mut().enumerate() {
                for operand in &mut op.operands {
                    self.rename_use(operand, &reaching, stats)?;
                }
                while let Some(&&(at, target)) = exception_edges.peek() {
                    if at != index {
                        break;
                    }
                    exception_edges.next();
                    if let Some(joined) = self.joins.get(&target) {
                        let at = op.exception_payload().len();
                        let payload = joined
                            .iter()
                            .map(|&variable| reaching[variable].ok_or(RepairRefusal::UndefinedUse))
                            .collect::<Result<Vec<_>, _>>()?;
                        op.operands.splice(at..at, payload);
                    }
                    for &variable in self.handler_live_ins.get(&target).into_iter().flatten() {
                        let carried = reaching[variable];
                        if exception_carried
                            .insert((target, variable), carried)
                            .is_some_and(|previous| previous != carried)
                        {
                            return Err(RepairRefusal::ExceptionEdgeMismatch);
                        }
                    }
                }
                for &result in &op.results {
                    self.define(result, &mut reaching, &mut undo);
                }
            }
            let mut undefined = false;
            block.terminator.for_each_value_mut(|value| {
                undefined |= self.rename_use(value, &reaching, stats).is_err();
            });
            block.terminator.for_each_edge_mut(|target, args| {
                for &variable in self.joins.get(target).into_iter().flatten() {
                    if let Some(value) = reaching[variable] {
                        args.push(value);
                    } else {
                        undefined = true;
                    }
                }
            });
            if undefined {
                return Err(RepairRefusal::UndefinedUse);
            }
        }
        if exception_carried
            .into_iter()
            .any(|(key, carried)| exception_expected.get(&key).copied().flatten() != carried)
        {
            return Err(RepairRefusal::ExceptionEdgeMismatch);
        }
        self.complete_unreachable_edges(func, cfg);
        Ok(())
    }

    fn define(
        &self,
        value: ValueId,
        reaching: &mut [Option<ValueId>],
        undo: &mut Vec<(usize, Option<ValueId>)>,
    ) {
        if let Some(&variable) = self.name_variables.get(&value) {
            undo.push((variable, reaching[variable].replace(value)));
        }
    }

    fn rename_use(
        &self,
        value: &mut ValueId,
        reaching: &[Option<ValueId>],
        stats: &mut RepairStats,
    ) -> Result<(), RepairRefusal> {
        if let Some(&variable) = self.name_variables.get(value) {
            let replacement = reaching[variable].ok_or(RepairRefusal::UndefinedUse)?;
            if replacement != *value {
                *value = replacement;
                stats.renamed_uses += 1;
            }
        }
        Ok(())
    }

    fn complete_unreachable_edges(&self, func: &mut TirFunction, cfg: &Cfg) {
        if self.joins.is_empty() {
            return;
        }
        // Unreachable predecessors (for example a vestigial loop-else)
        // need a placeholder to keep the block argument arity consistent.
        let unreachable: Vec<_> = func
            .blocks
            .keys()
            .copied()
            .filter(|bid| !cfg.reachable.contains(bid))
            .collect();
        let labels = exception_label_to_block(func);
        for bid in unreachable {
            let variables: BTreeSet<_> = func.blocks[&bid]
                .terminator
                .successors()
                .into_iter()
                .chain(
                    func.blocks[&bid]
                        .ops
                        .iter()
                        .filter_map(|op| exception_transfer_target(op, &labels)),
                )
                .flat_map(|target| self.joins.get(&target).into_iter().flatten().copied())
                .collect();
            if variables.is_empty() {
                continue;
            }
            let placeholders: HashMap<_, _> = variables
                .into_iter()
                .map(|variable| {
                    let value = func.fresh_value();
                    func.value_types.insert(value, TirType::None);
                    (variable, value)
                })
                .collect();
            let block = func.blocks.get_mut(&bid).expect("block exists");
            for op in &mut block.ops {
                if let Some(target) = exception_transfer_target(op, &labels)
                    && let Some(joined) = self.joins.get(&target)
                {
                    let at = op.exception_payload().len();
                    op.operands
                        .splice(at..at, joined.iter().map(|v| placeholders[v]));
                }
            }
            block.ops.splice(
                0..0,
                placeholders.values().map(|&value| {
                    crate::tir::ops::TirOp::new(
                        crate::tir::ops::OpCode::ConstNone,
                        vec![],
                        vec![value],
                    )
                }),
            );
            block.terminator.for_each_edge_mut(|target, args| {
                args.extend(
                    self.joins
                        .get(target)
                        .into_iter()
                        .flatten()
                        .map(|v| placeholders[v]),
                );
            });
        }
    }
}

#[cfg(test)]
#[path = "ssa_update_tests.rs"]
mod tests;
