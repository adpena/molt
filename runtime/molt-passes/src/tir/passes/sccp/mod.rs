//! Sparse Conditional Constant Propagation (SCCP).
//!
//! Propagates constants through the SSA graph, folds constant operations,
//! and eliminates branches with known-constant conditions.
//!
//! The flow-insensitive SSA lattice carries immutable values only. Heap object
//! contents require memory-state and alias custody; they are not SSA constants.

mod eval;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::PassStats;
use super::effects;
use super::reachability::metadata_preserving_reachable_blocks;
use super::value_identity;
use crate::tir::blocks::{BlockId, CaseIndex, LoopRole, Terminator, TerminatorEdge};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ExceptionRegionNestingRole, SccpConstantEvalRule, SccpConstantSeedRule,
    opcode_exception_region_nesting_role_table, opcode_sccp_constant_eval_rule_table,
    opcode_sccp_constant_seed_rule_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use eval::{evaluate_builtin_call, evaluate_op};

/// One SSA value's state. Descends Unknown -> Constant -> Overdefined, never back.
#[derive(Debug, Clone, PartialEq)]
enum Lattice {
    /// No execution reaching the definition has been found yet.
    Unknown,
    /// The value on every execution found so far.
    Constant(ConstVal),
    /// Not one constant.
    Overdefined,
}

impl Lattice {
    /// The greatest lower bound of `self` and `incoming`.
    fn meet(&self, incoming: Lattice) -> Lattice {
        match (self, incoming) {
            (Lattice::Overdefined, _) | (_, Lattice::Overdefined) => Lattice::Overdefined,
            (Lattice::Unknown, incoming) => incoming,
            (current, Lattice::Unknown) => current.clone(),
            (Lattice::Constant(current), Lattice::Constant(incoming)) => {
                if *current == incoming {
                    Lattice::Constant(incoming)
                } else {
                    Lattice::Overdefined
                }
            }
        }
    }
}

/// Immutable value constants carried through the flow-insensitive lattice.
///
/// Mutability is excluded by construction, including transitively through
/// tuples. A list/dict snapshot would become stale after mutation through any
/// alias or callback, even when its SSA identity is unchanged. Mutable object
/// producers and their dependent observations therefore remain overdefined.
///
/// Equality denotes exact lattice value identity, not Python `==` or `is`.
/// Float bits preserve signed zero and NaN sign/payload; tuples compare their
/// recursively immutable values without depending on allocation identity.
#[derive(Debug, Clone)]
enum ConstVal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    None,
    /// Recursively immutable tuple (all elements are ConstVal).
    /// Capped at MAX_COMPOUND_ELEMENTS to avoid embedding huge data at compile time.
    /// Shared storage preserves the value DAG instead of expanding nested clones.
    Tuple(Arc<[ConstVal]>),
    /// Compile-time range(start, stop, step). Not materialized as a list,
    /// but supports len() and iteration count propagation.
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
}

impl PartialEq for ConstVal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left.to_bits() == right.to_bits(),
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Str(left), Self::Str(right)) => left == right,
            (Self::None, Self::None) => true,
            (Self::Tuple(left), Self::Tuple(right)) => {
                Arc::ptr_eq(left, right) || left.as_ref() == right.as_ref()
            }
            (
                Self::Range {
                    start: ls,
                    stop: le,
                    step: ld,
                },
                Self::Range {
                    start: rs,
                    stop: re,
                    step: rd,
                },
            ) => (ls, le, ld) == (rs, re, rd),
            _ => false,
        }
    }
}

// Bit-exact floats make equality reflexive even for NaNs.
impl Eq for ConstVal {}

/// Maximum number of elements for compile-time compound value folding.
/// Counts recursive tuple nodes and UTF-8 bytes, not only immediate children.
/// Admission happens before allocation, bounding compiler work and output size.
const MAX_COMPOUND_ELEMENTS: usize = 1000;

impl ConstVal {
    fn materialization_cost(&self) -> Option<usize> {
        let cost = match self {
            Self::Str(value) => value.len().max(1),
            Self::Tuple(elements) => elements.iter().try_fold(1usize, |cost, value| {
                let cost = cost.checked_add(value.materialization_cost()?)?;
                (cost <= MAX_COMPOUND_ELEMENTS).then_some(cost)
            })?,
            _ => 1,
        };
        (cost <= MAX_COMPOUND_ELEMENTS).then_some(cost)
    }
}

/// SCCP computes one immutable result, never a broadcast into result siblings.
/// The generated operation contract owns structural validity for every caller.
fn admits_constant_result(op: &TirOp) -> bool {
    op.has_valid_shape() && op.results.len() == 1
}

/// Build a set of ValueIds that are results of ops inside try regions.
/// When `has_exception_handling` is true, we must not rewrite these ops
/// to constants because the op's execution may transfer control to a
/// handler, and removing the op would change observable behavior.
///
/// "May throw" is sourced from the single op-kind registry oracle
/// (`effects::op_may_throw`, backed by `op_kinds.toml`) rather than a local
/// hand-list — a duplicate list is exactly the drift that mis-classified
/// `Shl`/`Shr`/`Pow` as non-throwing and let SCCP/DCE drop a dead `1 << -1`.
fn build_try_region_results(func: &TirFunction) -> HashSet<ValueId> {
    let mut result_set = HashSet::new();
    let value_types = crate::tir::type_refine::extract_exact_scalar_map(func);
    for block in func.blocks.values() {
        let mut try_depth: u32 = 0;
        for op in &block.ops {
            match opcode_exception_region_nesting_role_table(op.opcode) {
                ExceptionRegionNestingRole::Enter => try_depth += 1,
                ExceptionRegionNestingRole::Exit => try_depth = try_depth.saturating_sub(1),
                ExceptionRegionNestingRole::None => {}
            }
            if try_depth > 0 && effects::op_may_throw_with_types(op, &value_types) {
                for &r in &op.results {
                    result_set.insert(r);
                }
            }
        }
    }
    result_set
}

/// Position of an op within its block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OpIndex(u32);

/// Position of an argument within a block's argument list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BlockArgIndex(u32);

/// Where control leaves a block: its terminator, or an exception-transfer op
/// in the middle of the block (`dominators::exception_transfer_target`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EdgeSource {
    Terminator(TerminatorEdge),
    Exception(OpIndex),
}

/// One CFG edge, named by its source block and how control leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Edge {
    from: BlockId,
    source: EdgeSource,
}

/// The values an edge binds to its target's arguments. A region registration
/// (`TryStart`) keeps its handler reachable but binds nothing
/// (`dominators::exception_edge_binds_handler_arguments`).
#[derive(Debug, Clone, Copy)]
enum EdgeBinding<'f> {
    Args(&'f [ValueId]),
    Unbound,
}

/// An edge found feasible and not yet processed.
#[derive(Debug, Clone, Copy)]
struct PendingEdge<'f> {
    edge: Edge,
    target: BlockId,
    binding: EdgeBinding<'f>,
}

/// A read of an SSA value that must be revisited when the value's state drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Use {
    /// An operand of an op with results.
    Op { block: BlockId, op: OpIndex },
    /// The predicate of a `CondBranch` or `Switch`.
    Predicate { block: BlockId },
    /// A value an edge forwards to its target's argument.
    EdgeArg {
        edge: Edge,
        target: BlockId,
        arg: BlockArgIndex,
    },
}

/// Which edge a constant `CondBranch` predicate takes, or `None` when the
/// constant does not decide it. One authority for analysis and rewrite: a
/// branch is folded exactly when the analysis marked one edge executable.
fn constant_branch_choice(value: &ConstVal) -> Option<TerminatorEdge> {
    let truthy = match value {
        ConstVal::Bool(value) => *value,
        ConstVal::Int(value) => *value != 0,
        ConstVal::None => false,
        ConstVal::Float(_) | ConstVal::Str(_) | ConstVal::Tuple(_) | ConstVal::Range { .. } => {
            return None;
        }
    };
    Some(if truthy {
        TerminatorEdge::Then
    } else {
        TerminatorEdge::Else
    })
}

/// Which edge a constant `Switch` value takes, or `None` when the constant does
/// not decide it. Only an exact `Int` selects a case.
fn constant_switch_choice(
    cases: &[(i64, BlockId, Vec<ValueId>)],
    value: &ConstVal,
) -> Option<TerminatorEdge> {
    let ConstVal::Int(value) = value else {
        return None;
    };
    Some(
        (0u32..)
            .zip(cases)
            .find(|(_, (case, _, _))| case == value)
            .map_or(TerminatorEdge::SwitchDefault, |(index, _)| {
                TerminatorEdge::SwitchCase(CaseIndex(index))
            }),
    )
}

/// The edge a terminator takes on every execution, when its predicate's state
/// decides one. `None` means the predicate does not decide a single edge.
fn decided_edge(term: &Terminator, lattice: &HashMap<ValueId, Lattice>) -> Option<TerminatorEdge> {
    let constant = |value: &ValueId| match lattice.get(value) {
        Some(Lattice::Constant(constant)) => Some(constant),
        _ => None,
    };
    match term {
        Terminator::CondBranch { cond, .. } => constant(cond).and_then(constant_branch_choice),
        Terminator::Switch { value, cases, .. } => {
            constant(value).and_then(|value| constant_switch_choice(cases, value))
        }
        Terminator::Branch { .. }
        | Terminator::StateDispatch { .. }
        | Terminator::Return { .. }
        | Terminator::Unreachable => None,
    }
}

/// The Wegman-Zadeck solver state for one function.
struct Solver<'f> {
    func: &'f TirFunction,
    label_to_block: HashMap<i64, BlockId>,
    try_region_results: HashSet<ValueId>,
    lattice: HashMap<ValueId, Lattice>,
    executable_blocks: HashSet<BlockId>,
    executable_edges: HashSet<Edge>,
    edge_worklist: Vec<PendingEdge<'f>>,
    value_worklist: Vec<ValueId>,
    uses: HashMap<ValueId, Vec<Use>>,
}

impl<'f> Solver<'f> {
    fn new(func: &'f TirFunction) -> Self {
        let label_to_block = dominators::exception_label_to_block(func);
        let try_region_results = if func.has_exception_handling {
            build_try_region_results(func)
        } else {
            HashSet::new()
        };
        let uses = build_uses(func, &label_to_block);
        Self {
            func,
            label_to_block,
            try_region_results,
            lattice: HashMap::new(),
            executable_blocks: HashSet::new(),
            executable_edges: HashSet::new(),
            edge_worklist: Vec::new(),
            value_worklist: Vec::new(),
            uses,
        }
    }

    fn state(&self, value: ValueId) -> &Lattice {
        self.lattice.get(&value).unwrap_or(&Lattice::Unknown)
    }

    /// Meet `incoming` into `value`'s state (I-1: a state never rises).
    fn lower(&mut self, value: ValueId, incoming: Lattice) {
        let current = self.lattice.entry(value).or_insert(Lattice::Unknown);
        let met = current.meet(incoming);
        if met != *current {
            *current = met;
            self.value_worklist.push(value);
        }
    }

    fn solve(&mut self) {
        let func = self.func;
        let entry = func.entry_block;
        // Function parameters: no edge binds them.
        for arg in &func.blocks[&entry].args {
            self.lower(arg.id, Lattice::Overdefined);
        }
        self.executable_blocks.insert(entry);
        self.visit_block(entry);
        loop {
            if let Some(pending) = self.edge_worklist.pop() {
                self.visit_edge(pending);
            } else if let Some(value) = self.value_worklist.pop() {
                self.visit_value(value);
            } else {
                break;
            }
        }
    }

    fn visit_edge(&mut self, pending: PendingEdge<'f>) {
        if !self.executable_edges.insert(pending.edge) {
            return;
        }
        let func = self.func;
        let Some(target) = func.blocks.get(&pending.target) else {
            return;
        };
        for (index, arg) in target.args.iter().enumerate() {
            let incoming = match pending.binding {
                EdgeBinding::Args(values) => values
                    .get(index)
                    .map_or(Lattice::Overdefined, |value| self.state(*value).clone()),
                EdgeBinding::Unbound => Lattice::Overdefined,
            };
            self.lower(arg.id, incoming);
        }
        if self.executable_blocks.insert(pending.target) {
            self.visit_block(pending.target);
        }
    }

    fn visit_value(&mut self, value: ValueId) {
        // Uses are fixed for the solve. Visits never read this value's own list,
        // so it is taken for the loop and restored after, with no copy.
        let Some(uses) = self.uses.get_mut(&value).map(std::mem::take) else {
            return;
        };
        for &use_site in &uses {
            match use_site {
                Use::Op { block, op } => {
                    if self.executable_blocks.contains(&block) {
                        self.visit_op(block, op);
                    }
                }
                Use::Predicate { block } => {
                    if self.executable_blocks.contains(&block) {
                        self.visit_terminator(block);
                    }
                }
                Use::EdgeArg { edge, target, arg } => {
                    if self.executable_edges.contains(&edge) {
                        let incoming = self.state(value).clone();
                        let func = self.func;
                        if let Some(argument) = func
                            .blocks
                            .get(&target)
                            .and_then(|block| block.args.get(arg.0 as usize))
                        {
                            self.lower(argument.id, incoming);
                        }
                    }
                }
            }
        }
        self.uses.insert(value, uses);
    }

    fn visit_block(&mut self, block_id: BlockId) {
        let func = self.func;
        let block = &func.blocks[&block_id];
        for (index, op) in (0u32..).zip(&block.ops) {
            let op_index = OpIndex(index);
            self.visit_op(block_id, op_index);
            if let Some(target) = dominators::exception_transfer_target(op, &self.label_to_block) {
                let binding = if dominators::exception_edge_binds_handler_arguments(op.opcode) {
                    EdgeBinding::Args(&op.operands)
                } else {
                    EdgeBinding::Unbound
                };
                self.edge_worklist.push(PendingEdge {
                    edge: Edge {
                        from: block_id,
                        source: EdgeSource::Exception(op_index),
                    },
                    target,
                    binding,
                });
            }
        }
        self.visit_terminator(block_id);
    }

    fn visit_op(&mut self, block_id: BlockId, op_index: OpIndex) {
        let func = self.func;
        let op = &func.blocks[&block_id].ops[op_index.0 as usize];
        if op.results.is_empty() {
            return;
        }
        let Some(state) = self.evaluate(op) else {
            return;
        };
        for &result in &op.results {
            self.lower(result, state.clone());
        }
    }

    /// The state an op's results take from its operands' current states, or
    /// `None` while an operand is still Unknown.
    fn evaluate(&self, op: &TirOp) -> Option<Lattice> {
        if !admits_constant_result(op) {
            // Malformed producers never seed downstream constants or branches.
            return Some(Lattice::Overdefined);
        }
        // Loop-carried values change on each iteration.
        if matches!(
            op.origin,
            Some(
                molt_ir::OpKind::LoopIndexStart
                    | molt_ir::OpKind::LoopIndexNext
                    | molt_ir::OpKind::IterNext
            )
        ) {
            return Some(Lattice::Overdefined);
        }
        if self.try_region_results.contains(&op.results[0]) {
            return Some(Lattice::Overdefined);
        }
        if let Some(seed) = seed_constant_lattice_value(op) {
            return Some(seed);
        }
        if let Some(source) = value_identity::copy_value_source(op) {
            return self.forwarded(source, Forwarding::Value);
        }
        if let Some(source) = value_identity::scalar_value_source(op) {
            return self.forwarded(source, Forwarding::ImmutableScalar);
        }
        let mut operands = Vec::with_capacity(op.operands.len());
        for operand in &op.operands {
            match self.state(*operand) {
                Lattice::Overdefined => return Some(Lattice::Overdefined),
                Lattice::Unknown => return None,
                Lattice::Constant(constant) => operands.push(Some(constant)),
            }
        }
        // A single Unknown operand waits; reaching here means every operand is Constant.
        Some(
            evaluate_op(op.opcode, &operands)
                .or_else(|| evaluate_builtin_call(op, &operands))
                .map_or(Lattice::Overdefined, Lattice::Constant),
        )
    }

    /// The state of a result that holds `source`'s value on every execution
    /// that observes it, or `None` while `source` is Unknown.
    fn forwarded(&self, source: ValueId, forwarding: Forwarding) -> Option<Lattice> {
        match (self.state(source), forwarding) {
            (Lattice::Unknown, _) => None,
            (state, Forwarding::Value) => Some(state.clone()),
            (
                Lattice::Constant(
                    constant @ (ConstVal::Int(_) | ConstVal::Float(_) | ConstVal::Bool(_)),
                ),
                Forwarding::ImmutableScalar,
            ) => Some(Lattice::Constant(constant.clone())),
            (_, Forwarding::ImmutableScalar) => Some(Lattice::Overdefined),
        }
    }

    fn visit_terminator(&mut self, block_id: BlockId) {
        let func = self.func;
        let term = &func.blocks[&block_id].terminator;
        let feasible: Option<TerminatorEdge> = match term {
            Terminator::CondBranch { cond, .. } => match self.state(*cond) {
                Lattice::Unknown => return,
                _ => decided_edge(term, &self.lattice),
            },
            Terminator::Switch { value, .. } => match self.state(*value) {
                Lattice::Unknown => return,
                _ => decided_edge(term, &self.lattice),
            },
            Terminator::Branch { .. } | Terminator::StateDispatch { .. } => None,
            Terminator::Return { .. } | Terminator::Unreachable => return,
        };
        term.for_each_labeled_edge(|label, target, args| {
            if feasible.is_some_and(|decided| decided != label) {
                return;
            }
            let edge = Edge {
                from: block_id,
                source: EdgeSource::Terminator(label),
            };
            if !self.executable_edges.contains(&edge) {
                self.edge_worklist.push(PendingEdge {
                    edge,
                    target,
                    binding: EdgeBinding::Args(args),
                });
            }
        });
    }
}

/// Every read of every value, built once (I-4 needs each state drop to visit
/// only its own uses).
fn build_uses(
    func: &TirFunction,
    label_to_block: &HashMap<i64, BlockId>,
) -> HashMap<ValueId, Vec<Use>> {
    let mut uses: HashMap<ValueId, Vec<Use>> = HashMap::new();
    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_unstable();
    for block_id in block_ids {
        let block = &func.blocks[&block_id];
        for (index, op) in (0u32..).zip(&block.ops) {
            let op_index = OpIndex(index);
            if !op.results.is_empty() {
                for operand in &op.operands {
                    uses.entry(*operand).or_default().push(Use::Op {
                        block: block_id,
                        op: op_index,
                    });
                }
            }
            if let Some(target) = dominators::exception_transfer_target(op, label_to_block)
                && dominators::exception_edge_binds_handler_arguments(op.opcode)
            {
                let edge = Edge {
                    from: block_id,
                    source: EdgeSource::Exception(op_index),
                };
                for (arg, operand) in (0u32..).zip(&op.operands) {
                    uses.entry(*operand).or_default().push(Use::EdgeArg {
                        edge,
                        target,
                        arg: BlockArgIndex(arg),
                    });
                }
            }
        }
        match &block.terminator {
            Terminator::CondBranch {
                cond: predicate, ..
            }
            | Terminator::Switch {
                value: predicate, ..
            } => uses
                .entry(*predicate)
                .or_default()
                .push(Use::Predicate { block: block_id }),
            Terminator::Branch { .. }
            | Terminator::StateDispatch { .. }
            | Terminator::Return { .. }
            | Terminator::Unreachable => {}
        }
        block
            .terminator
            .for_each_labeled_edge(|label, target, args| {
                let edge = Edge {
                    from: block_id,
                    source: EdgeSource::Terminator(label),
                };
                for (arg, value) in (0u32..).zip(args) {
                    uses.entry(*value).or_default().push(Use::EdgeArg {
                        edge,
                        target,
                        arg: BlockArgIndex(arg),
                    });
                }
            });
    }
    uses
}

/// Run the SCCP pass on `func`, returning statistics.
///
/// Requires: `func` verifies.
/// Ensures: every rewritten value equals its value on every execution; a branch
/// is folded only when its predicate takes one edge on every execution and no
/// target is a loop header; blocks no longer reachable are removed so the
/// result verifies. Each value's state drops at most twice and each drop visits
/// only that value's uses, so the solve is O(edges + uses) lattice steps, each
/// costing at most one `ConstVal` comparison (bounded by MAX_COMPOUND_ELEMENTS).
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "sccp",
        ..Default::default()
    };

    let lattice = {
        let mut solver = Solver::new(func);
        solver.solve();
        solver.lattice
    };

    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_unstable();

    // Rewrite: constant results become constant constructors. Values in blocks
    // the solve never executed stay Unknown and are never rewritten.
    for &bid in &block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        for op in &mut block.ops {
            if !admits_constant_result(op) {
                continue;
            }
            if opcode_sccp_constant_seed_rule_table(op.opcode) != SccpConstantSeedRule::None {
                continue;
            }
            if !rewrites_to_constant(op) {
                continue;
            }
            if let Some(Lattice::Constant(constant)) = lattice.get(&op.results[0])
                && rewrite_as_constant(op, constant)
            {
                stats.values_changed += 1;
            }
        }
    }

    // Fold branches whose predicate decides one edge. A loop-header target is
    // never folded: lower_to_simple rebuilds structured loops from it.
    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        let Some(decided) = decided_edge(&block.terminator, &lattice) else {
            continue;
        };
        let mut targets_loop_header = false;
        let mut taken = None;
        block
            .terminator
            .for_each_labeled_edge(|label, target, args| {
                targets_loop_header |= func.loop_roles.get(&target) == Some(&LoopRole::LoopHeader);
                if label == decided {
                    taken = Some((target, args.to_vec()));
                }
            });
        if targets_loop_header {
            continue;
        }
        if let Some((target, args)) = taken {
            func.blocks.get_mut(&bid).unwrap().terminator = Terminator::Branch { target, args };
            stats.ops_removed += 1; // count branch simplification
        }
    }

    // Remove blocks a fold left unreachable. Their ops may name values whose
    // definitions no longer dominate them, which the verifier rejects.
    if stats.ops_removed > 0 {
        let reachable = metadata_preserving_reachable_blocks(func);
        stats.ops_removed += func
            .retain_blocks(&reachable)
            .expect("SCCP reachability must preserve live block and metadata references");
    }

    stats
}

/// Which value-identity authority makes a result hold its source's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Forwarding {
    /// A value copy (`value_identity::copy_value_source`): any state crosses.
    Value,
    /// An effectful publication or a guard (`value_identity::scalar_value_source`):
    /// only an immutable scalar constant crosses; anything else is Overdefined.
    ImmutableScalar,
}

/// Whether a constant result may replace `op`: only an op whose constant the
/// evaluator computed. A forwarding op keeps its op even when its value is
/// constant: a publication, slot store or guard keeps its effect, and a plain
/// copy is copy propagation's to forward. A copy rewritten into a fresh
/// constant would survive copy propagation as a duplicate, and a string
/// constant would add an owned materialization, a release and an exception
/// check (measured: +283 of each in bench_generator). Readers of a forwarded
/// constant still fold.
fn rewrites_to_constant(op: &TirOp) -> bool {
    opcode_sccp_constant_eval_rule_table(op.opcode) != SccpConstantEvalRule::None
        || op.opcode == OpCode::CallBuiltin
}

/// Rewrite `op` as the constant constructor of `constant`; false when TIR has
/// no constructor for it (immutable compounds keep their runtime producer).
fn rewrite_as_constant(op: &mut TirOp, constant: &ConstVal) -> bool {
    let (opcode, attrs) = match constant {
        ConstVal::Int(value) => (
            OpCode::ConstInt,
            AttrDict::from([("value".into(), AttrValue::Int(*value))]),
        ),
        ConstVal::Float(value) => (
            OpCode::ConstFloat,
            AttrDict::from([("f_value".into(), AttrValue::Float(*value))]),
        ),
        ConstVal::Bool(value) => (
            OpCode::ConstBool,
            AttrDict::from([("value".into(), AttrValue::Bool(*value))]),
        ),
        ConstVal::Str(value) => (
            OpCode::ConstStr,
            AttrDict::from([("s_value".into(), AttrValue::Str(value.clone()))]),
        ),
        ConstVal::None => (OpCode::ConstNone, AttrDict::new()),
        // Immutable compounds inform downstream value observations, but their
        // producers retain runtime allocation/identity. TIR has no ConstTuple
        // or ConstRange materialization opcode.
        ConstVal::Tuple(_) | ConstVal::Range { .. } => return false,
    };
    op.opcode = opcode;
    op.operands.clear();
    op.attrs = attrs;
    op.origin = None;
    true
}

fn seed_constant_lattice_value(op: &TirOp) -> Option<Lattice> {
    if !admits_constant_result(op) {
        return Some(Lattice::Overdefined);
    }
    let attrs = &op.attrs;
    match opcode_sccp_constant_seed_rule_table(op.opcode) {
        SccpConstantSeedRule::None => None,
        SccpConstantSeedRule::IntAttr => Some(match attrs.get("value") {
            Some(AttrValue::Int(v)) => Lattice::Constant(ConstVal::Int(*v)),
            _ => Lattice::Overdefined,
        }),
        SccpConstantSeedRule::FloatAttr => Some(match attrs.get("f_value") {
            Some(AttrValue::Float(v)) => Lattice::Constant(ConstVal::Float(*v)),
            _ => Lattice::Overdefined,
        }),
        SccpConstantSeedRule::BoolAttr => Some(match attrs.get("value") {
            Some(AttrValue::Bool(v)) => Lattice::Constant(ConstVal::Bool(*v)),
            _ => Lattice::Overdefined,
        }),
        SccpConstantSeedRule::StrAttr => {
            use molt_ir::literal_payload::LiteralPayload;
            let payload = LiteralPayload::from_tir(op).unwrap_or_else(|error| panic!("{error}"));
            Some(match payload {
                LiteralPayload::Text(value) if value.len() <= MAX_COMPOUND_ELEMENTS => {
                    Lattice::Constant(ConstVal::Str(value.to_owned()))
                }
                // Rust str cannot represent Python surrogate code points.
                // Preserve the admitted byte carrier; never seed a lossy string.
                LiteralPayload::Text(_) | LiteralPayload::Bytes(_) => Lattice::Overdefined,
            })
        }
        SccpConstantSeedRule::NoneSingleton => Some(Lattice::Constant(ConstVal::None)),
    }
}
