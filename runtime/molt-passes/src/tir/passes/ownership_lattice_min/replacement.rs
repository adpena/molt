//! A replaced result keeps the owner it had (design 20 §1.2).
//!
//! Value numbering, load forwarding, algebraic identities, tuple scalarization
//! and generator frame promotion prove an operation's result equal to a value
//! that already exists, and rewrite the operation into a `Copy` of that value.
//! Equal values are not equal owners. A result that held a reference of its own
//! may be a Python binding, which ends at its own boundary (`del`, a rebinding,
//! a frame clear) while the equal value is still read, or the reverse. A
//! transparent copy folds both names onto one reference: the first release
//! frees what the other still reads, and a second release frees it again.
//!
//! A replacing pass records each operation before it rewrites it
//! ([`Replacements::record`]), then calls [`Replacements::finish`]. A recorded
//! result keeps an owner only where the replaced operation's result contract
//! gave it one, by the root facts DropInsertion reads
//! (`OwnershipRootFacts::result_holds_own_reference`): the result was its own
//! alias root, and not the result of a `Copy` whose results hold no reference.
//! A frame binding view, a borrowed getter or an inert marker held none, and a
//! result that forwarded an operand's object (a no-op `TypeGuard`, a
//! transparent copy) held none of its own, so their copies stay transparent.
//! So does the copy of a raw carrier, by the raw facts DropInsertion reads, and
//! of an unread result, which names nothing.
//! Every other recorded copy becomes an owned alias (`binding_alias`), whose
//! lowering retains the object, so DropInsertion releases each name at its own
//! boundary. The alias keeps the replaced producer's Python-local provenance
//! (`bound_local`).
//!
//! An operation can also hold a reference that no result of it names, as a
//! generator frame holds each argument and each value stored in a slot. A pass
//! that removes one records the copy it places for that reference
//! ([`Replacements::record_held`]).

use std::collections::{HashMap, HashSet};

use crate::representation_facts::non_heap_values_for;
use crate::tir::ValueRangeResult;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::copy_kind_mints_owned_alias_ref_table;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::alias_analysis::{AliasUnionFind, build_alias_union_find};
use crate::tir::passes::liveness::compute_raw_scalars;
use crate::tir::values::ValueId;

use super::{OwnershipRootFacts, original_kind};

/// The owned-alias `Copy` kind: its lowering retains the operand, so the result
/// holds a reference of its own.
const OWNED_ALIAS_KIND: &str = "binding_alias";

/// The producer attr of a value bound to a Python local (#58).
const BOUND_LOCAL: &str = "bound_local";

/// An owned alias of `source`: `result` names the same object and holds a
/// reference of its own.
pub(crate) fn owned_alias(
    source: ValueId,
    result: ValueId,
    source_span: Option<(u32, u32)>,
) -> TirOp {
    let mut alias = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![source],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span,
    };
    mint_owner(&mut alias);
    alias
}

/// Gives a copy's result a reference of its own.
fn mint_owner(copy: &mut TirOp) {
    debug_assert!(copy_kind_mints_owned_alias_ref_table(OWNED_ALIAS_KIND));
    copy.attrs.insert(
        "_original_kind".into(),
        AttrValue::Str(OWNED_ALIAS_KIND.into()),
    );
}

/// The results one pass rewrites into copies of equal values.
pub(crate) struct Replacements {
    /// Alias roots before the pass rewrites anything.
    aliases: AliasUnionFind,
    /// Each replaced owner, and whether its producer bound a Python local.
    owners: HashMap<ValueId, bool>,
}

impl Replacements {
    /// Starts a pass's replacements, before the pass rewrites `func`.
    pub(crate) fn new(func: &TirFunction) -> Self {
        Self {
            aliases: build_alias_union_find(func),
            owners: HashMap::new(),
        }
    }

    /// Records `replaced` before the pass rewrites it into transparent copies,
    /// one per result, each of a value equal to that result. Only a result that
    /// held a reference of its own is kept.
    pub(crate) fn record(&mut self, replaced: &TirOp) {
        let bound = matches!(
            replaced.attrs.get(BOUND_LOCAL),
            Some(AttrValue::Bool(true))
        );
        for &result in &replaced.results {
            if OwnershipRootFacts::result_holds_own_reference(replaced, result, &self.aliases) {
                self.owners.insert(result, bound);
            }
        }
    }

    /// Records `result`, the transparent copy a pass placed for a reference
    /// that an operation it removed held of its own, which no result named.
    pub(crate) fn record_held(&mut self, result: ValueId) {
        self.owners.insert(result, false);
    }

    /// Makes the copy of each recorded owner that is read and names a heap
    /// reference an owned alias. `ranges` are the value ranges the pass holds
    /// for `func`; without them the raw facts are computed here, once.
    pub(crate) fn finish(self, func: &mut TirFunction, ranges: Option<&ValueRangeResult>) {
        if self.owners.is_empty() {
            return;
        }
        // A transparent copy carries its source's representation, so the
        // source decides whether the value holds a reference at all.
        let raw = match ranges {
            Some(ranges) => non_heap_values_for(func, ranges),
            None => compute_raw_scalars(func),
        };
        let mut read = HashSet::new();
        for block in func.blocks.values() {
            for op in &block.ops {
                read.extend(op.operands.iter().copied());
            }
            block.terminator.for_each_value(|value| {
                read.insert(value);
            });
        }
        for block in func.blocks.values_mut() {
            for op in &mut block.ops {
                if op.opcode != OpCode::Copy
                    || original_kind(op).is_some()
                    || op.operands.len() != 1
                    || op.results.len() != 1
                {
                    continue;
                }
                let result = op.results[0];
                let Some(&bound) = self.owners.get(&result) else {
                    continue;
                };
                if raw.contains(&op.operands[0]) || !read.contains(&result) {
                    continue;
                }
                mint_owner(op);
                if bound {
                    op.attrs.insert(BOUND_LOCAL.into(), AttrValue::Bool(true));
                }
            }
        }
    }
}
