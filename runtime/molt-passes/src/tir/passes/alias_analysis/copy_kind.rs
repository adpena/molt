//! Copy-lowering classification: the ownership taxonomy of `OpCode::Copy` ops.
//!
//! The SSA converter's `Copy` opcode is overloaded across every SimpleIR op that
//! lowers to a value move. These pure `_original_kind` classifiers decide, for
//! each copy, whether its result is a independently owned reference, an owned alias, a
//! transparent (no-incref) alias, or an inert marker — the fact the RC drop pass
//! and the backends' explicit-lowering sets depend on. Split out of
//! `alias_analysis.rs` as a move-only decomposition; every classifier reads the
//! single-source op-kind registry (`op_kinds.toml` → `op_kinds_generated`).

use crate::tir::ops::{AttrValue, TirOp};

/// Result custody for operations carried by `Copy` with `_original_kind`.
/// An owned result is an independent reference obligation, not a newly
/// allocated object. Public namespace lookup may return any published object.
/// The result must not inherit operand zero's identity or type. Allocation
/// and escape facts remain in the separate generated allocation authority.
///
/// Unknown spellings fail closed to the non-owning category. Only an explicit
/// generated ownership contract permits drop insertion to release a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyLowering {
    /// Independent boxed result custody; immediates own no heap reference.
    /// Heap identity may alias any existing object with a separate +1.
    OwnedValue,
    /// Exactly operand zero's object, with a separate +1 for this result.
    OwnedAlias,
    /// A non-owning result. Only the separate no-heap-move fact proves aliasing.
    TransparentAlias,
    /// A semantically inert marker with no surviving heap reference.
    /// Trace frame lifecycle uses the effectful non-owning category.
    InertMarker,
}

/// The runtime returns an independent +1 and every drop-enabled backend
/// lowers the operation explicitly. This does not prove fresh allocation,
/// callable identity, or disjointness from any other object's storage.
pub(crate) fn copy_kind_mints_owned_value(kind: &str) -> bool {
    // Exact result ownership comes from the canonical operation registry.
    // A similar spelling supplies no runtime or reference-ownership contract.
    crate::tir::op_kinds_generated::copy_kind_mints_owned_value_table(kind)
}

pub(crate) fn copy_kind_mints_owned_alias_ref(kind: &str) -> bool {
    crate::tir::op_kinds_generated::copy_kind_mints_owned_alias_ref_table(kind)
}

pub(crate) fn copy_kind_is_exception_creation_ref(kind: &str) -> bool {
    crate::tir::op_kinds_generated::copy_kind_is_exception_creation_ref_table(kind)
}

/// Classify a `Copy`'s `_original_kind` into its lowering class — THE single
/// source of truth for "does this `Copy` mint a independently owned reference, alias
/// operand 0, or mark nothing?" See [`CopyLowering`]. FAIL-CLOSED to
/// `TransparentAlias` for any unrecognized kind.
pub(crate) fn classify_copy_kind(kind: Option<&str>) -> CopyLowering {
    // A bare `Copy` (no `_original_kind`) is the SSA converter's pure value move:
    // result := operand 0, same bits, no new reference.
    let Some(k) = kind else {
        return CopyLowering::TransparentAlias;
    };
    // Proven owned-result value producers (the explicit allow-list).
    if copy_kind_mints_owned_value(k) {
        return CopyLowering::OwnedValue;
    }
    // Proven owned aliases: same object bits as operand 0, but the lowering
    // mints an independent +1 for the result binding.
    if copy_kind_mints_owned_alias_ref(k) {
        return CopyLowering::OwnedAlias;
    }
    // ── Inert markers: no surviving heap reference to own. ──
    // `line` / `missing` carry dedicated (RC-inert) backend
    // lowerings; `nop` is an explicit no-op. The read-only representation guards
    // (`guard_int`/`guard_float`/`guard_str`/`guard_bool`/`guard_none`) clobber
    // nothing and yield no droppable reference. The layout guards
    // (`guard_layout`/`guard_dict_shape`/`guard_layout`) produce a RAW BOOL
    // (`molt_guard_layout` → `from_bool`), never a heap reference —
    // drop-irrelevant — and clobber no heap memory. The set is the registry's
    // `classifier_inert_marker` (op_kinds.toml, generated into
    // [`crate::tir::op_kinds_generated`]; docs/design/foundation/25).
    if crate::tir::op_kinds_generated::copy_kind_is_inert_marker_table(k) {
        return CopyLowering::InertMarker;
    }
    // Known runtime/effect ops that intentionally keep the same fail-closed
    // droppability as the default transparent-alias bucket, but are table-visible
    // so future ownership promotions cannot hide in the `_ =>` arm. This is NOT
    // the no-heap-move/MemGVN alias set.
    if crate::tir::op_kinds_generated::copy_kind_is_explicit_transparent_alias_table(k) {
        return CopyLowering::TransparentAlias;
    }
    // ── Everything else (incl. the explicit pure moves `copy`/`copy_var`/
    //    `store_var`/`load_var`/`identity_alias`, the pass-through guards
    //    `guard_tag`/`guard_type`, AND any UNKNOWN kind) → transparent alias.
    //    FAIL-CLOSED: an unrecognized owned value mislabelled here leaks (its
    //    +1 is never released) but can never be double-freed, because the drop
    //    pass emits NO independent `DecRef` for a non-`OwnedValue` `Copy`. ──
    CopyLowering::TransparentAlias
}

/// The RAW-CARRIER scalar type an overloaded `OpCode::Copy` produces, when (and
/// only when) the `Copy` is a value-CONVERSION whose result is carried in a raw
/// machine register (`I64`/`F64`/`Bool`) rather than the boxed NaN-box word.
///
/// `OpCode::Copy` is the SSA converter's fallback opcode for every SimpleIR op
/// without a dedicated [`OpCode`] (the name is stashed in `_original_kind`), so a
/// `Copy`'s result type is NOT, in general, operand 0's type. A full typed
/// counterpart of [`classify_copy_kind`] would map every `OwnedValue` kind to its
/// produced type; but the ONLY observable miscompile is the RAW-CARRIER scalar
/// conversions, where a wrong type is a representation error (a raw register
/// stored into a differently-typed variable/phi slot). The keystone is `int(t)`
/// with `t: float`, which lowers to `Copy[int_from_obj](t)`: `type_refine`'s plain
/// `Copy => operand_types.first()` rule aliased its type to `t`'s `F64`, flooding
/// the integer accumulator chain (and its loop-carried/join phis) with a spurious
/// `float` carrier — observed as a native Cranelift `def_var` repr mismatch (an
/// `i64` value stored into an `F64`-declared join slot, `_seconds_float_to_sec_nsec`)
/// and the matching LIR-verifier branch-repr divergence.
///
/// Returns `Some(I64/F64/Bool)` for exactly those scalar conversions, `None` for
/// every other `Copy`. The caller keeps its existing operand-0 propagation for the
/// `None` case — INCLUDING the heap-producing `OwnedValue` copies (containers,
/// `str`, iterators, views, `range`, `slice`, `object_new`, `complex`): those
/// carry a boxed `DynBox` word, so propagating operand 0's (also-boxed) type is
/// already representationally correct, and NARROWING the fix to raw carriers keeps
/// the type lattice for heap values byte-identical to the pre-fix behavior. A
/// broader change (retyping a heap-producing copy away from operand 0) perturbs
/// CFG/optimization passes that key on heap-value types — observed as a
/// jump-label numbering regression in `_typing_strip_wrapping_parens` when
/// `enumerate`'s result was retyped — so it is deliberately out of scope: those
/// copies have no raw carrier and so cannot trigger the repr-mismatch class this
/// closes. Membership of [`copy_kind_mints_owned_value`] is required so a
/// NON-fresh `Copy` whose `_original_kind` happens to collide with a conversion
/// name (there are none today) can never be misclassified.
pub(crate) fn copy_kind_raw_carrier_type(kind: Option<&str>) -> Option<crate::tir::types::TirType> {
    use crate::tir::types::TirType;
    let k = kind?;
    if !copy_kind_mints_owned_value(k) {
        return None;
    }
    match k {
        // `int(x)` is a semantic `int` → `I64` (the repr lattice independently
        // boxes a BigInt result; the semantic-type axis is `I64`, exactly like
        // `ConstInt`), and so is `operator.index(x)`, range()'s bound
        // conversion. `float(x)` → `F64`. The `in` / `not in` membership test
        // (`x in c`, lowered to `contains`) → `bool` → `Bool`.
        "int_from_obj" | "int_from_str_of_obj" | "operator_index" => Some(TirType::I64),
        "float_from_obj" => Some(TirType::F64),
        "contains" => Some(TirType::Bool),
        _ => None,
    }
}

/// Returns whether an `OpCode::Copy` op is an EXPLICIT transparent local alias:
/// its result PROVABLY names operand 0's heap object (bit-for-bit, no incref). The
/// alias union-find unions the result into operand 0's root, so this MUST be
/// PRECISE — a false union would let MemGVN forward a store from one object to a
/// load from a *different* object (a miscompile). Therefore it is the EXPLICIT
/// no-incref pass-through set only (bare `Copy`, the named SSA/var moves, and the
/// validate-and-pass-through guards `guard_tag`/`guard_type` whose runtime returns
/// operand 0 unchanged); an UNKNOWN kind is NOT unioned (it gets its own root).
///
/// This is intentionally DISTINCT from the drop pass's fail-closed droppability
/// rule: the union-find fails closed to "NOT an alias" (precise, MemGVN-safe),
/// while the drop pass separately fails closed to "do NOT release" (leak-safe,
/// see `drop_insertion`'s `copy_result_is_owned_ref`). The two axes fail closed
/// in opposite directions, so they use different predicates — collapsing them
/// re-creates either a MemGVN miscompile or a drop-pass double-free.
pub(super) fn copy_is_known_local_alias(op: &TirOp) -> bool {
    copy_kind_is_explicit_no_heap_move(copy_original_kind(op))
}

/// Returns whether an `OpCode::Copy` op is an EXPLICIT no-heap-footprint pure
/// move: a bare `Copy`, one of the named SSA/var moves, or a validate-and-pass-
/// through guard (`guard_tag`/`guard_type`). These provably touch NO heap memory
/// (the result is operand 0; no allocation, no store), so they are
/// [`MemRegion::ScalarRegister`] for MemGVN/SROA, and their result aliases operand
/// 0 for the union-find. An UNKNOWN kind is NOT a pure move — its memory effects
/// are unknown (an unmapped op like `list_append` mutates the heap) and its result
/// is not provably operand 0, so it stays `GenericHeap` / its own alias root.
pub(crate) fn copy_kind_is_explicit_no_heap_move(kind: Option<&str>) -> bool {
    // The explicit no-heap-move set is the registry's `classifier_no_heap_move`
    // (op_kinds.toml, generated into [`crate::tir::op_kinds_generated`];
    // docs/design/foundation/25). A bare `Copy` with no `_original_kind` is the
    // SSA converter's pure value move and is likewise a no-heap move.
    match kind {
        None => true,
        Some(k) => crate::tir::op_kinds_generated::copy_kind_is_explicit_no_heap_move_table(k),
    }
}

/// The `_original_kind` string of an op, if present.
#[inline]
pub(super) fn copy_original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

/// True if a `Copy` with `_original_kind = kind` is SOUND to lower as a plain
/// no-incref bit-passthrough of operand 0 (or as an inert marker) — i.e. it is
/// neither [`CopyLowering::OwnedValue`] nor [`CopyLowering::OwnedAlias`]. The
/// LLVM backend's `Copy` arm gates its passthrough on this: an owned result that
/// was not explicitly lowered would return operand 0 without the required retain,
/// making ownership silently disagree with runtime refcounts.
///
/// Gated to the `llvm` feature (plus `test`): the only non-test caller is the
/// LLVM `Copy` arm's fatal gate (`llvm_backend::lowering`), so under a non-LLVM
/// profile (e.g. `--features native-backend`) this predicate would otherwise be
/// dead code and fail the `-D warnings` clippy gate. The drop pass and the
/// always-compiled alias/memory-region axes consume `classify_copy_kind` /
/// `copy_kind_is_explicit_no_heap_move` directly, not this LLVM-specific view.
#[cfg(any(feature = "llvm", test))]
pub fn copy_kind_reaches_no_incref_passthrough(kind: Option<&str>) -> bool {
    !matches!(
        classify_copy_kind(kind),
        CopyLowering::OwnedValue | CopyLowering::OwnedAlias
    )
}

/// True if a `Copy`-carried `_original_kind` op writes/reads/clobbers NO heap
/// memory — a debug / source-location / control-flow marker or a read-only guard
/// the SSA lift carries as a `Copy` (it has no dedicated `OpCode`). These are
/// classified [`MemRegion::ScalarRegister`] so they do not spuriously bump the
/// memory version between adjacent field accesses (which would starve MemGVN
/// store-to-load forwarding and SROA — see [`AliasAnalysisResult::region_of`]).
///
/// FAIL-CLOSED: every kind classified inert is *proven* heap-inert —
/// `line` (source location), `missing` (unbound-cell sentinel), `nop`,
/// and the read-only representation/layout `guard_*`s (they read a class/layout
/// version and may raise, but never write a field). Any other kind keeps the
/// conservative `GenericHeap` classification.
///
/// Delegates to the single-source-of-truth [`classify_copy_kind`]: a `Copy` is
/// memory-inert iff its kind classifies as [`CopyLowering::InertMarker`]. (A bare
/// `Copy` with no `_original_kind` is a `TransparentAlias`, NOT inert — its
/// region is handled by the alias path in [`AliasAnalysisResult::region_of`].)
pub(super) fn copy_kind_is_memory_inert(op: &TirOp) -> bool {
    matches!(
        classify_copy_kind(copy_original_kind(op)),
        CopyLowering::InertMarker
    )
}
