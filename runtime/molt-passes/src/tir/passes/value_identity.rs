//! Shared value-identity facts for TIR passes.
//!
//! `OpCode::Copy` is both a structural SSA move and the fallback carrier for
//! SimpleIR spellings that have not become first-class TIR opcodes. This module
//! owns the fail-closed predicate for the subset of Copy ops that truly forward
//! one SSA value unchanged, so loop recognition, range analysis, representation
//! planning, and loop transforms cannot drift.

use std::{borrow::Cow, cell::OnceCell};

use molt_ir::deterministic_hash::HashMap;

use crate::tir::analysis::{Analysis, LoopForest, LoopForestResult};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    PredicateSemantics, opcode_negated_predicate_table, opcode_predicate_semantics,
};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// `Some(src)` when `op` is a `Copy` that holds the same value as one source.
///
/// Accepted cases are a plain SSA copy or a Copy carrying a known
/// value-forwarding SimpleIR spelling. Every operand must name the same source
/// value, so multi-operand stack-machine spellings such as `Copy(v, v)` remain
/// transparent while opaque or aggregating Copy fallbacks fail closed.
pub fn copy_value_source(op: &TirOp) -> Option<ValueId> {
    if op.opcode != OpCode::Copy || op.results.len() != 1 || op.operands.is_empty() {
        return None;
    }
    let kind_ok = match op.origin {
        None => true,
        Some(k) => matches!(
            k,
            molt_ir::OpKind::Copy
                | molt_ir::OpKind::CopyVar
                | molt_ir::OpKind::StoreVar
                | molt_ir::OpKind::LoadVar
                | molt_ir::OpKind::IdentityAlias
        ),
    };
    if !kind_ok {
        return None;
    }
    let src = op.operands[0];
    op.operands
        .iter()
        .all(|&operand| operand == src)
        .then_some(src)
}

/// The source whose heap identity and ownership root a result preserves.
/// This is not permission to erase an operation: runtime guards still read
/// their expected tag, may reject it, and count mismatches when profiling is on.
/// A mismatch returns the source unchanged and never proves its type. Their
/// declared shape selects two reads; the result aliases only operand zero.
pub fn no_heap_alias_source(op: &TirOp) -> Option<ValueId> {
    // Pure copy identity is a subset, including repeated identical operands.
    // Keep its one shape authority instead of narrowing existing copy facts.
    if let Some(source) = copy_value_source(op) {
        return Some(source);
    }
    use crate::tir::op_kinds_generated::{
        AliasTransparentAliasRole, copy_kind_is_explicit_no_heap_move_table,
        opcode_alias_transparent_alias_role_table, simpleir_op_shape,
    };
    if op.results.len() != 1 {
        return None;
    }
    let arity = match opcode_alias_transparent_alias_role_table(op.opcode) {
        AliasTransparentAliasRole::Copy => match op.origin {
            None => 1,
            Some(kind) if copy_kind_is_explicit_no_heap_move_table(kind) => {
                simpleir_op_shape(kind).map_or(1, |shape| shape.operands)
            }
            _ => return None,
        },
        _ => return None,
    };
    (op.operands.len() == arity)
        .then(|| op.operands.first().copied())
        .flatten()
}

/// Build a transitive copy-resolution map over value-forwarding Copy ops.
///
/// This map retains the existing identity/alias contract. Numeric-only clients
/// that need effectful scalar forwarding use `ScalarFacts::forwarding` instead.
pub(crate) fn build_copy_map(func: &TirFunction) -> HashMap<ValueId, ValueId> {
    let mut copy_of: HashMap<ValueId, ValueId> = HashMap::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(src) = copy_value_source(op) {
                copy_of.insert(op.results[0], src);
            }
        }
    }
    flatten_copy_map(&mut copy_of);
    copy_of
}

/// A Python object is identical to every unchanged SSA copy of itself, even
/// when its raw machine representation would need a new heap box at each use.
/// Distinct producers keep the object-word comparison.
pub(crate) fn fold_forwarded_identity(func: &mut TirFunction) -> bool {
    use crate::tir::ops::AttrValue;

    if !func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .any(|op| canonical_predicate(op.opcode).0 == OpCode::Is)
    {
        return false;
    }
    let copies = build_copy_map(func);
    let mut changed = false;
    for op in func.blocks.values_mut().flat_map(|block| &mut block.ops) {
        let (predicate, negated) = canonical_predicate(op.opcode);
        if let [left, right] = op.operands[..]
            && predicate == OpCode::Is
            && resolve_copy(&copies, left) == resolve_copy(&copies, right)
        {
            op.opcode = OpCode::ConstBool;
            op.operands.clear();
            op.origin = None;
            op.attrs.clear();
            op.attrs.insert("value".into(), AttrValue::Bool(!negated));
            changed = true;
        }
    }
    changed
}

/// The operand whose Python value and exact type a result preserves. A
/// Python integer can change machine representation. An instance is the
/// supplied object; its lifetime-stable class proof crosses the forwarding.
/// An owned alias has independent custody. A home store can release a
/// displaced owner and run callbacks: the result represents the supplied
/// value even when a later home read observes a reentrant write.
pub fn forwarded_python_value_source(op: &TirOp) -> Option<ValueId> {
    if let Some(source) = no_heap_alias_source(op) {
        return Some(source);
    }
    if op.opcode != OpCode::Copy || op.results.len() != 1 || op.operands.len() != 1 {
        return None;
    }
    let kind = op.origin?;
    crate::tir::op_kinds_generated::copy_kind_forwards_python_value_table(kind)
        .then_some(op.operands[0])
}

/// Exact scalar provenance and its forwarding projection for one immutable
/// SSA body. The shared borrow prevents reusing these facts after a rewrite.
pub(crate) struct ScalarFacts<'f> {
    body: &'f TirFunction,
    checked_calls: OnceCell<(
        Vec<crate::tir::blocks::BlockId>,
        crate::tir::type_refine::CheckedResults,
    )>,
    call_ranges: OnceCell<Vec<(ValueId, crate::tir::numeric_facts::IntRange)>>,
    types: OnceCell<Cow<'f, HashMap<ValueId, TirType>>>,
    forwarding: OnceCell<HashMap<ValueId, ValueId>>,
    loops: OnceCell<LoopForestResult>,
}

impl<'f> ScalarFacts<'f> {
    pub(crate) fn new(body: &'f TirFunction) -> Self {
        Self {
            body,
            checked_calls: OnceCell::new(),
            call_ranges: OnceCell::new(),
            types: OnceCell::new(),
            forwarding: OnceCell::new(),
            loops: OnceCell::new(),
        }
    }

    pub(crate) fn body(&self) -> &'f TirFunction {
        self.body
    }

    pub(crate) fn with_types(body: &'f TirFunction, types: &'f HashMap<ValueId, TirType>) -> Self {
        Self {
            types: OnceCell::from(Cow::Borrowed(types)),
            ..Self::new(body)
        }
    }

    pub(crate) fn loops(&self) -> &LoopForestResult {
        self.loops.get_or_init(|| LoopForest::compute(self.body))
    }

    pub(crate) fn types(&self) -> &HashMap<ValueId, TirType> {
        self.types
            .get_or_init(|| Cow::Owned(Self::solve_types(self.body, self.checked_calls())))
    }

    pub(crate) fn into_types(self) -> HashMap<ValueId, TirType> {
        self.types
            .into_inner()
            .map(Cow::into_owned)
            .unwrap_or_else(|| {
                self.checked_calls.get().map_or_else(
                    || crate::tir::type_refine::extract_exact_scalar_map(self.body),
                    |checked| Self::solve_types(self.body, checked),
                )
            })
    }

    fn checked_calls(
        &self,
    ) -> &(
        Vec<crate::tir::blocks::BlockId>,
        crate::tir::type_refine::CheckedResults,
    ) {
        self.checked_calls.get_or_init(|| {
            let order = crate::tir::dominators::executable_reverse_postorder(self.body);
            let checked = crate::tir::type_refine::CheckedResults::collect(self.body, &order);
            (order, checked)
        })
    }

    fn solve_types(
        body: &TirFunction,
        (order, checked): &(
            Vec<crate::tir::blocks::BlockId>,
            crate::tir::type_refine::CheckedResults,
        ),
    ) -> HashMap<ValueId, TirType> {
        use crate::tir::type_refine::{ExactScalarPlan, exact_scalar_map_from_plan};
        exact_scalar_map_from_plan(ExactScalarPlan::from_checked(
            body,
            Cow::Borrowed(order),
            Cow::Borrowed(checked),
        ))
    }

    pub(crate) fn checked_call_results(&self) -> molt_ir::deterministic_hash::HashSet<ValueId> {
        self.checked_calls().1.successful_call_results(self.body)
    }

    pub(crate) fn checked_call_ranges(&self) -> &[(ValueId, crate::tir::numeric_facts::IntRange)] {
        self.call_ranges.get_or_init(|| {
            self.checked_calls()
                .1
                .call_range_values(self.body, TirOp::exact_result_range)
        })
    }

    pub(crate) fn forwarding(&self) -> &HashMap<ValueId, ValueId> {
        self.forwarding
            .get_or_init(|| build_scalar_value_map(self.body, self.types()))
    }
}

/// Numeric-only fact resolution, including effectful scalar publications.
/// Never use this map to rewrite SSA uses, classify aliases or elide effects.
fn build_scalar_value_map(
    func: &TirFunction,
    exact: &HashMap<ValueId, TirType>,
) -> HashMap<ValueId, ValueId> {
    // Range clients also track mutable container lengths. Only a proven
    // immutable scalar may cross an effectful publication: its displaced
    // owner's finalizer could mutate a forwarded container. The exact solver
    // calls forwarded_python_value_source, not this map, so this query is non-recursive.
    let mut scalar_of = HashMap::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            // A checked success may have a narrower range than the physical
            // call word. Sharing a numeric root would publish that range back
            // into exceptional cleanup, where the word may be a sentinel.
            if op.checked_call_result_source().is_some() {
                continue;
            }
            let source = copy_value_source(op).or_else(|| {
                forwarded_python_value_source(op).filter(|source| exact.contains_key(source))
            });
            if let Some(source) = source {
                scalar_of.insert(op.results[0], source);
            }
        }
    }
    flatten_copy_map(&mut scalar_of);
    scalar_of
}

fn flatten_copy_map(copy_of: &mut HashMap<ValueId, ValueId>) {
    for _ in 0..64 {
        let mut changed = false;
        let keys: Vec<ValueId> = copy_of.keys().copied().collect();
        for key in keys {
            let value = copy_of[&key];
            if let Some(&deeper) = copy_of.get(&value)
                && deeper != value
            {
                copy_of.insert(key, deeper);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Resolve `value` through a copy map to its root source.
pub(crate) fn resolve_copy(copy_of: &HashMap<ValueId, ValueId>, value: ValueId) -> ValueId {
    copy_of.get(&value).copied().unwrap_or(value)
}

/// `opcode` as the predicate it states and whether it negates it: `IsNot`
/// is `Is` negated, `Not` is `Bool` negated (registry `negated_predicates`).
pub fn canonical_predicate(opcode: OpCode) -> (OpCode, bool) {
    match opcode_negated_predicate_table(opcode) {
        Some(positive) => (positive, true),
        None => (opcode, false),
    }
}

/// A truth test with its truth wrappers removed.
///
/// `Bool(x)` is true exactly when `x` is truthy and `Not(x)` exactly when it
/// is not; a value-forwarding copy keeps the value. Peeling follows these to
/// the first other producer: the original value is truthy on every execution
/// exactly when `core` is truthy, or exactly when it is not if `negated`. The
/// wrappers still execute (`Bool` may call `__bool__`); this is a fact about
/// their results, not permission to erase them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TruthTest {
    pub core: ValueId,
    pub negated: bool,
}

/// Peel `value`'s truth wrappers. `producer` names each value's defining op;
/// the walk is bounded by `limit` steps so a malformed cycle stays finite.
pub fn peel_truth_wrappers<'f>(
    value: ValueId,
    limit: usize,
    producer: impl Fn(ValueId) -> Option<&'f TirOp>,
) -> TruthTest {
    let mut test = TruthTest {
        core: value,
        negated: false,
    };
    for _ in 0..=limit {
        let Some(op) = producer(test.core) else {
            break;
        };
        if let Some(source) = copy_value_source(op) {
            test.core = source;
            continue;
        }
        if opcode_predicate_semantics(op.opcode) != Some(PredicateSemantics::Truth) {
            break;
        }
        let (_, flips) = canonical_predicate(op.opcode);
        let [operand] = op.operands[..] else {
            break;
        };
        test = TruthTest {
            core: operand,
            negated: test.negated != flips,
        };
    }
    test
}
