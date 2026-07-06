use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::{AliasAnalysisResult, AliasUnionFind};
use crate::tir::values::ValueId;

pub(super) fn terminator_uses_root(
    terminator: &Terminator,
    root: ValueId,
    aliases: &AliasUnionFind,
) -> bool {
    let mut uses_root = |value: &ValueId| aliases.root(*value) == root;
    match terminator {
        Terminator::Branch { args, .. } => args.iter().any(&mut uses_root),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            uses_root(cond)
                || then_args.iter().any(&mut uses_root)
                || else_args.iter().any(&mut uses_root)
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            uses_root(value)
                || cases
                    .iter()
                    .any(|(_, _, args)| args.iter().any(&mut uses_root))
                || default_args.iter().any(&mut uses_root)
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            cases
                .iter()
                .any(|(_, _, args)| args.iter().any(&mut uses_root))
                || default_args.iter().any(&mut uses_root)
        }
        Terminator::Return { values } => values.iter().any(&mut uses_root),
        Terminator::Unreachable => false,
    }
}

/// The alias-roots that are referenced in a block OTHER than the one that
/// produces them - i.e. the roots that escape their producing block via a
/// dominance-based cross-block SSA use, a block-argument binding, or a
/// terminator reference. Pattern 2 must keep a stack object's final stores live
/// when its root is in this set: such an object is observable downstream and its
/// constructed fields are read after this block.
///
/// We first map every value to its producing block (op result, or block-argument
/// binding), then scan every reference (op operand, terminator-referenced value,
/// and branch/cond/switch argument) and union the *referencing* block into the
/// root's use set. A root escapes when its use set contains any block other than
/// the one that produced it.
///
/// This is intentionally a CONSERVATIVE SUPERSET: a value with no recorded
/// producer block (e.g. a function parameter, or a root that is only ever an
/// operand) is treated as escaping the moment it is referenced in two distinct
/// blocks, and any reference whose producer is unknown is treated as escaping.
/// Over-reporting an escape only makes Pattern 2 keep more stores live (strictly
/// safe). Under-reporting would re-open the silent zero-default miscompile, so
/// the analysis fails closed.
pub(super) fn compute_escaping_roots(
    func: &TirFunction,
    alias: &AliasAnalysisResult,
) -> HashSet<ValueId> {
    // value-root -> the single block that produces it (None marks "seen in >1
    // producing block" or "producer unknown", which forces escaping treatment).
    let mut producer: HashMap<ValueId, Option<BlockId>> = HashMap::new();
    let mut note_producer = |root: ValueId, bid: BlockId| {
        producer
            .entry(root)
            .and_modify(|slot| {
                if *slot != Some(bid) {
                    *slot = None;
                }
            })
            .or_insert(Some(bid));
    };
    for (&bid, block) in func.blocks.iter() {
        for arg in &block.args {
            note_producer(alias.root(arg.id), bid);
        }
        for op in &block.ops {
            for result in &op.results {
                note_producer(alias.root(*result), bid);
            }
        }
    }

    let mut escaping: HashSet<ValueId> = HashSet::new();
    let note_use = |root: ValueId, bid: BlockId, escaping: &mut HashSet<ValueId>| {
        match producer.get(&root) {
            // Referenced outside its single producing block => escapes.
            Some(Some(prod)) if *prod != bid => {
                escaping.insert(root);
            }
            Some(Some(_)) => {}
            // Ambiguous/unknown producer => fail closed.
            _ => {
                escaping.insert(root);
            }
        }
    };
    for (&bid, block) in func.blocks.iter() {
        for op in &block.ops {
            for operand in &op.operands {
                note_use(alias.root(*operand), bid, &mut escaping);
            }
        }
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for a in args {
                    note_use(alias.root(*a), bid, &mut escaping);
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                note_use(alias.root(*cond), bid, &mut escaping);
                for a in then_args.iter().chain(else_args.iter()) {
                    note_use(alias.root(*a), bid, &mut escaping);
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                note_use(alias.root(*value), bid, &mut escaping);
                for (_, _, args) in cases {
                    for a in args {
                        note_use(alias.root(*a), bid, &mut escaping);
                    }
                }
                for a in default_args {
                    note_use(alias.root(*a), bid, &mut escaping);
                }
            }
            // `StateDispatch` has no condition value; only its per-edge args.
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                for (_, _, args) in cases {
                    for a in args {
                        note_use(alias.root(*a), bid, &mut escaping);
                    }
                }
                for a in default_args {
                    note_use(alias.root(*a), bid, &mut escaping);
                }
            }
            Terminator::Return { values } => {
                for a in values {
                    note_use(alias.root(*a), bid, &mut escaping);
                }
            }
            Terminator::Unreachable => {}
        }
    }
    escaping
}
