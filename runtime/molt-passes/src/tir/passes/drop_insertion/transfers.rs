//! Custody of the operands an operation takes.
//!
//! An operation takes an operand's reference in one of two declared ways
//! (`op_transferred_operands`). It **consumes** it by its generated operand
//! ownership: a frame home store moves the binding's reference into the home,
//! and `call_bind` frees the CallArgs builder. A source Python call instruction
//! **adopts** each argument whose typed custody is `Transferred`. Either way the
//! op owns the reference on its normal and its exceptional continuation,
//! whether or not a callee runs, so the holder never releases it. The holder
//! supplies it in one of two ways:
//!
//! * **Move.** An owned root that nothing reads after the op gives its own +1
//!   to the first position naming it. Its name retires there
//!   (`PointAvailability::retire_adopted`), so no placement releases it again
//!   on either continuation: no last use, landing, arc or `Return` release.
//! * **Retain.** Every other position receives an `IncRef` right before the
//!   op: a second position of the same root, a root still read afterwards or
//!   borrowed by the same op, a borrowed parameter, a non-owning alias or
//!   binding view, a root a Python boundary keeps. The holder keeps its own
//!   reference and every ordinary release.
//!
//! Which Python boundaries keep a root depends on how the op takes it
//! (`holds_to_boundary`). Adoption and generic consumption leave the binding
//! bound, so a lexical root is retained and released at its own boundary. Only
//! a binding store ends its SSA custody: the home owns it from then on. An
//! existing binding owner that cannot move into its home is malformed IR,
//! rather than permission to create a second owner that delays finalization.
//! Expression and statement-held temporaries are different: they may retain
//! their independent reference across a store, for example in a walrus or
//! chained assignment. Neither a borrowed parameter nor a view owns a binding.
//!
//! "Read afterwards" is `PointAvailability::last_reads`, the projection that
//! last-use releases read too. A raw carrier holds no reference, so its
//! position needs neither. Boxing it at the op is the backend's obligation: a
//! boxing that allocates hands its fresh reference to the op.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::passes::liveness::TirLivenessResult;
use crate::tir::passes::ownership_lattice_min::{
    DropEligibility, OperandTransfer, explicit_release_values, op_transferred_operands,
    terminator_branch_args, terminator_uses_root,
};
use crate::tir::values::ValueId;

/// What one taking operation takes from its holder.
#[derive(Default)]
struct Adoption {
    /// Roots whose own +1 the op takes, each at the first position naming it.
    moves: HashSet<ValueId>,
    /// Operands retained right before the op, in operand order, one per
    /// taking position that does not take its root's own +1.
    retains: Vec<ValueId>,
}

/// How every taking operation in the reachable blocks receives its
/// references, by block and operation index.
#[derive(Default)]
pub(super) struct TransferPlan {
    ops: HashMap<BlockId, HashMap<usize, Adoption>>,
}

impl TransferPlan {
    /// Plan each taking operation once. `last_reads` is the per-block read
    /// projection that last-use releases also consume. `holds_to_boundary`
    /// names the roots whose release belongs to a Python boundary that the
    /// way the op takes them does not end; they are never moved.
    /// `has_binding_custody` identifies existing Python binding owners whose
    /// custody must move into a binding store, rather than remain as a shadow.
    pub(super) fn compute(
        func: &TirFunction,
        blocks: &[BlockId],
        eligibility: &DropEligibility<'_>,
        live: &TirLivenessResult,
        last_reads: &HashMap<BlockId, HashMap<ValueId, usize>>,
        holds_to_boundary: &dyn Fn(ValueId, OperandTransfer) -> bool,
        has_binding_custody: &dyn Fn(ValueId) -> bool,
        describe_binding_custody: &dyn Fn(ValueId) -> String,
    ) -> Self {
        let canon = |value: ValueId| eligibility.root(value);
        let mut plan = Self::default();
        for &bid in blocks {
            let Some(reads) = last_reads.get(&bid) else {
                continue;
            };
            let block = &func.blocks[&bid];
            let taking: Vec<(usize, Vec<OperandTransfer>)> = block
                .ops
                .iter()
                .enumerate()
                .map(|(index, op)| (index, op_transferred_operands(op)))
                .filter(|(_, transfers)| {
                    transfers
                        .iter()
                        .any(|&transfer| transfer != OperandTransfer::Borrowed)
                })
                .collect();
            if taking.is_empty() {
                continue;
            }
            let forwarded: HashSet<ValueId> = terminator_branch_args(&block.terminator)
                .into_iter()
                .map(canon)
                .collect();
            let read_after = |root: ValueId, index: usize| {
                reads.get(&root).is_some_and(|&last| last > index)
                    || forwarded.contains(&root)
                    || live.is_live_out(bid, root)
                    || terminator_uses_root(&block.terminator, root, &canon)
            };
            let adoptions = plan.ops.entry(bid).or_default();
            for (index, transfers) in taking {
                let operands = &block.ops[index].operands;
                let borrowed: HashSet<ValueId> = operands
                    .iter()
                    .zip(&transfers)
                    .filter(|&(_, &transfer)| transfer == OperandTransfer::Borrowed)
                    .map(|(&operand, _)| canon(operand))
                    .collect();
                let mut adoption = Adoption::default();
                for (&operand, &transfer) in operands
                    .iter()
                    .zip(&transfers)
                    .filter(|&(_, &transfer)| transfer != OperandTransfer::Borrowed)
                {
                    let root = canon(operand);
                    if eligibility.is_raw_scalar_root(root) {
                        continue;
                    }
                    let movable = eligibility.is_droppable(root)
                        && !holds_to_boundary(root, transfer)
                        && !borrowed.contains(&root)
                        && !read_after(root, index);
                    assert!(
                        transfer != OperandTransfer::BindingStore
                            || !has_binding_custody(root)
                            || movable,
                        "DropInsertion({}): stale Python binding custody for {:?} at binding store {:?}:{}; store the binding once and read its view thereafter; operand={operand:?}; store={:?}; droppable={} boundary_hold={} borrowed_overlap={} later_read={} last_read={:?} forwarded={} live_out={}; binding_authority=[{}]; root_evidence=[{}]",
                        func.name,
                        root,
                        bid,
                        index,
                        block.ops[index],
                        eligibility.is_droppable(root),
                        holds_to_boundary(root, transfer),
                        borrowed.contains(&root),
                        read_after(root, index),
                        reads.get(&root),
                        forwarded.contains(&root),
                        live.is_live_out(bid, root),
                        describe_binding_custody(root),
                        describe_root(func, root, &canon)
                    );
                    if !(movable && adoption.moves.insert(root)) {
                        adoption.retains.push(operand);
                    }
                }
                adoptions.insert(index, adoption);
            }
        }
        plan
    }

    /// Whether the op at `op_index` of `block` takes `root`'s own +1.
    pub(super) fn moves(&self, block: BlockId, op_index: usize, root: ValueId) -> bool {
        self.ops
            .get(&block)
            .and_then(|adoptions| adoptions.get(&op_index))
            .is_some_and(|adoption| adoption.moves.contains(&root))
    }

    /// The operands to retain before each taking op of `block`, by op index.
    pub(super) fn retains(&self, block: BlockId) -> impl Iterator<Item = (usize, &[ValueId])> {
        self.ops
            .get(&block)
            .into_iter()
            .flatten()
            .map(|(&index, adoption)| (index, adoption.retains.as_slice()))
    }

    /// Every move, as the op after which its root's name owns nothing.
    pub(super) fn moved(&self) -> impl Iterator<Item = (BlockId, usize, ValueId)> {
        self.ops.iter().flat_map(|(&block, adoptions)| {
            adoptions.iter().flat_map(move |(&index, adoption)| {
                adoption.moves.iter().map(move |&root| (block, index, root))
            })
        })
    }

    /// Moves and retains, for the stage audit.
    pub(super) fn counts(&self) -> (usize, usize) {
        self.ops
            .values()
            .flat_map(HashMap::values)
            .fold((0, 0), |(moves, retains), adoption| {
                (
                    moves + adoption.moves.len(),
                    retains + adoption.retains.len(),
                )
            })
    }
}

/// Failure-only provenance: definition (including source index and original
/// family), block-argument identity, and explicit releases. Diagnostic strings
/// establish no custody, and this never dumps the complete function.
fn describe_root(func: &TirFunction, root: ValueId, canon: &dyn Fn(ValueId) -> ValueId) -> String {
    let mut blocks: Vec<_> = func.blocks.values().collect();
    blocks.sort_by_key(|block| block.id);
    let mut evidence = Vec::new();
    for block in blocks {
        for (position, argument) in block.args.iter().enumerate() {
            if argument.id == root {
                evidence.push(format!("argument {:?}:{position} {argument:?}", block.id));
            }
        }
        for (index, op) in block.ops.iter().enumerate() {
            if op.results.contains(&root)
                || explicit_release_values(op).any(|value| canon(value) == root)
            {
                evidence.push(format!("{:?}:{index} {op:?}", block.id));
            }
        }
    }
    evidence.join("; ")
}
