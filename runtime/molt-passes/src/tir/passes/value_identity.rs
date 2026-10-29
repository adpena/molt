//! Shared value-identity facts for TIR passes.
//!
//! `OpCode::Copy` is both a structural SSA move and the fallback carrier for
//! SimpleIR spellings that have not become first-class TIR opcodes. This module
//! owns the fail-closed predicate for the subset of Copy ops that truly forward
//! one SSA value unchanged, so loop recognition, range analysis, representation
//! planning, and loop transforms cannot drift.

use std::collections::HashMap;

use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
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
        AliasTransparentAliasRole::TypeGuard if !op.origin.is_some() => 1,
        _ => return None,
    };
    (op.operands.len() == arity)
        .then(|| op.operands.first().copied())
        .flatten()
}

/// Build a transitive copy-resolution map over value-forwarding Copy ops.
///
/// This map retains the existing identity/alias contract. Numeric-only clients
/// that need effectful scalar forwarding use `build_scalar_value_map` instead.
pub(crate) fn build_copy_map(func: &TirFunction) -> HashMap<ValueId, ValueId> {
    let mut copy_of: HashMap<ValueId, ValueId> = HashMap::new();
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

/// The operand supplying a result's mathematical scalar value, provided that
/// operand already has a scalar proof. A home publication returns its saved
/// operand view even if releasing the displaced owner reentrantly writes the
/// home. It does not prove the home's later contents or a subsequent load.
///
/// This is NOT heap identity, ownership, purity, callback freedom or permission
/// to erase/move the operation. In particular a home store still consumes its
/// operand, publishes the binding and releases its displaced owner.
pub fn scalar_value_source(op: &TirOp) -> Option<ValueId> {
    if let Some(source) = no_heap_alias_source(op) {
        return Some(source);
    }
    if op.opcode != OpCode::Copy || op.results.len() != 1 || op.operands.len() != 1 {
        return None;
    }
    let Some(kind) = op.origin else {
        return None;
    };
    crate::tir::op_kinds_generated::copy_kind_forwards_scalar_value_table(kind)
        .then_some(op.operands[0])
}

/// Numeric-only fact resolution, including effectful scalar publications.
/// Never use this map to rewrite SSA uses, classify aliases or elide effects.
pub fn build_scalar_value_map(func: &TirFunction) -> HashMap<ValueId, ValueId> {
    // Range clients also track mutable container lengths. Only a proven
    // immutable scalar may cross an effectful publication: its displaced
    // owner's finalizer could mutate a forwarded container. The exact solver
    // calls scalar_value_source, not this map, so this query is non-recursive.
    let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
    let mut scalar_of = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            let source = copy_value_source(op)
                .or_else(|| scalar_value_source(op).filter(|source| exact.contains_key(source)));
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
