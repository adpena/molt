//! Escape-analysis classifiers: the escape lattice, per-use record, and the
//! predicate helpers that decide whether an op is an allocation site, a pure
//! SSA-move copy, a container-builder passthrough, or a borrowing call. See the
//! module-level docs on [`super`].

use crate::tir::op_kinds_generated::{
    copy_kind_is_explicit_no_heap_move_table, kind_result_absorbs_operand_ownership_table,
    opcode_is_escape_alloc_site_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode};
use crate::tir::values::ValueId;

use super::super::effects;

/// Escape lattice for allocated values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EscapeState {
    /// Value never leaves the function — safe to stack allocate.
    NoEscape = 0,
    /// Passed to a callee that doesn't store it (future refinement).
    ArgEscape = 1,
    /// Stored to heap/global or returned — must heap allocate.
    GlobalEscape = 2,
}

/// A recorded use of an alloc'd value.
#[derive(Debug)]
pub(super) struct UseInfo {
    /// The opcode that uses the value.
    pub(super) opcode: OpCode,
    /// All operands of the using op (for Store target analysis).
    pub(super) operands: Vec<ValueId>,
    /// Index of our value within the operands list.
    pub(super) operand_index: usize,
    /// Attribute dictionary from the using op (for callee name lookup).
    pub(super) attrs: AttrDict,
}

/// Extract a string attribute value from an `AttrDict`.
pub(super) fn attr_str<'a>(attrs: &'a AttrDict, key: &str) -> Option<&'a str> {
    match attrs.get(key) {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Returns `true` when an `OpCode::Copy` op is a genuine SSA move (its result
/// aliases its operand — the same heap object), as opposed to the opaque
/// `_original_kind` passthrough that `kind_to_opcode` assigns to SimpleIR ops
/// without a dedicated TIR opcode.
///
/// A move has either no `_original_kind` (a true SSA-lift copy) or an
/// `_original_kind` the generated registry proves is a no-heap move of operand 0
/// (the named SSA/var moves plus the validate-and-pass-through guards). Anything
/// else under `Copy` is a passthrough whose result is a *distinct* value (e.g. a
/// freshly built container), so it must NOT be aliased to its operand.
///
/// The kind set is the single generated authority `op_kinds.toml`
/// `classifier_no_heap_move` (`copy_kind_is_explicit_no_heap_move_table`), shared
/// with `alias_analysis.rs` and the ownership lattice — escape analysis no longer
/// keeps a private hand-list that could diverge. Every alias-propagation site
/// below reads `op.operands.first()` / `op.results.first()`, which matches the
/// registry's operand-0 pure-move contract for all those kinds (including the
/// guard passthroughs), so consuming the broader authority is sound and only
/// tightens the alias relation toward the rest of the compiler.
pub(super) fn is_pure_move_copy(attrs: &AttrDict) -> bool {
    match attr_str(attrs, "_original_kind") {
        None => true,
        Some(kind) => copy_kind_is_explicit_no_heap_move_table(kind),
    }
}

/// Returns `true` when an `OpCode::Copy` op is the passthrough carrier for a
/// container constructor. Such ops absorb their operand lifetimes into a new
/// container that may outlive the frame, so every operand must be treated as
/// escaping — exactly like the first-class `BuildList`/`BuildDict`/… opcodes.
pub(super) fn is_container_builder_passthrough(attrs: &AttrDict) -> bool {
    attr_str(attrs, "_original_kind").is_some_and(kind_result_absorbs_operand_ownership_table)
}

/// Returns `true` if the named builtin only borrows (reads) its arguments and
/// never stores them into heap-reachable locations.
///
/// We use the effects system as the source of truth: any builtin that is
/// `effect_free` cannot store its arguments (storing is a side effect).
/// Additionally, builtins like `print`, `isinstance`, `type`, etc. that have
/// I/O or introspection effects but still never *capture* their arguments are
/// included explicitly.
pub(super) fn is_borrowing_builtin(name: &str) -> bool {
    // If the effects system classifies it as effect_free, it borrows.
    if effects::builtin_effects(name).is_some_and(|fx| fx.effect_free) {
        return true;
    }
    // Builtins that have side effects (I/O) but never store their arguments.
    matches!(
        name,
        "print"
            | "type"
            | "isinstance"
            | "issubclass"
            | "hasattr"
            | "getattr"
            | "id"
            | "iter"
            | "next"
            | "any"
            | "all"
            | "vars"
            | "dir"
            | "format"
    )
}

/// Returns `true` if a `CallMethod` op only borrows its operands (receiver and
/// arguments) without storing them.
///
/// Uses the effects system: a method that is `effect_free` on an immutable
/// receiver type cannot capture its arguments. Falls back to `false` for
/// unknown receiver types or methods (conservative).
///
/// Supports two encodings of method identity on the SSA `attrs` dict:
///
/// 1. **Frontend canonical form (production)**: `method` is the
///    full `BoundMethod:<receiver_type>:<method_name>` string copied
///    from the SimpleIR `s_value` of `call_method` ops.  This is
///    what the frontend's `_emit_dynamic_call` produces for
///    monomorphic builtin-method dispatches and what the native
///    backend's `s_value` match arm expects to see at codegen
///    (`function_compiler.rs:16489+`).  We parse the receiver and
///    method out inline so the existing effects table
///    (`("list", "append")`, `("str", "upper")`, …) matches.
///
/// 2. **Test / future-refined form**: `method` is a bare method
///    name AND `receiver_type` is a separate attr.  This is what
///    the existing unit tests use, and what a future SSA-lift
///    refinement would emit if we ever derive receiver type from
///    the receiver value's `TirType` directly.
///
/// The two encodings are equivalent contracts; the parse logic
/// here lets a single effects-table lookup serve both.
pub(super) fn is_borrowing_method_call(attrs: &AttrDict) -> bool {
    let method_attr = match attr_str(attrs, "method") {
        Some(m) => m,
        None => return false,
    };
    let (receiver_type, method) = if let Some(rest) = method_attr.strip_prefix("BoundMethod:") {
        // Frontend canonical form: split on the first ':' to
        // recover (receiver_type, method_name).  Both halves
        // must be non-empty for the lookup to succeed.
        let mut parts = rest.splitn(2, ':');
        match (parts.next(), parts.next()) {
            (Some(rcv), Some(mthd)) if !rcv.is_empty() && !mthd.is_empty() => (rcv, mthd),
            _ => return false,
        }
    } else {
        // Test / future-refined form: bare method name plus
        // explicit receiver_type attr.
        let receiver_type = match attr_str(attrs, "receiver_type") {
            Some(rt) => rt,
            None => return false,
        };
        (receiver_type, method_attr)
    };
    effects::method_effects(receiver_type, method).is_some_and(|fx| fx.effect_free)
}

/// Returns `true` if this opcode is an allocation site whose result we
/// want to track for escape state.
///
/// * `Alloc` — generic heap blocks.
/// * `ObjectNewBound` — class-instance allocation from the frontend's
///   class-instantiation fold.
/// * `BuildList` / `BuildDict` / `BuildTuple` / `BuildSet` / `AllocTask` —
///   container / task allocation sites (S5 phase 1). Tracking these as escape
///   roots lets the alias analysis classify a freshly-built container's escape
///   state; it is sound because `apply` only ever *rewrites* `Alloc` /
///   `ObjectNewBound` opcodes (never the `Build*` family), so adding these
///   roots can only refine the escape map, never change which ops get
///   stack-promoted.
#[inline]
pub(super) fn is_alloc_site(opcode: OpCode) -> bool {
    opcode_is_escape_alloc_site_table(opcode)
}

/// Return the operand that carries the stored value for StoreAttr-family ops.
///
/// The TIR opcode intentionally groups several SimpleIR store variants behind
/// `StoreAttr`; the preserved `_original_kind` defines operand roles for the
/// variants whose attribute name/class guard is also an SSA operand.
pub(super) fn store_attr_value_operand_index(
    attrs: &AttrDict,
    operand_count: usize,
) -> Option<usize> {
    let value_index = match attr_str(attrs, "_original_kind") {
        Some("set_attr_name") => 2,
        Some("guarded_field_set") | Some("guarded_field_init") => 3,
        Some("set_attr") | Some("store_attr") if operand_count >= 3 => 2,
        _ => 1,
    };
    (value_index < operand_count).then_some(value_index)
}
