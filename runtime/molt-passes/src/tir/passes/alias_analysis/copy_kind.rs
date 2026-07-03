//! Copy-lowering classification: the ownership taxonomy of `OpCode::Copy` ops.
//!
//! The SSA converter's `Copy` opcode is overloaded across every SimpleIR op that
//! lowers to a value move. These pure `_original_kind` classifiers decide, for
//! each copy, whether its result is a fresh owned reference, an owned alias, a
//! transparent (no-incref) alias, or an inert marker — the fact the RC drop pass
//! and the backends' explicit-lowering sets depend on. Split out of
//! `alias_analysis.rs` as a move-only decomposition; every classifier reads the
//! single-source op-kind registry (`op_kinds.toml` → `op_kinds_generated`).

use crate::tir::ops::{AttrValue, TirOp};

/// ── THE LOWERING-TRUTH ALIAS CONTRACT (the single source of truth) ──────────
///
/// `OpCode::Copy` is overloaded in the post-lowering TIR: most `Copy`s carry an
/// `_original_kind` naming the SimpleIR op they were lifted from (the SSA
/// converter folds every op WITHOUT a dedicated `OpCode` into `Copy`, stashing
/// the name in `_original_kind` — see `ssa::kind_to_opcode`'s `_ => Copy` arm).
/// Each such `Copy` falls into exactly one lowering class, and the RC
/// drop-insertion pass's *entire* over-release safety rests on classifying them
/// conservatively:
///
/// * [`CopyLowering::FreshValue`] — a value-producing op (container constructor,
///   `slice`, `str_from_obj`, `int_from_obj`, …) whose result is a NEW owned heap
///   reference returned by a dedicated runtime call. The result is an INDEPENDENT
///   alias root that the drop pass owns and releases on its own.
///   It is an EXPLICIT allow-list: a kind is `FreshValue` only when it is *proven*
///   to mint a fresh owned heap reference AND every drop-enabled backend lowers it
///   explicitly (LLVM: `lower_preserved_simpleir_op`; WASM/Luau: their
///   `_original_kind` dispatch). The LLVM `Copy` arm fails loud on any `FreshValue`
///   kind it did not lower (it would otherwise return operand 0 — a wrong result —
///   AND make the result silently alias operand 0, a drop-insertion double-free).
/// * [`CopyLowering::OwnedAlias`] - the result is operand 0's heap object bits,
///   but the lowering mints a new `+1` for the result binding. It is an
///   independent ownership root and MUST be explicitly lowered as retain+alias.
/// * [`CopyLowering::TransparentAlias`] — the result is operand 0's heap object,
///   bit-for-bit, with **no incref** (a pure SSA/var move, or a
///   validate-and-pass-through guard like `guard_tag`). The alias union-find unions
///   the result into operand 0's root: the two SSA handles share ONE owned
///   reference, dropped once at the group's last use. Treating such a `Copy` as a
///   fresh value would emit a second `DecRef` on the same object → **double-free**.
/// * [`CopyLowering::InertMarker`] — a debug / source-location / control-flow
///   marker (`line`, `trace_*`, `nop`, `missing`, the read-only `guard_*`s) that
///   produces no surviving *heap* reference (it yields nothing, or a raw bool).
///   The drop pass never drops it.
///
/// THE FAIL-CLOSED RULE (the keystone the adversarial review demanded). The
/// `_ =>` arm maps every UNKNOWN kind to [`CopyLowering::TransparentAlias`], NOT
/// `FreshValue`. This makes the set the drop pass treats as "produces a fresh
/// owned reference to release" a *proven SUBSET* of the kinds that actually mint
/// one — equivalently, the transparent-alias view is a *proven SUPERSET* of every
/// no-incref bit-passthrough lowering. The consequence is the only acceptable
/// failure direction:
///
/// > A kind we forgot to allow-list is treated as an alias of operand 0. If it is
/// > actually a fresh value, its `+1` is never released → a **leak**. It can NEVER
/// > be double-freed (a UAF), because the drop pass never emits an independent
/// > `DecRef` for a non-owned `Copy`.
///
/// Leak-not-UAF is exactly the rail the RC layer must hold (see the module-level
/// soundness model). The allow-lists and the LLVM backend's explicit-lowering set
/// are tied by [`copy_kind_mints_fresh_owned_ref`] and
/// [`copy_kind_mints_owned_alias_ref`]: the LLVM `Copy` arm fatals on an owned
/// result it did not lower (so an owned `Copy` is always explicitly lowered to
/// a +1, never a silent passthrough), and
/// `tests::copy_lowering_classes_are_total_and_disjoint` pins the bucket of every
/// representative kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyLowering {
    /// A fresh owned heap value produced by a dedicated runtime call (an
    /// independent alias root; the only class the drop pass releases on its own).
    FreshValue,
    /// Result is operand 0's heap object bits, but the lowering mints a new +1
    /// owned reference. This is an independent ownership root, not a transparent
    /// alias root.
    OwnedAlias,
    /// Result is operand 0's heap object, no incref (a transparent alias).
    TransparentAlias,
    /// A debug / control-flow marker producing no surviving heap reference.
    InertMarker,
}

/// The EXPLICIT allow-list of `_original_kind`s that mint a **fresh owned heap
/// reference** (the result is a brand-new object the holder must release exactly
/// once). A kind belongs here ONLY when both are true:
///   1. its runtime semantics return a NEW owned heap object (not operand 0, not a
///      raw scalar), and
///   2. EVERY drop-enabled backend (LLVM / WASM / Luau) lowers it explicitly.
///
/// This is the single gate the drop pass uses to decide whether a `Copy`'s own
/// result is an independent droppable reference. Anything NOT in this list is
/// treated as a transparent alias (fail-closed: leak, never UAF — see
/// [`CopyLowering`]).
///
/// CONSERVATIVE BY DESIGN. The list contains the value producers that are
/// *proven* to return a fresh owned reference (allocating constructors/conversions
/// plus the increfing iterators) AND are explicitly lowered by every drop-enabled
/// backend. It deliberately does NOT try to be exhaustive over every owned-result
/// op; one category is intentionally left to the fail-closed alias path:
/// getter-shaped ops whose ownership (owned vs borrowed) is not locally proven
/// (`dict_get`, `gen_send`, …). Conservatively aliasing them can at worst LEAK a
/// reference (never a UAF), the sanctioned failure direction for this layer.
/// Promoting such a kind to `FreshValue` requires *proving* its runtime returns a
/// fresh +1 (and is leak-tested), so a wrong guess can never turn a leak into a
/// double-free.
pub(crate) fn copy_kind_mints_fresh_owned_ref(kind: &str) -> bool {
    // The fresh-value allow-list is the single-source-of-truth op-kind registry
    // (`runtime/molt-ir/src/tir/op_kinds.toml`'s `classifier_fresh_value` +
    // `classifier_fresh_value_prefixes`, generated into
    // [`crate::tir::op_kinds_generated`]; see
    // `docs/design/foundation/25_op_kind_registry.md`). Membership means the
    // kind's runtime mints a fresh +1 owned reference that must be dropped on its
    // own. Editing the set means editing the table + regenerating; the sync test
    // (`tests/test_gen_op_kinds.py`) pins them in lock-step.
    //
    // Prefix form first (the `vec_*` vectorized-reduction family — each calls a
    // dedicated `molt_vec_*` runtime reduction returning a fresh boxed result),
    // then the exact set.
    use crate::tir::op_kinds_generated::{
        FRESH_VALUE_PREFIXES, copy_kind_mints_fresh_owned_ref_table,
    };
    if FRESH_VALUE_PREFIXES.iter().any(|p| kind.starts_with(p)) {
        return true;
    }
    copy_kind_mints_fresh_owned_ref_table(kind)
    // NOTE: staged variadic builders must only appear here after their backend
    // lowering consumes the ownership fact. `frozenset_new` is now in the table
    // with explicit LLVM lowering; `list_int_new` remains outside this generated
    // authority until its streamed-element shape has the same backend contract.
}

pub(crate) fn copy_kind_mints_owned_alias_ref(kind: &str) -> bool {
    crate::tir::op_kinds_generated::copy_kind_mints_owned_alias_ref_table(kind)
}

pub(crate) fn copy_kind_is_exception_creation_ref(kind: &str) -> bool {
    crate::tir::op_kinds_generated::copy_kind_is_exception_creation_ref_table(kind)
}

/// Classify a `Copy`'s `_original_kind` into its lowering class — THE single
/// source of truth for "does this `Copy` mint a fresh owned reference, alias
/// operand 0, or mark nothing?" See [`CopyLowering`]. FAIL-CLOSED to
/// `TransparentAlias` for any unrecognized kind.
pub(crate) fn classify_copy_kind(kind: Option<&str>) -> CopyLowering {
    // A bare `Copy` (no `_original_kind`) is the SSA converter's pure value move:
    // result := operand 0, same bits, no new reference.
    let Some(k) = kind else {
        return CopyLowering::TransparentAlias;
    };
    // Proven fresh-owned value producers (the explicit allow-list).
    if copy_kind_mints_fresh_owned_ref(k) {
        return CopyLowering::FreshValue;
    }
    // Proven owned aliases: same object bits as operand 0, but the lowering
    // mints an independent +1 for the result binding.
    if copy_kind_mints_owned_alias_ref(k) {
        return CopyLowering::OwnedAlias;
    }
    // ── Inert markers: no surviving heap reference to own. ──
    // `line` / `trace_*` / `missing` carry dedicated (RC-inert) backend
    // lowerings; `nop` is an explicit no-op. The read-only representation guards
    // (`guard_int`/`guard_float`/`guard_str`/`guard_bool`/`guard_none`) clobber
    // nothing and yield no droppable reference. The layout guards
    // (`guard_layout`/`guard_dict_shape`/`guard_layout_ptr`) produce a RAW BOOL
    // (`molt_guard_layout_ptr` → `from_bool`), never a heap reference —
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
    //    FAIL-CLOSED: an unrecognized fresh value mislabelled here leaks (its
    //    +1 is never released) but can never be double-freed, because the drop
    //    pass emits NO independent `DecRef` for a non-`FreshValue` `Copy`. ──
    CopyLowering::TransparentAlias
}

/// The RAW-CARRIER scalar type an overloaded `OpCode::Copy` produces, when (and
/// only when) the `Copy` is a value-CONVERSION whose result is carried in a raw
/// machine register (`I64`/`F64`/`Bool`) rather than the boxed NaN-box word.
///
/// `OpCode::Copy` is the SSA converter's fallback opcode for every SimpleIR op
/// without a dedicated [`OpCode`] (the name is stashed in `_original_kind`), so a
/// `Copy`'s result type is NOT, in general, operand 0's type. A full typed
/// counterpart of [`classify_copy_kind`] would map every `FreshValue` kind to its
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
/// `None` case — INCLUDING the heap-producing `FreshValue` copies (containers,
/// `str`, iterators, views, `range`, `slice`, `object_new`, `complex`): those
/// carry a boxed `DynBox` word, so propagating operand 0's (also-boxed) type is
/// already representationally correct, and NARROWING the fix to raw carriers keeps
/// the type lattice for heap values byte-identical to the pre-fix behavior. A
/// broader change (retyping a heap-producing copy away from operand 0) perturbs
/// CFG/optimization passes that key on heap-value types — observed as a
/// jump-label numbering regression in `_typing_strip_wrapping_parens` when
/// `enumerate`'s result was retyped — so it is deliberately out of scope: those
/// copies have no raw carrier and so cannot trigger the repr-mismatch class this
/// closes. Membership of [`copy_kind_mints_fresh_owned_ref`] is required so a
/// NON-fresh `Copy` whose `_original_kind` happens to collide with a conversion
/// name (there are none today) can never be misclassified.
pub(crate) fn copy_kind_raw_carrier_type(kind: Option<&str>) -> Option<crate::tir::types::TirType> {
    use crate::tir::types::TirType;
    let k = kind?;
    if !copy_kind_mints_fresh_owned_ref(k) {
        return None;
    }
    match k {
        // `int(x)` is a semantic `int` → `I64` (the repr lattice independently
        // boxes a BigInt result; the semantic-type axis is `I64`, exactly like
        // `ConstInt`). `float(x)` → `F64`. The `in` / `not in` membership test
        // (`x in c`, lowered to `contains`) → `bool` → `Bool`.
        "int_from_obj" | "int_from_str_of_obj" => Some(TirType::I64),
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
/// neither [`CopyLowering::FreshValue`] nor [`CopyLowering::OwnedAlias`]. The
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
        CopyLowering::FreshValue | CopyLowering::OwnedAlias
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
/// `line`/`trace_*` (debug markers), `missing` (unbound-cell sentinel), `nop`,
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
