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
//! Exception edges leave a block in the middle, at the operation that can
//! raise, and pass no new block arguments. A repair that needs an argument on
//! a block an exception edge enters is refused. The repair also checks that
//! every exception edge into a block where the variable is live carries the
//! definition that the block reads. On refusal the function is left partly
//! rewritten: the caller restores its own snapshot.

use molt_ir::deterministic_hash::{HashMap, HashSet};
use std::collections::BTreeSet;

use super::blocks::BlockId;
use super::dominators::{
    CfgEdgePolicy, build_dom_children, build_pred_map, build_pred_map_with,
    compute_dominance_frontiers, compute_dominator_tree, dom_tree_preorder,
    exception_label_to_block, exception_transfer_target, executable_reachable_blocks,
    iterated_dominance_frontier,
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
    /// A block that an exception edge enters needs a new block argument.
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

#[derive(Clone, Copy)]
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
                if !reachable.contains(&bid) {
                    continue;
                }
                for &operand in &op.operands {
                    uses.entry(operand)
                        .or_default()
                        .push((bid, UseSite::Op(index)));
                }
                if let Some(target) = exception_transfer_target(op, &labels) {
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
    let cfg = Cfg::build(func);
    let mut seen: HashSet<ValueId> = HashSet::default();
    for variable in variables {
        for &name in &variable.names {
            if !seen.insert(name) || !cfg.defs.contains_key(&name) {
                return Err(RepairRefusal::MalformedName);
            }
        }
    }
    let mut stats = RepairStats::default();
    for variable in variables {
        repair_one(func, &cfg, variable, &mut stats)?;
    }
    Ok(stats)
}

fn repair_one(
    func: &mut TirFunction,
    cfg: &Cfg,
    variable: &Variable,
    stats: &mut RepairStats,
) -> Result<(), RepairRefusal> {
    let names: HashSet<ValueId> = variable.names.iter().copied().collect();
    // Definitions per reachable block. A name defined in an unreachable block
    // has no path to any use.
    let mut defs_in: HashMap<BlockId, Vec<DefPos>> = HashMap::default();
    let mut arg_def_blocks: HashSet<BlockId> = HashSet::default();
    for name in &variable.names {
        let (block, pos) = cfg.defs[name];
        if !cfg.reachable.contains(&block) {
            continue;
        }
        defs_in.entry(block).or_default().push(pos);
        if matches!(pos, DefPos::Arg) {
            arg_def_blocks.insert(block);
        }
    }
    let first_def_before = |block: BlockId, index: usize| {
        defs_in
            .get(&block)
            .is_some_and(|positions| positions.iter().any(|pos| pos.precedes_op(index)))
    };

    // Liveness: a block is live in when a path from its entry reaches a use
    // before any definition. An exception edge leaves at its operation, so a
    // definition later in the source block does not cover it.
    let mut live_in: HashSet<BlockId> = HashSet::default();
    let mut work: Vec<BlockId> = Vec::new();
    for name in &variable.names {
        for &(block, site) in cfg.uses.get(name).into_iter().flatten() {
            let exposed = match site {
                UseSite::Op(index) => !first_def_before(block, index),
                UseSite::Terminator => !defs_in.contains_key(&block),
            };
            if exposed && live_in.insert(block) {
                work.push(block);
            }
        }
    }
    while let Some(block) = work.pop() {
        for &pred in cfg.terminator_preds.get(&block).into_iter().flatten() {
            if cfg.reachable.contains(&pred) && !defs_in.contains_key(&pred) && live_in.insert(pred)
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

    // Joins: the pruned iterated dominance frontier. A block whose argument
    // is itself a name already merges the variable.
    let def_blocks: HashSet<BlockId> = defs_in.keys().copied().collect();
    let joins: BTreeSet<BlockId> = iterated_dominance_frontier(&def_blocks, &cfg.frontiers)
        .into_iter()
        .filter(|block| live_in.contains(block) && !arg_def_blocks.contains(block))
        .collect();
    let mut join_arg: HashMap<BlockId, ValueId> = HashMap::default();
    for &block in &joins {
        if cfg.exception_preds.contains_key(&block) {
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
        join_arg.insert(block, arg);
        stats.joins += 1;
    }

    // Rename in dominator-tree preorder: a block starts from its join
    // argument, else from its immediate dominator's exit.
    let mut entry_state: HashMap<BlockId, Option<ValueId>> = HashMap::default();
    let mut exit_state: HashMap<BlockId, Option<ValueId>> = HashMap::default();
    let mut exception_edges: Vec<(BlockId, Option<ValueId>)> = Vec::new();
    for &bid in &cfg.preorder {
        let mut state = match join_arg.get(&bid) {
            Some(&arg) => Some(arg),
            None => cfg
                .tree
                .idom(bid)
                .and_then(|parent| exit_state.get(&parent).copied().flatten()),
        };
        entry_state.insert(bid, state);
        let block = func
            .blocks
            .get_mut(&bid)
            .expect("dominator tree block exists");
        for arg in &block.args {
            if names.contains(&arg.id) {
                state = Some(arg.id);
            }
        }
        let block_exception_succs = cfg.exception_succs.get(&bid);
        for (index, op) in block.ops.iter_mut().enumerate() {
            for operand in &mut op.operands {
                if names.contains(operand) {
                    let reaching = state.ok_or(RepairRefusal::UndefinedUse)?;
                    if *operand != reaching {
                        *operand = reaching;
                        stats.renamed_uses += 1;
                    }
                }
            }
            for &(at, target) in block_exception_succs.into_iter().flatten() {
                if at == index {
                    exception_edges.push((target, state));
                }
            }
            for result in &op.results {
                if names.contains(result) {
                    state = Some(*result);
                }
            }
        }
        let mut undefined = false;
        let mut renamed = 0;
        block.terminator.for_each_value_mut(|value| {
            if names.contains(value) {
                match state {
                    Some(reaching) if *value != reaching => {
                        *value = reaching;
                        renamed += 1;
                    }
                    Some(_) => {}
                    None => undefined = true,
                }
            }
        });
        block.terminator.for_each_edge_mut(|target, args| {
            if join_arg.contains_key(target) {
                match state {
                    Some(reaching) => args.push(reaching),
                    None => undefined = true,
                }
            }
        });
        if undefined {
            return Err(RepairRefusal::UndefinedUse);
        }
        stats.renamed_uses += renamed;
        exit_state.insert(bid, state);
    }

    // An unreachable block can still branch into a join: the lift keeps the
    // vestigial loop-else edge into a loop header. No execution takes that
    // edge, so it passes a `None` placeholder, as the lift's own vestigial
    // edges do, and the block keeps its arity.
    if !join_arg.is_empty() {
        let unreachable: Vec<_> = func
            .blocks
            .keys()
            .copied()
            .filter(|bid| !cfg.reachable.contains(bid))
            .collect();
        for bid in unreachable {
            let targets_join = func.blocks[&bid]
                .terminator
                .successors()
                .iter()
                .any(|target| join_arg.contains_key(target));
            if !targets_join {
                continue;
            }
            let placeholder = func.fresh_value();
            func.value_types.insert(placeholder, TirType::None);
            let block = func.blocks.get_mut(&bid).expect("block exists");
            block.ops.push(crate::tir::ops::TirOp::new(
                crate::tir::ops::OpCode::ConstNone,
                vec![],
                vec![placeholder],
            ));
            block.terminator.for_each_edge_mut(|target, args| {
                if join_arg.contains_key(target) {
                    args.push(placeholder);
                }
            });
        }
    }

    // An exception edge into a live block must carry the definition the
    // block reads, because the edge cannot pass a new argument.
    for (target, carried) in exception_edges {
        if live_in.contains(&target) && entry_state.get(&target).copied().flatten() != carried {
            return Err(RepairRefusal::ExceptionEdgeMismatch);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "ssa_update_tests.rs"]
mod tests;
