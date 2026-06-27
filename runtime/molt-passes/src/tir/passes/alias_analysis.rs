//! First-class alias analysis for TIR — Tier-0 substrate **S5, Phase 1**.
//!
//! Before this module, FOUR independent ad-hoc "barrier" lists answered slightly
//! different versions of the same memory-aliasing question, each hand-maintained
//! and free to drift apart:
//!
//! | Old list                                  | Question it answered                                   |
//! |-------------------------------------------|--------------------------------------------------------|
//! | `refcount_elim::is_barrier(opcode)`       | could this op capture/store/observe *any* refcount?    |
//! | `reuse_analysis::is_aliasing_op(op,val)`  | could this op alias/observe the memory of `val`?       |
//! | `dead_store_elim::may_observe_slot`       | could this op read/escape the slot of object `root`?   |
//! | `escape_analysis` per-use classification  | does this alloc'd value escape the function?           |
//!
//! Four lists ⇒ four chances to forget an opcode, and a *missed* barrier is the
//! worst possible bug in this layer: a wrong (too-permissive) barrier lets RC
//! elimination / reuse / dead-store-elim drop an operation that was actually
//! observable, producing a **use-after-free or a leak**. This module collapses
//! all four into ONE oracle whose queries are, by construction, a **conservative
//! superset** of each old list (verified op-by-op in the tests below).
//!
//! ## What this analysis computes
//!
//! [`AliasAnalysisResult`] is the cached value (an S1 [`Analysis`]). It carries:
//!
//! * an [`AliasUnionFind`] — transparent-SSA-copy / typeguard alias roots
//!   (promoted verbatim from `dead_store_elim`'s former inline `AliasState`),
//! * a points-to / escape map (`escape: HashMap<ValueId, EscapeState>`, the
//!   former `escape_analysis::analyze` result, now anchored here),
//! * the [`MemRegion`] taxonomy classifying every memory-touching op's region,
//! * the [`LoadPurity`] gate distinguishing a proven-pure typed-slot load
//!   (`guarded_field_get` / `load` against a known concrete-class offset) from
//!   an opaque attribute lookup (`get_attr*`) that **MayDispatch** a user
//!   `__getattr__` / `__getattribute__` and is therefore opaque.
//!
//! and exposes the queries the three barrier-consuming passes need:
//!
//! * [`AliasAnalysisResult::is_rc_barrier`] — replaces `refcount_elim::is_barrier`.
//! * [`AliasAnalysisResult::is_barrier_for`] — replaces `reuse_analysis::is_aliasing_op`.
//! * [`AliasAnalysisResult::may_observe_slot`] — replaces `dead_store_elim::may_observe_slot`.
//! * [`AliasAnalysisResult::escape_state`] / [`AliasAnalysisResult::escape`] —
//!   replaces direct `escape_analysis::analyze` calls.
//!
//! ## Soundness model: CONSERVATIVE SUPERSET, FAIL-CLOSED
//!
//! Every barrier query is monotone in the direction of *more* barriers: when in
//! doubt, it returns `true`. The proof obligation discharged by the tests is, for
//! each old list `L` and its replacement `Q`:
//!
//! > ∀ (op, value). `L(op, value)` ⇒ `Q(op, value)`
//!
//! (`Q` may be strictly more conservative — that only ever costs a missed
//! optimization, never correctness.) The `MemRegion` / `LoadPurity` refinements
//! are *additive precision* layered on top of the superset core; they never make
//! a query *less* conservative than the old list it replaces.
//!
//! ### The Python-dunder soundness gate
//!
//! A `LoadAttr` / `Index` is classified [`LoadPurity::ProvenPure`] **only** when
//! it is a typed-slot access against a statically-known concrete class with no
//! `__getattr__` / `__getattribute__` override — i.e. its `_original_kind` is one
//! of the offset-based field accessors (`guarded_field_get`, `load`) that the
//! frontend emits exclusively for proven-concrete-class field reads. Every other
//! attribute spelling (`get_attr`, `get_attr_name`, `get_attr_generic_*`) and
//! every `Index` is [`LoadPurity::MayDispatch`]: it can run arbitrary user code
//! and is treated as fully opaque (a barrier). Conservative-false on any doubt.

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    AliasMemoryRegionClass, AliasSlotObservation, AliasTransparentAliasRole, AliasTypedSlotRole,
    opcode_alias_memory_region_table, opcode_alias_slot_observation_table,
    opcode_alias_transparent_alias_role_table, opcode_alias_typed_slot_role_table,
    opcode_is_alias_heap_barrier_table, opcode_is_alias_rc_barrier_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

pub use super::escape_analysis::EscapeState;

// ===========================================================================
// MemRegion taxonomy
// ===========================================================================

/// Abstract memory region a memory-touching op reads or writes. A TBAA-style
/// partition: two ops can only alias if their regions *may* overlap
/// ([`MemRegion::may_alias`]). It is always sound to widen an op's region to
/// [`MemRegion::GenericHeap`]; every refinement below is a *proven* disjointness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemRegion {
    /// A specific typed field of a concrete user class at a fixed byte offset:
    /// `obj.<offset>`, where the class is the one the field op's own guard proved
    /// (the `_class` attr the frontend stamps on every offset-based field op —
    /// see [`typed_slot_field`]). Two `TypedField`s disjoint-alias iff they differ
    /// in class id OR offset. We do NOT track object identity, so two same-class,
    /// same-offset fields of possibly-different objects STILL may-alias
    /// (conservative-true; proof obligation (b), see [`MemRegion::may_alias`]).
    TypedField { class: String, offset: i64 },
    /// An element of a container (list/dict/set/tuple) reached through
    /// `Index` / `StoreIndex`. Opaque: may dispatch `__getitem__` / `__setitem__`.
    ContainerElement,
    /// A module dictionary slot (`Module*` opcodes). Globally visible, outlives
    /// every function frame.
    ModuleDict,
    /// A stack-allocated object (`StackAlloc` / `ObjectNewBoundStack`) that is
    /// proven not to escape. Distinct region per allocation root.
    StackObject { root: ValueId },
    /// A scalar SSA register value with no heap footprint — touching it cannot
    /// alias any heap region.
    ScalarRegister,
    /// Unknown / conservative heap region. Aliases everything heap-resident.
    GenericHeap,
}

impl MemRegion {
    /// Conservative may-alias relation between two regions. `true` means the two
    /// regions *might* name overlapping memory — the analysis must assume they
    /// do. The only disjoint pairs are the ones we can *prove* cannot overlap.
    ///
    /// * `TypedField`s are disjoint only when class id or offset differs;
    ///   same-class/same-offset fields may-alias (object identity is untracked —
    ///   proof obligation (b): two `p.x` reads on possibly-different `Point`s must
    ///   still be treated as the same memory). Cross-class and cross-offset fields
    ///   are provably disjoint: distinct concrete classes never share an object,
    ///   and distinct offsets are distinct bytes of one instance layout.
    /// * A `TypedField` is disjoint from a `ContainerElement` and from a
    ///   `ModuleDict` slot: a class instance's fixed-layout slot is a different
    ///   kind of memory from a container's element storage or a module object's
    ///   attr dict (proof obligation (a)).
    /// * A `ScalarRegister` never aliases a heap region. Two distinct
    ///   `StackObject` roots are disjoint. Everything else falls back to "may
    ///   alias" (notably `GenericHeap`, which an opaque call/raise/yield widens
    ///   to — proof obligation (c)).
    pub fn may_alias(&self, other: &MemRegion) -> bool {
        use MemRegion::*;
        match (self, other) {
            // A scalar register has no heap footprint at all.
            (ScalarRegister, _) | (_, ScalarRegister) => false,
            // Distinct typed fields (different class or offset) are disjoint;
            // same class+offset may-alias (object identity untracked, oblig. (b)).
            (
                TypedField {
                    class: c1,
                    offset: o1,
                },
                TypedField {
                    class: c2,
                    offset: o2,
                },
            ) => c1 == c2 && o1 == o2,
            // Distinct stack objects never overlap; the same root does.
            (StackObject { root: r1 }, StackObject { root: r2 }) => r1 == r2,
            // A non-escaping stack object cannot be named by a generic-heap,
            // container, module, or typed-field access of a *different* object:
            // a proven-non-escaping object is unreachable through any of those.
            (StackObject { .. }, _) | (_, StackObject { .. }) => false,
            // TypedField vs ContainerElement / ModuleDict / GenericHeap: a typed
            // class slot is a distinct kind of memory from a container element
            // or module dict slot, but a GenericHeap access is opaque and may
            // name anything.
            (TypedField { .. }, GenericHeap) | (GenericHeap, TypedField { .. }) => true,
            (TypedField { .. }, ContainerElement) | (ContainerElement, TypedField { .. }) => false,
            (TypedField { .. }, ModuleDict) | (ModuleDict, TypedField { .. }) => false,
            // ContainerElement vs ModuleDict: distinct memory kinds.
            (ContainerElement, ModuleDict) | (ModuleDict, ContainerElement) => false,
            // Same-kind opaque regions, or anything paired with GenericHeap,
            // may alias.
            _ => true,
        }
    }
}

// ===========================================================================
// LoadPurity — the Python-dunder soundness gate
// ===========================================================================

/// Whether a load (`LoadAttr` / `Index`) is a proven side-effect-free read of a
/// known memory slot, or may dispatch arbitrary user code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPurity {
    /// A typed-slot field read against a statically-known concrete class with no
    /// `__getattr__` / `__getattribute__` override (`_original_kind` ∈
    /// {`guarded_field_get`, `load`}). Pure: reads exactly one offset, runs no
    /// Python code, cannot mutate observable state.
    ProvenPure,
    /// An opaque attribute / index access that may dispatch a user dunder
    /// (`__getattr__`, `__getattribute__`, `__getitem__`) with arbitrary side
    /// effects. Fully opaque — treated as a barrier.
    MayDispatch,
}

/// `_original_kind` values that the frontend emits *exclusively* for a
/// proven-concrete-class typed-slot field read (a fixed byte offset, no dunder
/// dispatch). Mirrors `ssa.rs`'s `kind_to_opcode` LoadAttr arm, partitioned by
/// whether the spelling can run Python code.
///
/// `guarded_field_get` carries a class guard + offset; `load` is the lowered
/// fixed-offset slot read. Everything else under `LoadAttr`
/// (`get_attr`, `get_attr_name`, `get_attr_generic_ptr`, `get_attr_generic_obj`)
/// is a generic attribute lookup that goes through the full
/// `__getattribute__` / `__getattr__` protocol and is therefore `MayDispatch`.
fn load_attr_is_typed_slot(attrs: &AttrDict) -> bool {
    match attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => matches!(kind.as_str(), "guarded_field_get" | "load"),
        // A `LoadAttr` with NO `_original_kind` is a raw SSA-lift attribute
        // read; conservatively opaque (it may be a generic get_attr that lost
        // its kind annotation). Only an *explicit* typed-slot kind proves
        // purity.
        _ => false,
    }
}

/// Classify a load op's purity. Only `LoadAttr` typed-slot reads are
/// `ProvenPure`; `Index` (and every opaque attribute spelling) is `MayDispatch`.
fn classify_load(op: &TirOp) -> LoadPurity {
    match op.opcode {
        OpCode::LoadAttr if load_attr_is_typed_slot(&op.attrs) => LoadPurity::ProvenPure,
        _ => LoadPurity::MayDispatch,
    }
}

// ===========================================================================
// AliasUnionFind — transparent-copy alias roots
// ===========================================================================

/// Union-find over transparent SSA aliases (pure `Copy` moves and no-op
/// `TypeGuard`s). Two values share a root iff they provably name the same heap
/// object through a chain of transparent copies. Promoted verbatim from
/// `dead_store_elim`'s former inline `AliasState`, now the single home for
/// SSA-copy alias roots.
/// Build the transparent-alias union-find for `func` via a single forward scan
/// over every op (Phase A of [`AliasAnalysisResult::compute`]). Exposed so the
/// liveness analysis (RC drop-insertion substrate, design 20) can canonicalize
/// values to their alias root WITHOUT computing the (heavier) escape/points-to
/// half: a `Copy`/`TypeGuard` borrow alias holds no new reference, so ownership
/// — and therefore drop placement — is per alias root, not per SSA value.
pub fn build_alias_union_find(func: &TirFunction) -> AliasUnionFind {
    let mut aliases = AliasUnionFind::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            aliases.record_transparent_aliases(op);
        }
    }
    aliases
}

// ===========================================================================
// Borrow provenance — the interior-borrow keepalive relation (RC drop-insertion
// substrate, design 20).
// ===========================================================================

/// The operand value `op`'s result interior-borrows (a BORROW into — or an opaque
/// HANDLE indexing — that operand's backing store), or `None`. Such a result keeps
/// its source object semantically alive: freeing the source (running its
/// finalizer) can invalidate the result.
///
/// REGISTRY-DRIVEN (design 27 §1.5 / §2.1, op-semantics ladder #73): the borrow-of
/// fact is no longer a hardcoded `LoadAttr | Index` match here — it is the
/// per-position `operand_ownership = "interior_borrow_keepalive"` row in
/// `op_kinds.toml`, generated into
/// [`crate::tir::op_kinds_generated::opcode_borrows_source_operand`] (which returns
/// the interior-borrowed operand INDEX, or `None`). The single declarative
/// authority means a FUTURE op whose result borrows into an operand (a `memoryview`
/// op, a slice-view intrinsic) gets correct keepalive by setting that operand's
/// position in op_kinds.toml — never by editing this function — retiring the
/// per-pass borrow-of hand list (the C4 interior-borrow-lifetime class).
///
/// The fact it encodes (byte-identical to the prior match): `LoadAttr` / `Index`
/// interior-borrow operand 0. `Index`'s key operand and `OrdAt` (an `i64` code
/// point copied out of the element, not a reference into the container) carry NO
/// keepalive — they are classified `borrowed` / left off the table.
///
/// This is DISTINCT from the transparent-alias relation (`copy_is_known_local_alias`):
/// a borrow result is NOT bit-identical to the source and must NOT be unioned into
/// the source's alias root (that would let MemGVN forward a store on the source to
/// a load of the result — a miscompile). It is a one-directional LIVENESS coupling
/// only: "the source must outlive this result."
///
/// FAIL-CLOSED (conservative superset). Every `LoadAttr` and `Index` is treated as
/// potentially borrowing, including the `ProvenPure` typed-slot forms. For an
/// owned-result load (the common case — a normal `obj.field` whose result carries
/// its own `+1`) the coupling only DEFERS the source's drop to after the result's
/// last use, which is harmless (a slightly later drop, never a leak, never a UAF).
/// For the borrow / opaque-handle case it is mandatory for soundness:
///
/// > The intrinsic-handle stdlib classes (`collections.Counter`, …) store their
/// > native data in a global registry keyed by a RAW-INTEGER handle held in an
/// > instance slot (`self._handle`). The fast-path lowering inlines `len(c)` /
/// > `c[k]` as `h = get_attr(c, "_handle")` then `molt_counter_len(h)` /
/// > `molt_counter_getitem(h, k)`. The handle `h` is a raw int (no refcount), and
/// > the registry entry is owned by the wrapper's `__del__` (`molt_counter_drop`).
/// > If the drop pass releases the wrapper `c` at its last DIRECT operand use (the
/// > `get_attr`), the wrapper's finalizer destroys the registry entry BEFORE the
/// > intrinsic call reads `h` → the call sees an empty/destroyed counter (the
/// > round-6 BLOCKER-1 use-after-free: `len(Counter(...))` returned 0).
fn op_borrow_source(op: &TirOp) -> Option<ValueId> {
    let idx = crate::tir::op_kinds_generated::opcode_borrows_source_operand(op.opcode)?;
    op.operands.get(idx).copied()
}

/// The interior-borrow keepalive relation for a function: maps each borrowing-read
/// result (the result of a [`OpCode::LoadAttr`] / [`OpCode::Index`]) to the alias
/// ROOT of the source object it borrows from. Both the liveness analysis and the
/// drop pass consume this single relation so the source-object liveness is extended
/// — identically — through the borrow result's uses (see [`op_borrow_source`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BorrowProvenance {
    /// borrow-result value → alias root of its immediate source object.
    immediate_source: HashMap<ValueId, ValueId>,
}

impl BorrowProvenance {
    /// The transitive set of source-object alias roots that `value` borrows from —
    /// the roots that must remain live wherever `value` is live. Empty when `value`
    /// is not (transitively) a borrow result. Resolves chains
    /// (`h2 = LoadAttr(h1); h1 = LoadAttr(obj)` → using `h2` keeps both `h1`'s root
    /// and `obj`'s root alive). `canon` maps any value to its transparent-alias
    /// root (so a `Copy` of a borrow result resolves to the same sources).
    pub fn keepalive_roots(
        &self,
        value: ValueId,
        canon: &dyn Fn(ValueId) -> ValueId,
    ) -> Vec<ValueId> {
        let mut out: Vec<ValueId> = Vec::new();
        let mut seen: HashSet<ValueId> = HashSet::new();
        // Seed with the value's own root and the value itself (a borrow result may
        // be referenced either by its raw SSA id or through a transparent Copy).
        let mut work: Vec<ValueId> = vec![value, canon(value)];
        while let Some(v) = work.pop() {
            if !seen.insert(v) {
                continue;
            }
            if let Some(&src_root) = self.immediate_source.get(&v) {
                if out.iter().all(|&r| r != src_root) {
                    out.push(src_root);
                }
                // The source root may itself be a borrow result (chain). Walk it.
                work.push(src_root);
                work.push(canon(src_root));
            }
        }
        out
    }

    /// True if the relation is empty (no borrowing reads in the function) — lets a
    /// consumer skip the per-use keepalive walk entirely on the common path.
    pub fn is_empty(&self) -> bool {
        self.immediate_source.is_empty()
    }
}

/// Build the [`BorrowProvenance`] relation for `func`. Keyed by the borrow-result
/// SSA id; the value is the source's alias root (canonicalized through the shared
/// transparent-alias union-find, so a borrow of a `Copy`-aliased object records the
/// underlying owned root). One forward scan, mirroring [`build_alias_union_find`].
pub fn build_borrow_provenance(func: &TirFunction, aliases: &AliasUnionFind) -> BorrowProvenance {
    let mut bp = BorrowProvenance::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            let Some(src) = op_borrow_source(op) else {
                continue;
            };
            let src_root = aliases.root(src);
            for &result in &op.results {
                // A self-referential edge (result aliases its own source) would
                // loop the keepalive walk; the `seen` guard in `keepalive_roots`
                // already breaks cycles, but never record an identity edge.
                if aliases.root(result) == src_root {
                    continue;
                }
                bp.immediate_source.insert(result, src_root);
            }
        }
    }
    bp
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AliasUnionFind {
    parent: HashMap<ValueId, ValueId>,
}

impl AliasUnionFind {
    /// The representative root of `value` (follows the transparent-alias chain).
    pub fn root(&self, value: ValueId) -> ValueId {
        let mut root = value;
        while let Some(next) = self.parent.get(&root).copied() {
            if next == root {
                break;
            }
            root = next;
        }
        root
    }

    /// True if any operand of `op` shares the alias root `root`.
    pub fn operand_aliases_root(&self, op: &TirOp, root: ValueId) -> bool {
        op.operands
            .iter()
            .any(|operand| self.root(*operand) == root)
    }

    /// If `op` is a transparent alias-producing op, union its results into the
    /// alias root of its source operand.
    fn record_transparent_aliases(&mut self, op: &TirOp) {
        let Some(root) = transparent_alias_root(op, self) else {
            return;
        };
        for result in &op.results {
            self.parent.insert(*result, root);
        }
    }
}

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
fn copy_is_known_local_alias(op: &TirOp) -> bool {
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
fn copy_original_kind(op: &TirOp) -> Option<&str> {
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
fn copy_kind_is_memory_inert(op: &TirOp) -> bool {
    matches!(
        classify_copy_kind(copy_original_kind(op)),
        CopyLowering::InertMarker
    )
}

/// The transparent-alias root an op contributes, if any. A no-op `TypeGuard` and
/// a pure-move `Copy` both forward their single operand's root. Mirrors
/// `dead_store_elim`'s former `transparent_alias_root`.
fn transparent_alias_root(op: &TirOp, aliases: &AliasUnionFind) -> Option<ValueId> {
    if op.results.is_empty() {
        return None;
    }
    match opcode_alias_transparent_alias_role_table(op.opcode) {
        AliasTransparentAliasRole::TypeGuard => {
            if op.attrs.contains_key("_original_kind") || op.operands.len() != 1 {
                return None;
            }
            Some(aliases.root(op.operands[0]))
        }
        AliasTransparentAliasRole::Copy => {
            if !copy_is_known_local_alias(op) || op.operands.is_empty() {
                return None;
            }
            let root = aliases.root(op.operands[0]);
            if op
                .operands
                .iter()
                .all(|operand| aliases.root(*operand) == root)
            {
                Some(root)
            } else {
                None
            }
        }
        AliasTransparentAliasRole::NotTransparentAlias => None,
    }
}

// ===========================================================================
// Typed-slot store helpers (shared with dead_store_elim's contract)
// ===========================================================================

/// `Some(offset)` when this op is a `store` / `store_init` against a typed-class
/// instance slot at a known integer offset. Mirrors `dead_store_elim::store_offset`.
///
/// Scoped to the PLAIN raw-offset store forms (operands `[obj, val]`); the
/// `guarded_field_set` / `guarded_field_init` forms carry a different operand
/// ABI and are handled by [`typed_slot_field`].
fn store_offset(op: &TirOp) -> Option<i64> {
    if op.opcode != OpCode::StoreAttr {
        return None;
    }
    let original = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    if !matches!(original, "store" | "store_init") {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(v)) => Some(*v),
        _ => None,
    }
}

/// `Some((target, offset))` for the narrow PLAIN typed-class slot store contract
/// (`store` / `store_init`, operands `[obj, val]`). Mirrors
/// `dead_store_elim::typed_slot_store`; that overwrite contract is restricted to
/// the two-operand form, so this helper stays scoped to it. The wider
/// region-classification set is [`typed_slot_field`].
fn typed_slot_store(op: &TirOp) -> Option<(ValueId, i64)> {
    if op.operands.len() != 2 {
        return None;
    }
    Some((op.operands[0], store_offset(op)?))
}

/// The `_original_kind` spellings of every offset-based typed-slot field op the
/// frontend emits **exclusively** for a proven fixed-layout concrete-class field
/// — partitioned by load vs store. Each is emitted only when the object's class
/// is proven at the op (a preceding runtime version-guard for the
/// `guarded_field_*` forms, static type inference for the plain `store`/`load`
/// forms) AND the attribute resolves to a fixed instance-layout byte offset (the
/// `offset is None` fallback in the frontend routes a `__dict__` /
/// exception-subclass / metaclass attribute to a generic `get_attr*` / `set_attr*`
/// spelling that classifies as `GenericHeap`). Discharges proof obligation (a):
/// a typed-slot field op can never name a container element or a module-dict slot.
fn typed_slot_field_kind(op: &TirOp) -> Option<&'static str> {
    let original = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    match opcode_alias_typed_slot_role_table(op.opcode) {
        AliasTypedSlotRole::Load => match original {
            "load" | "guarded_field_get" => Some("load"),
            _ => None,
        },
        AliasTypedSlotRole::Store => match original {
            "store" | "store_init" | "guarded_field_set" | "guarded_field_init" => Some("store"),
            _ => None,
        },
        AliasTypedSlotRole::NotTypedSlot => None,
    }
}

/// `Some((obj_root_operand, offset))` for ANY typed-slot field op (load or store,
/// plain or guarded), WITHOUT requiring the class identity. `obj` is always
/// operand[0] across both ABIs (plain `[obj, val]` / `[obj]`; guarded
/// `[obj, class_bits, expected_version, (val)]`). This is the object+offset
/// identity a `StackObject` region needs — a proven-non-escaping object's slot is
/// keyed by the allocation root alone, so it stays precise even when the op
/// carries no `_class` attr (a cached pre-S5-1.5 TIR artifact).
fn typed_slot_obj_offset(op: &TirOp) -> Option<(ValueId, i64)> {
    typed_slot_field_kind(op)?;
    let obj = *op.operands.first()?;
    let offset = match op.attrs.get("value") {
        Some(AttrValue::Int(v)) => *v,
        _ => return None,
    };
    Some((obj, offset))
}

/// The concrete class the frontend proved at this typed-slot field op (its
/// `_class` attr — the class whose layout authored the op's `offset`). FAIL-CLOSED:
/// `None` when the op is not a typed-slot field op OR carries no `_class` proof
/// (a cached pre-S5-1.5 artifact, or a future spelling that dropped the attr) — in
/// which case the region stays `GenericHeap`.
fn typed_slot_class(op: &TirOp) -> Option<String> {
    typed_slot_field_kind(op)?;
    match op.attrs.get("_class") {
        Some(AttrValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

// ===========================================================================
// The barrier core — conservative superset of all four old lists
// ===========================================================================

/// The opcode-only "could this op capture/store/observe a reference count"
/// predicate. This is the EXACT superset core that `refcount_elim::is_barrier`
/// required, plus the additional ops that only ever *add* barriers (it is sound
/// to over-barrier RC pairing). Operand-agnostic by design: an RC barrier blocks
/// pairing regardless of which value the op touches.
///
/// Superset obligation vs the old `refcount_elim::is_barrier`: every opcode in
/// that list ({Call, CallMethod, CallBuiltin, StoreAttr, StoreIndex, StateSwitch,
/// StateTransition, StateYield, ClosureLoad, ClosureStore, ChanSendYield,
/// ChanRecvYield}) is present here. Exception-control transfer is also a
/// barrier: `Raise` does not fall through, and `CheckException` / `TryStart`
/// carry implicit handler edges whose payload retains are consumed only on that
/// exceptional path. Verified in `tests::rc_barrier_is_superset_*`.
fn opcode_is_rc_barrier(opcode: OpCode) -> bool {
    opcode_is_alias_rc_barrier_table(opcode)
}

/// True if `opcode` may observe / mutate / escape *arbitrary* heap memory — the
/// opcode-only half of the may-alias barrier. Conservative superset of the
/// opcode portion of `reuse_analysis::is_aliasing_op`.
///
/// Superset obligation vs `reuse_analysis::is_aliasing_op`'s opcode list
/// ({Call, CallMethod, CallBuiltin, StoreAttr, StoreIndex, Raise, Yield,
/// YieldFrom, StateSwitch, StateTransition, StateYield, ChanSendYield,
/// ChanRecvYield, ClosureStore, Free}): every one is present here. Verified in
/// `tests::reuse_barrier_is_superset`.
fn opcode_is_heap_barrier(opcode: OpCode) -> bool {
    opcode_is_alias_heap_barrier_table(opcode)
}

fn aliasing_op_may_observe_slot(op: &TirOp, root: ValueId, aliases: &AliasUnionFind) -> bool {
    match opcode_alias_slot_observation_table(op.opcode) {
        AliasSlotObservation::DirectObserver | AliasSlotObservation::ConservativeObserver => true,
        AliasSlotObservation::TypedSlotStore => match typed_slot_store(op) {
            Some((target, _)) => aliases.root(target) != root,
            None => true,
        },
        AliasSlotObservation::TransparentAlias => transparent_alias_root(op, aliases).is_none(),
        AliasSlotObservation::NeverObserver => false,
    }
}

// ===========================================================================
// AliasAnalysisResult
// ===========================================================================

/// The cached alias-analysis result for one function. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct AliasAnalysisResult {
    /// Transparent-SSA-copy alias roots.
    pub aliases: AliasUnionFind,
    /// Points-to / escape lattice for every tracked allocation root.
    pub escape: HashMap<ValueId, EscapeState>,
    /// Allocation roots tracked by the escape analysis (alloc-site results +
    /// their transparent-move aliases).
    pub alloc_roots: HashSet<ValueId>,
}

impl AliasAnalysisResult {
    /// Compute the result for `func`. Builds the alias union-find by a single
    /// forward scan, then folds the (now alias-aware) escape analysis on top.
    /// `pub(crate)` so module-phase transforms (which have no per-function
    /// `AnalysisManager`) can compute it directly; per-function passes go
    /// through `am.get::<AliasAnalysis>()` for caching.
    pub fn compute(func: &TirFunction) -> Self {
        // Phase A: build the transparent-alias union-find with a forward scan.
        let aliases = build_alias_union_find(func);

        // Phase B: the escape / points-to map. This is the former
        // `escape_analysis::analyze`, anchored here as the points-to half of the
        // unified alias analysis. Its borrowing logic (effect-free builtins /
        // methods don't capture) lives in `escape_analysis` and is reused.
        let escape = super::escape_analysis::analyze(func);
        let alloc_roots: HashSet<ValueId> = escape.keys().copied().collect();

        Self {
            aliases,
            escape,
            alloc_roots,
        }
    }

    /// Escape state of `value` (defaults to `NoEscape` for untracked values —
    /// they are not allocation roots and have nothing to escape).
    #[inline]
    pub fn escape_state(&self, value: ValueId) -> EscapeState {
        self.escape
            .get(&value)
            .copied()
            .unwrap_or(EscapeState::NoEscape)
    }

    /// Read-only view of the full escape map.
    #[inline]
    pub fn escape(&self) -> &HashMap<ValueId, EscapeState> {
        &self.escape
    }

    /// The transparent-alias root of `value`.
    #[inline]
    pub fn root(&self, value: ValueId) -> ValueId {
        self.aliases.root(value)
    }

    /// Replaces `refcount_elim::is_barrier`. True if `op` is a barrier that
    /// prevents IncRef/DecRef pairing across it: the op may capture, store, or
    /// observe a reference count. Operand-agnostic (an RC barrier blocks pairing
    /// for every value).
    ///
    /// CONSERVATIVE SUPERSET of the old `is_barrier(opcode)`: see
    /// [`opcode_is_rc_barrier`].
    #[inline]
    pub fn is_rc_barrier(&self, op: &TirOp) -> bool {
        opcode_is_rc_barrier(op.opcode)
    }

    /// Replaces `reuse_analysis::is_aliasing_op`. True if `op` might alias with
    /// or observe the memory of `val` (so a `DecRef(val) … Alloc` reuse window
    /// must close here).
    ///
    /// CONSERVATIVE SUPERSET of the old predicate: an op aliases if it (a) takes
    /// `val` (or a transparent alias of it) as a direct operand, OR (b) is an
    /// opcode that may observe/mutate/escape arbitrary heap memory
    /// ([`opcode_is_heap_barrier`]). The old list compared operands by raw
    /// `ValueId` equality; routing through the alias root is *strictly more
    /// conservative* (it also catches uses through a transparent copy), so the
    /// superset property holds.
    pub fn is_barrier_for(&self, op: &TirOp, val: ValueId) -> bool {
        // (a) A direct (or aliased) use of `val` reads/escapes it.
        let root = self.aliases.root(val);
        if op
            .operands
            .iter()
            .any(|&o| o == val || self.aliases.root(o) == root)
        {
            return true;
        }
        // (b) An opcode that can touch arbitrary heap memory is a barrier even
        //     without naming `val` (it could reach `val` through global state).
        opcode_is_heap_barrier(op.opcode)
    }

    /// Replaces `dead_store_elim::may_observe_slot`. True if `op` may observe the
    /// slot value of object `root` (read it, escape it, or trigger a side effect
    /// that could). `root` is an alias root.
    ///
    /// CONSERVATIVE SUPERSET of the old predicate (in fact byte-identical to it —
    /// see `tests::dse_observe_is_conservative_superset_of_old_may_observe`, which
    /// asserts equality on the aliasing arm). The op must alias `root` to be an
    /// observer at all; given that, the per-opcode classification reproduces the
    /// former allow-list exactly. The `LoadPurity` refinement is intentionally
    /// NOT applied here: every aliasing `LoadAttr` is treated as a slot observer
    /// regardless of whether it is a proven-pure typed-slot read, because a load
    /// of the *same* slot still observes a pending store's value. Purity is only
    /// consulted by callers that need to reorder the load itself.
    pub fn may_observe_slot(&self, op: &TirOp, root: ValueId) -> bool {
        if !self.aliases.operand_aliases_root(op, root) {
            return false;
        }
        aliasing_op_may_observe_slot(op, root, &self.aliases)
    }

    /// The memory region a memory-touching op reads or writes, used for
    /// may-alias disambiguation (see [`MemRegion`]).
    pub fn region_of(&self, op: &TirOp) -> MemRegion {
        match opcode_alias_memory_region_table(op.opcode) {
            AliasMemoryRegionClass::TypedSlotAttr => self.typed_slot_region(op),
            AliasMemoryRegionClass::CopyRefinement => Self::copy_region(op),
            AliasMemoryRegionClass::ContainerElement => MemRegion::ContainerElement,
            AliasMemoryRegionClass::ModuleDict => MemRegion::ModuleDict,
            AliasMemoryRegionClass::ScalarRegister => MemRegion::ScalarRegister,
            AliasMemoryRegionClass::GenericHeap => MemRegion::GenericHeap,
        }
    }

    fn typed_slot_region(&self, op: &TirOp) -> MemRegion {
        if let Some((target, offset)) = typed_slot_obj_offset(op) {
            let root = self.aliases.root(target);
            if self.is_stack_object(root) {
                return MemRegion::StackObject { root };
            }
            if let Some(class) = typed_slot_class(op) {
                return MemRegion::TypedField { class, offset };
            }
        }
        MemRegion::GenericHeap
    }

    fn copy_region(op: &TirOp) -> MemRegion {
        if copy_kind_is_explicit_no_heap_move(copy_original_kind(op))
            || copy_kind_is_memory_inert(op)
        {
            MemRegion::ScalarRegister
        } else {
            MemRegion::GenericHeap
        }
    }

    /// Load-purity gate (the Python-dunder soundness gate). [`LoadPurity::ProvenPure`]
    /// only for a typed-slot `LoadAttr` against a proven concrete class.
    #[inline]
    pub fn load_purity(&self, op: &TirOp) -> LoadPurity {
        classify_load(op)
    }

    /// True if `root` is a non-escaping stack object (rewritten or eligible to be
    /// rewritten to a stack allocation). A value is stack-resident iff it is a
    /// tracked allocation root that does not escape the function — i.e. its state
    /// is `NoEscape` or `ArgEscape` (borrowed by a call but not captured), mirroring
    /// `escape_analysis::apply`'s promotion set.
    fn is_stack_object(&self, root: ValueId) -> bool {
        matches!(
            self.escape.get(&root),
            Some(EscapeState::NoEscape) | Some(EscapeState::ArgEscape)
        )
    }

    /// True if `op` is a **transparent-alias producer**: a no-op `TypeGuard` or a
    /// pure-move `Copy` whose result names the *same* heap object as its operand
    /// (object identity flows through it unchanged). This is exactly the op set
    /// [`record_transparent_aliases`] unions into one root, so callers that have
    /// already routed values through [`root`](Self::root) can recognize such an op
    /// as object-identity plumbing rather than a fresh use.
    ///
    /// The opaque `_original_kind` passthrough carriers (container constructors,
    /// unmapped SimpleIR ops) are NOT transparent — their result is a distinct
    /// value — and return `false`. This is the single source of truth for "is
    /// this Copy/TypeGuard a pure identity move?"; SROA consumes it so it never
    /// re-implements the contract.
    #[inline]
    pub fn is_transparent_alias_op(&self, op: &TirOp) -> bool {
        transparent_alias_root(op, &self.aliases).is_some()
    }
}

// ===========================================================================
// S1 Analysis registration
// ===========================================================================

/// Alias analysis marker. Cached by the [`AnalysisManager`](crate::tir::analysis::AnalysisManager).
///
/// CFG-sensitive (escape/points-to propagation follows control flow and
/// terminator uses) and ops-sensitive (the alias union-find, escape uses, and
/// region classification all derive from the op stream). Both flags `true` ⇒ any
/// CFG or op rewrite drops the cached result, recomputed on next demand.
pub struct AliasAnalysis;

impl Analysis for AliasAnalysis {
    type Result = AliasAnalysisResult;
    const ID: AnalysisId = AnalysisId::AliasAnalysis;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        AliasAnalysisResult::compute(func)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, TirOp};
    use crate::tir::types::TirType;

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn op_kind(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>, kind: &str) -> TirOp {
        let mut o = op(opcode, operands, results);
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        o
    }

    /// Every `OpCode` variant — kept exhaustive by `assert_opcode_listed`, so a
    /// newly-added opcode forces a deliberate barrier classification.
    fn all_opcodes() -> Vec<OpCode> {
        use OpCode::*;
        vec![
            Add,
            CheckedAdd,
            CheckedMul,
            Sub,
            Mul,
            InplaceAdd,
            InplaceSub,
            InplaceMul,
            Div,
            FloorDiv,
            Mod,
            Pow,
            Neg,
            Pos,
            Eq,
            Ne,
            Lt,
            Le,
            Gt,
            Ge,
            Is,
            IsNot,
            In,
            NotIn,
            BitAnd,
            BitOr,
            BitXor,
            BitNot,
            Shl,
            Shr,
            And,
            Or,
            Not,
            Bool,
            Alloc,
            StackAlloc,
            ObjectNewBound,
            ObjectNewBoundStack,
            Free,
            LoadAttr,
            StoreAttr,
            DelAttr,
            Index,
            StoreIndex,
            DelIndex,
            DeleteVar,
            Call,
            CallMethod,
            CallMethodIc,
            CallSuperMethodIc,
            CallBuiltin,
            OrdAt,
            BoxVal,
            UnboxVal,
            TypeGuard,
            IncRef,
            DecRef,
            DelBoundary,
            BuildList,
            BuildDict,
            BuildTuple,
            BuildSet,
            BuildSlice,
            GetIter,
            IterNext,
            IterNextUnboxed,
            ForIter,
            AllocTask,
            StateSwitch,
            StateTransition,
            StateYield,
            ChanSendYield,
            ChanRecvYield,
            ClosureLoad,
            ClosureStore,
            Yield,
            YieldFrom,
            Raise,
            CheckException,
            ExceptionPending,
            FunctionDefaultsVersion,
            TryStart,
            TryEnd,
            StateBlockStart,
            StateBlockEnd,
            ConstInt,
            ConstBigInt,
            ConstFloat,
            ConstStr,
            ConstBool,
            ConstNone,
            ConstBytes,
            Copy,
            Import,
            ImportFrom,
            ModuleCacheGet,
            ModuleCacheSet,
            ModuleCacheDel,
            ModuleGetAttr,
            ModuleImportFrom,
            ModuleGetGlobal,
            ModuleGetName,
            ModuleSetAttr,
            ModuleDelGlobal,
            ModuleDelGlobalIfPresent,
            WarnStderr,
            ScfIf,
            ScfFor,
            ScfWhile,
            ScfYield,
            Deopt,
        ]
    }

    fn assert_opcode_listed(opcode: OpCode) {
        use OpCode::*;
        match opcode {
            Add
            | CheckedAdd
            | CheckedMul
            | Sub
            | Mul
            | InplaceAdd
            | InplaceSub
            | InplaceMul
            | Div
            | FloorDiv
            | Mod
            | Pow
            | Neg
            | Pos
            | Eq
            | Ne
            | Lt
            | Le
            | Gt
            | Ge
            | Is
            | IsNot
            | In
            | NotIn
            | BitAnd
            | BitOr
            | BitXor
            | BitNot
            | Shl
            | Shr
            | And
            | Or
            | Not
            | Bool
            | Alloc
            | StackAlloc
            | ObjectNewBound
            | ObjectNewBoundStack
            | Free
            | LoadAttr
            | StoreAttr
            | DelAttr
            | Index
            | StoreIndex
            | DelIndex
            | DeleteVar
            | Call
            | CallMethod
            | CallMethodIc
            | CallSuperMethodIc
            | CallBuiltin
            | OrdAt
            | BoxVal
            | UnboxVal
            | TypeGuard
            | IncRef
            | DecRef
            | DelBoundary
            | BuildList
            | BuildDict
            | BuildTuple
            | BuildSet
            | BuildSlice
            | GetIter
            | IterNext
            | IterNextUnboxed
            | ForIter
            | AllocTask
            | StateSwitch
            | StateTransition
            | StateYield
            | ChanSendYield
            | ChanRecvYield
            | ClosureLoad
            | ClosureStore
            | Yield
            | YieldFrom
            | Raise
            | CheckException
            | ExceptionPending
            | FunctionDefaultsVersion
            | TryStart
            | TryEnd
            | StateBlockStart
            | StateBlockEnd
            | ConstInt
            | ConstBigInt
            | ConstFloat
            | ConstStr
            | ConstBool
            | ConstNone
            | ConstBytes
            | Copy
            | Import
            | ImportFrom
            | ModuleCacheGet
            | ModuleCacheSet
            | ModuleCacheDel
            | ModuleGetAttr
            | ModuleImportFrom
            | ModuleGetGlobal
            | ModuleGetName
            | ModuleSetAttr
            | ModuleDelGlobal
            | ModuleDelGlobalIfPresent
            | WarnStderr
            | ScfIf
            | ScfFor
            | ScfWhile
            | ScfYield
            | Deopt => {}
        }
    }

    // ── The OLD four barrier lists, reproduced verbatim as oracles ─────────

    const OLD_REFCOUNT_BARRIER_OPCODES: &[OpCode] = &[
        OpCode::Call,
        OpCode::CallMethod,
        OpCode::CallMethodIc,
        OpCode::CallSuperMethodIc,
        OpCode::CallBuiltin,
        OpCode::StoreAttr,
        OpCode::StoreIndex,
        OpCode::StateSwitch,
        OpCode::StateTransition,
        OpCode::StateYield,
        OpCode::ClosureLoad,
        OpCode::ClosureStore,
        OpCode::ChanSendYield,
        OpCode::ChanRecvYield,
    ];

    const OLD_REUSE_OPCODE_BARRIERS: &[OpCode] = &[
        OpCode::Call,
        OpCode::CallMethod,
        OpCode::CallMethodIc,
        OpCode::CallSuperMethodIc,
        OpCode::CallBuiltin,
        OpCode::StoreAttr,
        OpCode::StoreIndex,
        OpCode::Raise,
        OpCode::Yield,
        OpCode::YieldFrom,
        OpCode::StateSwitch,
        OpCode::StateTransition,
        OpCode::StateYield,
        OpCode::ChanSendYield,
        OpCode::ChanRecvYield,
        OpCode::ClosureStore,
        OpCode::Free,
    ];

    const OLD_DSE_DIRECT_OBSERVERS: &[OpCode] = &[
        OpCode::LoadAttr,
        OpCode::Index,
        OpCode::StoreIndex,
        OpCode::Call,
        OpCode::CallMethod,
        OpCode::CallMethodIc,
        OpCode::CallSuperMethodIc,
        OpCode::CallBuiltin,
        OpCode::Raise,
        OpCode::Yield,
        OpCode::YieldFrom,
        OpCode::BuildList,
        OpCode::BuildDict,
        OpCode::BuildSet,
        OpCode::BuildTuple,
        OpCode::BuildSlice,
        OpCode::AllocTask,
    ];

    const OLD_DSE_TRANSPARENT_ALIAS_NON_OBSERVERS: &[OpCode] = &[OpCode::Copy, OpCode::TypeGuard];

    const OLD_DSE_NEVER_OBSERVERS: &[OpCode] =
        &[OpCode::IncRef, OpCode::DecRef, OpCode::CheckException];

    /// `refcount_elim::is_barrier` as it stood before S5 phase 1.
    fn old_refcount_is_barrier(opcode: OpCode) -> bool {
        OLD_REFCOUNT_BARRIER_OPCODES.contains(&opcode)
    }

    /// `reuse_analysis::is_aliasing_op`'s opcode portion (excluding the
    /// operand-uses-val branch, tested separately).
    fn old_reuse_opcode_barrier(opcode: OpCode) -> bool {
        OLD_REUSE_OPCODE_BARRIERS.contains(&opcode)
    }

    /// `dead_store_elim::may_observe_slot` as it stood before S5 phase 1.
    /// Reproduced against the *promoted* helpers (semantically identical).
    fn old_dse_may_observe(op: &TirOp, root: ValueId, aliases: &AliasUnionFind) -> bool {
        if !aliases.operand_aliases_root(op, root) {
            return false;
        }
        if OLD_DSE_DIRECT_OBSERVERS.contains(&op.opcode) {
            return true;
        }
        if op.opcode == OpCode::StoreAttr {
            return match typed_slot_store(op) {
                Some((target, _)) => aliases.root(target) != root,
                None => true,
            };
        }
        if OLD_DSE_TRANSPARENT_ALIAS_NON_OBSERVERS.contains(&op.opcode)
            && transparent_alias_root(op, aliases).is_some()
        {
            return false;
        }
        if OLD_DSE_NEVER_OBSERVERS.contains(&op.opcode) {
            return false;
        }
        true
    }

    // ── Superset proofs ────────────────────────────────────────────────────

    #[test]
    fn opcode_enum_is_exhaustively_listed() {
        for op in all_opcodes() {
            assert_opcode_listed(op);
        }
    }

    /// `is_rc_barrier ⊇ refcount_elim::is_barrier` for EVERY opcode.
    #[test]
    fn rc_barrier_is_conservative_superset_of_old_refcount_list() {
        for opcode in all_opcodes() {
            if old_refcount_is_barrier(opcode) {
                assert!(
                    opcode_is_rc_barrier(opcode),
                    "{opcode:?}: old refcount is_barrier=true but new is_rc_barrier=false — \
                     UNSOUND (would re-pair across a real barrier ⇒ refcount imbalance)"
                );
            }
        }
    }

    #[test]
    fn exception_control_transfer_ops_are_rc_barriers() {
        for opcode in [OpCode::Raise, OpCode::CheckException, OpCode::TryStart] {
            assert!(
                opcode_is_rc_barrier(opcode),
                "{opcode:?} must stop IncRef/DecRef pairing across exceptional control transfer"
            );
        }
        assert!(
            !opcode_is_rc_barrier(OpCode::TryEnd),
            "TryEnd is structural region-close metadata, not a transfer into the handler"
        );
    }

    /// `is_barrier_for ⊇ reuse_analysis::is_aliasing_op` for every (opcode, val):
    /// both the opcode branch and the operand-uses-val branch.
    #[test]
    fn reuse_barrier_is_conservative_superset_of_old_aliasing_op() {
        let v = ValueId(7);
        let other = ValueId(99);
        let res = AliasAnalysisResult {
            aliases: AliasUnionFind::default(),
            escape: HashMap::new(),
            alloc_roots: HashSet::new(),
        };
        for opcode in all_opcodes() {
            // Opcode branch: an op NOT using `v`.
            let no_use = op(opcode, vec![other], vec![]);
            if old_reuse_opcode_barrier(opcode) {
                assert!(
                    res.is_barrier_for(&no_use, v),
                    "{opcode:?}: old is_aliasing_op opcode-barrier=true but new is_barrier_for=false"
                );
            }
            // Operand-uses-val branch: ANY op that names `v` is a barrier in the
            // old list. New must agree.
            let uses_v = op(opcode, vec![v], vec![]);
            assert!(
                res.is_barrier_for(&uses_v, v),
                "{opcode:?}: old is_aliasing_op returns true when op uses val; new must too"
            );
        }
    }

    /// `may_observe_slot ⊇ dead_store_elim::may_observe_slot` for every opcode,
    /// in both the aliasing and non-aliasing cases.
    #[test]
    fn dse_observe_is_conservative_superset_of_old_may_observe() {
        let root = ValueId(3);
        let res = AliasAnalysisResult {
            aliases: AliasUnionFind::default(),
            escape: HashMap::new(),
            alloc_roots: HashSet::new(),
        };
        for opcode in all_opcodes() {
            // Aliasing case: op uses `root`.
            let aliasing = op(opcode, vec![root], vec![ValueId(50)]);
            let old = old_dse_may_observe(&aliasing, root, &res.aliases);
            let new = res.may_observe_slot(&aliasing, root);
            assert!(
                !old || new,
                "{opcode:?}: old may_observe_slot=true but new=false (aliasing case) — \
                 UNSOUND (would drop an observable store)"
            );
            // Non-aliasing case: op does not name `root` ⇒ both must be false
            // (a store-elim observer must alias the object).
            let non_aliasing = op(opcode, vec![ValueId(60)], vec![ValueId(61)]);
            assert!(
                !res.may_observe_slot(&non_aliasing, root),
                "{opcode:?}: non-aliasing op must not observe slot"
            );
        }
    }

    /// Byte-identical equivalence (not just superset) on the typed-slot store
    /// overwrite semantics, so dead_store_elim keeps eliminating exactly what it
    /// used to.
    #[test]
    fn dse_typed_slot_store_overwrite_matches_old() {
        let root = ValueId(3);
        let val = ValueId(4);
        let res = AliasAnalysisResult {
            aliases: AliasUnionFind::default(),
            escape: HashMap::new(),
            alloc_roots: HashSet::new(),
        };
        // store to the SAME root+offset is an overwrite, not an observer.
        let mut store = op(OpCode::StoreAttr, vec![root, val], vec![]);
        store.attrs.insert("value".into(), AttrValue::Int(0));
        store
            .attrs
            .insert("_original_kind".into(), AttrValue::Str("store".into()));
        assert!(
            !res.may_observe_slot(&store, root),
            "same-root store is an overwrite"
        );
        // store that USES root as the stored value (target != root) observes it.
        let other = ValueId(8);
        let mut escape_store = op(OpCode::StoreAttr, vec![other, root], vec![]);
        escape_store
            .attrs
            .insert("value".into(), AttrValue::Int(16));
        escape_store
            .attrs
            .insert("_original_kind".into(), AttrValue::Str("store".into()));
        assert!(
            res.may_observe_slot(&escape_store, root),
            "storing root into another object observes/escapes it"
        );
    }

    // ── LoadPurity dunder gate ─────────────────────────────────────────────

    #[test]
    fn typed_slot_load_is_proven_pure() {
        for kind in ["guarded_field_get", "load"] {
            let o = op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], kind);
            assert_eq!(
                classify_load(&o),
                LoadPurity::ProvenPure,
                "{kind} is a typed slot"
            );
        }
    }

    #[test]
    fn opaque_attr_load_may_dispatch() {
        for kind in [
            "get_attr",
            "get_attr_name",
            "get_attr_generic_ptr",
            "get_attr_generic_obj",
        ] {
            let o = op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], kind);
            assert_eq!(
                classify_load(&o),
                LoadPurity::MayDispatch,
                "{kind} can dispatch __getattr__/__getattribute__"
            );
        }
        // A LoadAttr with no kind annotation is conservatively opaque.
        let bare = op(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)]);
        assert_eq!(classify_load(&bare), LoadPurity::MayDispatch);
    }

    #[test]
    fn index_always_may_dispatch() {
        // Index can dispatch __getitem__ regardless of any attr.
        let o = op(
            OpCode::Index,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
        );
        assert_eq!(classify_load(&o), LoadPurity::MayDispatch);
    }

    // ── MemRegion may-alias ────────────────────────────────────────────────

    #[test]
    fn scalar_register_aliases_nothing() {
        let scalar = MemRegion::ScalarRegister;
        for other in [
            MemRegion::GenericHeap,
            MemRegion::ContainerElement,
            MemRegion::ModuleDict,
            MemRegion::TypedField {
                class: "Point".into(),
                offset: 0,
            },
            MemRegion::StackObject { root: ValueId(1) },
            MemRegion::ScalarRegister,
        ] {
            assert!(!scalar.may_alias(&other));
            assert!(!other.may_alias(&scalar));
        }
    }

    #[test]
    fn distinct_typed_fields_are_disjoint() {
        let f0 = MemRegion::TypedField {
            class: "Point".into(),
            offset: 0,
        };
        let f8 = MemRegion::TypedField {
            class: "Point".into(),
            offset: 8,
        };
        let g0 = MemRegion::TypedField {
            class: "Line".into(),
            offset: 0,
        };
        assert!(!f0.may_alias(&f8), "different offset ⇒ disjoint");
        assert!(!f0.may_alias(&g0), "different class ⇒ disjoint");
        assert!(f0.may_alias(&f0.clone()), "same class+offset ⇒ may alias");
    }

    #[test]
    fn distinct_stack_objects_are_disjoint() {
        let a = MemRegion::StackObject { root: ValueId(1) };
        let b = MemRegion::StackObject { root: ValueId(2) };
        assert!(!a.may_alias(&b));
        assert!(a.may_alias(&a.clone()));
        // A stack object never aliases generic heap (it is proven non-escaping).
        assert!(!a.may_alias(&MemRegion::GenericHeap));
    }

    #[test]
    fn generic_heap_aliases_opaque_regions() {
        let g = MemRegion::GenericHeap;
        assert!(g.may_alias(&MemRegion::ContainerElement));
        assert!(g.may_alias(&MemRegion::ModuleDict));
        assert!(g.may_alias(&MemRegion::GenericHeap));
        assert!(g.may_alias(&MemRegion::TypedField {
            class: "P".into(),
            offset: 0
        }));
    }

    // ── AliasUnionFind ─────────────────────────────────────────────────────

    #[test]
    fn transparent_copy_chain_resolves_to_root() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let obj = ValueId(0);
        let a = func.fresh_value();
        let b = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // a = Copy obj ; b = Copy a   (both pure moves)
        entry.ops.push(op(OpCode::Copy, vec![obj], vec![a]));
        entry.ops.push(op(OpCode::Copy, vec![a], vec![b]));
        entry.terminator = Terminator::Return { values: vec![] };

        let res = AliasAnalysisResult::compute(&func);
        assert_eq!(res.root(b), obj, "b aliases obj through the copy chain");
        assert_eq!(res.root(a), obj);
    }

    #[test]
    fn container_builder_passthrough_copy_is_not_an_alias() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let obj = ValueId(0);
        let lst = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // lst = Copy[list_new] obj  — result is a NEW container, not an alias.
        entry
            .ops
            .push(op_kind(OpCode::Copy, vec![obj], vec![lst], "list_new"));
        entry.terminator = Terminator::Return { values: vec![] };

        let res = AliasAnalysisResult::compute(&func);
        assert_ne!(
            res.root(lst),
            obj,
            "container builder result is not an alias of its element"
        );
    }

    #[test]
    fn owned_binding_alias_copy_is_not_a_transparent_root() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let obj = ValueId(0);
        let alias = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(op_kind(
            OpCode::Copy,
            vec![obj],
            vec![alias],
            "binding_alias",
        ));
        entry.terminator = Terminator::Return { values: vec![] };

        let res = AliasAnalysisResult::compute(&func);
        assert_eq!(
            res.root(alias),
            alias,
            "binding_alias carries source bits but owns a distinct droppable root"
        );
        assert_ne!(res.root(alias), res.root(obj));
    }

    // ── The lowering-truth Copy-class contract (over-release keystone) ──────

    /// Every `_original_kind` classifies into exactly one [`CopyLowering`] bucket,
    /// and the derived predicates (alias / inert / passthrough-reachable)
    /// are a partition consistent with the classifier. This is the single-source-
    /// of-truth guard: the alias view and the no-incref-passthrough set cannot
    /// drift because both read `classify_copy_kind`.
    #[test]
    fn copy_lowering_classes_are_total_and_disjoint() {
        // A representative sample spanning the buckets, plus the bare-Copy
        // (None) case and the bug-repro fresh-value kinds the review flagged.
        let alias = [
            None,
            Some("copy"),
            Some("copy_var"),
            Some("store_var"),
            Some("load_var"),
            Some("identity_alias"),
            // validate-and-pass-through guards: result == operand 0, no incref.
            Some("guard_tag"),
            Some("guard_type"),
        ];
        let inert = [
            Some("line"),
            Some("trace_enter_slot"),
            Some("trace_exit"),
            Some("missing"),
            Some("nop"),
            Some("guard_layout"),
            Some("guard_dict_shape"),
            Some("guard_int"),
            Some("guard_float"),
            Some("guard_str"),
            Some("guard_bool"),
            Some("guard_none"),
        ];
        // The fresh-value kinds the drop pass releases independently. Each MUST
        // classify FreshValue (incl. the review's double-free root `slice` and the
        // generator-iterator `iter`) AND must NOT be allowed to reach the benign
        // no-incref passthrough.
        let fresh = [
            Some("slice"),
            Some("slice_new"),
            Some("string_format"),
            Some("repr_from_obj"),
            Some("int_from_obj"),
            Some("float_from_obj"),
            Some("contains"),
            Some("classmethod_new"),
            Some("code_new"),
            Some("dataclass_new"),
            Some("dataclass_new_values"),
            Some("str_from_obj"),
            Some("iter"),
            Some("aiter"),
            Some("enumerate"),
            Some("func_new"),
            Some("func_new_closure"),
            Some("dict_keys"),
            Some("dict_values"),
            Some("dict_items"),
            Some("dict_from_obj"),
            Some("object_new"),
            Some("property_new"),
            Some("complex_from_obj"),
            Some("list_new"),
            Some("list_pop"),
            Some("dict_new"),
            Some("tuple_new"),
            Some("string_join"),
            Some("staticmethod_new"),
            Some("vec_sum_i64"),
        ];
        let owned_alias = [Some("binding_alias")];
        // FAIL-CLOSED: an unrecognized future kind classifies as TransparentAlias
        // (leak-safe), NOT FreshValue — so the drop pass never double-frees it.
        let unknown_fail_closed = [Some("some_brand_new_kind_v2"), Some("promise_new")];

        for k in alias {
            assert_eq!(
                classify_copy_kind(k),
                CopyLowering::TransparentAlias,
                "{k:?} must be a transparent alias"
            );
            assert!(
                copy_kind_reaches_no_incref_passthrough(k),
                "{k:?} reaches passthrough"
            );
        }
        for k in inert {
            assert_eq!(
                classify_copy_kind(k),
                CopyLowering::InertMarker,
                "{k:?} is inert"
            );
            assert!(
                copy_kind_reaches_no_incref_passthrough(k),
                "{k:?} reaches passthrough"
            );
        }
        for k in fresh {
            assert_eq!(
                classify_copy_kind(k),
                CopyLowering::FreshValue,
                "{k:?} mints a fresh owned value"
            );
            assert!(
                !copy_kind_reaches_no_incref_passthrough(k),
                "{k:?} must NOT reach the benign passthrough — a FreshValue that fell \
                 through would alias operand 0 and be double-freed by drop insertion"
            );
        }
        for k in owned_alias {
            assert_eq!(
                classify_copy_kind(k),
                CopyLowering::OwnedAlias,
                "{k:?} mints an owned alias reference"
            );
            assert!(
                !copy_kind_reaches_no_incref_passthrough(k),
                "{k:?} must lower as inc_ref + alias, not no-incref passthrough"
            );
        }
        for k in unknown_fail_closed {
            assert_eq!(
                classify_copy_kind(k),
                CopyLowering::TransparentAlias,
                "{k:?} must FAIL CLOSED to TransparentAlias (leak-safe, never UAF)"
            );
            assert!(
                copy_kind_reaches_no_incref_passthrough(k),
                "{k:?} fail-closes to the leak-safe passthrough/alias path"
            );
        }
    }

    /// The exact double-free vector from the adversarial review: a `Copy` carrying
    /// `_original_kind = "slice"` (the `s[-5:]` subscript) must NOT be unioned into
    /// its source operand's alias root. If it were treated as a transparent alias,
    /// the drop pass would drop the slice and its source as one group — but they
    /// are two independent owned references on a correct (FreshValue) backend.
    #[test]
    fn slice_subscript_copy_is_a_fresh_value_not_an_alias() {
        let mut func = TirFunction::new("f".into(), vec![TirType::Str], TirType::Str);
        let src = ValueId(0);
        let start = func.fresh_value();
        let stop = func.fresh_value();
        let sliced = func.fresh_value();
        func.value_types.insert(sliced, TirType::Str);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(op_kind(
            OpCode::Copy,
            vec![src, start, stop],
            vec![sliced],
            "slice",
        ));
        entry.terminator = Terminator::Return {
            values: vec![sliced],
        };

        let res = AliasAnalysisResult::compute(&func);
        assert_ne!(
            res.root(sliced),
            res.root(src),
            "slice result must be an independent alias root, not an alias of its source"
        );
    }

    // ── Escape map plumbing + S1 caching ───────────────────────────────────

    #[test]
    fn escape_map_matches_escape_analysis_and_caches() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst = func.fresh_value();
        let load = func.fresh_value();
        let none = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(op(OpCode::ObjectNewBound, vec![class_ref], vec![inst]));
        entry.ops.push(op(OpCode::LoadAttr, vec![inst], vec![load]));
        entry.ops.push(op(OpCode::ConstNone, vec![], vec![none]));
        entry.terminator = Terminator::Return { values: vec![none] };

        // The alias analysis's escape map equals escape_analysis::analyze.
        let res = AliasAnalysisResult::compute(&func);
        let direct = super::super::escape_analysis::analyze(&func);
        assert_eq!(res.escape, direct);
        assert_eq!(res.escape_state(inst), EscapeState::NoEscape);

        // S1 caching: first get computes, second is a cache hit.
        let mut am = AnalysisManager::new();
        assert!(!am.is_cached(AnalysisId::AliasAnalysis));
        let cached = am.get::<AliasAnalysis>(&func);
        assert_eq!(cached.escape_state(inst), EscapeState::NoEscape);
        assert!(am.is_cached(AnalysisId::AliasAnalysis));
    }

    #[test]
    fn region_of_classifies_pure_compute_as_scalar() {
        let add = op(OpCode::Add, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]);
        let res = AliasAnalysisResult {
            aliases: AliasUnionFind::default(),
            escape: HashMap::new(),
            alloc_roots: HashSet::new(),
        };
        assert_eq!(res.region_of(&add), MemRegion::ScalarRegister);
        let idx = op(
            OpCode::Index,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
        );
        assert_eq!(res.region_of(&idx), MemRegion::ContainerElement);
        let mcg = op(
            OpCode::ModuleGetGlobal,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
        );
        assert_eq!(res.region_of(&mcg), MemRegion::ModuleDict);
    }

    // ── region_of: class-aware TypedField regions (S5-1.5) ─────────────────

    /// Set the offset + class attrs on a typed-slot field op.
    fn with_field_attrs(mut o: TirOp, offset: i64, class: Option<&str>) -> TirOp {
        o.attrs.insert("value".into(), AttrValue::Int(offset));
        if let Some(c) = class {
            o.attrs.insert("_class".into(), AttrValue::Str(c.into()));
        }
        o
    }

    fn empty_res() -> AliasAnalysisResult {
        AliasAnalysisResult {
            aliases: AliasUnionFind::default(),
            escape: HashMap::new(),
            alloc_roots: HashSet::new(),
        }
    }

    #[test]
    fn plain_load_store_classify_as_typed_field_from_class_attr() {
        let res = empty_res();
        // `load obj.<8>` of class Point  (operands [obj], offset 8).
        let load = with_field_attrs(
            op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], "load"),
            8,
            Some("Point"),
        );
        assert_eq!(
            res.region_of(&load),
            MemRegion::TypedField {
                class: "Point".into(),
                offset: 8
            }
        );
        // `store obj.<16> = val` of class Line (operands [obj, val], offset 16).
        let store = with_field_attrs(
            op_kind(
                OpCode::StoreAttr,
                vec![ValueId(0), ValueId(2)],
                vec![],
                "store",
            ),
            16,
            Some("Line"),
        );
        assert_eq!(
            res.region_of(&store),
            MemRegion::TypedField {
                class: "Line".into(),
                offset: 16
            }
        );
        // `store_init` is also a typed-slot store.
        let init = with_field_attrs(
            op_kind(
                OpCode::StoreAttr,
                vec![ValueId(0), ValueId(2)],
                vec![],
                "store_init",
            ),
            0,
            Some("Point"),
        );
        assert_eq!(
            res.region_of(&init),
            MemRegion::TypedField {
                class: "Point".into(),
                offset: 0
            }
        );
    }

    /// A `Copy` is classified by whether it touches heap memory: a pure SSA
    /// move (no `_original_kind`) and the inert debug / source-location / guard
    /// markers are `ScalarRegister`; an opaque passthrough carrier stays the
    /// conservative `GenericHeap`. This is the keystone that stops a `line` /
    /// `trace_exit` marker `Copy` from spuriously clobbering the memory version
    /// between a constructor's field stores and the field loads (S5-2d).
    #[test]
    fn copy_region_pure_and_inert_markers_are_scalar() {
        let res = empty_res();

        // Pure SSA move (no `_original_kind`): identity plumbing, no heap.
        let pure_move = op(OpCode::Copy, vec![ValueId(0)], vec![ValueId(1)]);
        assert_eq!(res.region_of(&pure_move), MemRegion::ScalarRegister);

        // Known-local-alias kinds are pure moves too.
        for kind in [
            "copy",
            "copy_var",
            "store_var",
            "load_var",
            "identity_alias",
        ] {
            let c = op_kind(OpCode::Copy, vec![ValueId(0)], vec![ValueId(1)], kind);
            assert_eq!(
                res.region_of(&c),
                MemRegion::ScalarRegister,
                "alias-kind copy '{kind}' is heap-inert"
            );
        }

        // Inert debug / source-location / sentinel / guard markers: no heap.
        for kind in [
            "line",
            "trace_enter_slot",
            "trace_exit",
            "missing",
            "nop",
            "guard_layout",
            "guard_dict_shape",
            "guard_int",
            "guard_float",
            "guard_str",
            "guard_bool",
            "guard_none",
        ] {
            let c = op_kind(OpCode::Copy, vec![], vec![], kind);
            assert_eq!(
                res.region_of(&c),
                MemRegion::ScalarRegister,
                "inert marker copy '{kind}' must not clobber memory"
            );
        }

        // An opaque passthrough carrier (an unmapped SimpleIR op with no proven
        // memory-inert kind) keeps the conservative GenericHeap classification.
        let opaque = op_kind(
            OpCode::Copy,
            vec![ValueId(0)],
            vec![ValueId(1)],
            "list_append",
        );
        assert_eq!(res.region_of(&opaque), MemRegion::GenericHeap);

        let owned_alias = op_kind(
            OpCode::Copy,
            vec![ValueId(0)],
            vec![ValueId(1)],
            "binding_alias",
        );
        assert_eq!(res.region_of(&owned_alias), MemRegion::GenericHeap);
    }

    #[test]
    fn guarded_field_get_3operand_abi_classifies_as_typed_field() {
        // `guarded_field_get` ABI: operands [obj, class_bits, expected_version],
        // offset in `value`, class in `_class`. obj is operand[0].
        let res = empty_res();
        let get = with_field_attrs(
            op_kind(
                OpCode::LoadAttr,
                vec![ValueId(0), ValueId(1), ValueId(2)], // obj, class_bits, version
                vec![ValueId(3)],
                "guarded_field_get",
            ),
            24,
            Some("Account"),
        );
        assert_eq!(
            res.region_of(&get),
            MemRegion::TypedField {
                class: "Account".into(),
                offset: 24
            }
        );
    }

    #[test]
    fn guarded_field_set_4operand_abi_classifies_as_typed_field() {
        // `guarded_field_set` ABI: operands [obj, class_bits, expected_version,
        // val], offset in `value`, class in `_class`. obj is operand[0].
        let res = empty_res();
        for kind in ["guarded_field_set", "guarded_field_init"] {
            let set = with_field_attrs(
                op_kind(
                    OpCode::StoreAttr,
                    vec![ValueId(0), ValueId(1), ValueId(2), ValueId(3)],
                    vec![],
                    kind,
                ),
                32,
                Some("Account"),
            );
            assert_eq!(
                res.region_of(&set),
                MemRegion::TypedField {
                    class: "Account".into(),
                    offset: 32
                },
                "{kind} on operand[0]=obj is a TypedField"
            );
        }

        let rejected = with_field_attrs(
            op_kind(
                OpCode::StoreAttr,
                vec![ValueId(0), ValueId(1), ValueId(2), ValueId(3)],
                vec![],
                "guarded_field_set_init",
            ),
            32,
            Some("Account"),
        );
        assert_eq!(
            res.region_of(&rejected),
            MemRegion::GenericHeap,
            "removed guarded-field init spelling must not remain a typed-slot alias"
        );
    }

    #[test]
    fn typed_slot_without_class_attr_fails_closed_to_generic_heap() {
        // FAIL-CLOSED: a typed-slot kind with offset but NO `_class` proof stays
        // GenericHeap (a pre-S5-1.5 cached artifact, or a dropped attr).
        let res = empty_res();
        let load = with_field_attrs(
            op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], "load"),
            8,
            None,
        );
        assert_eq!(res.region_of(&load), MemRegion::GenericHeap);
        let store = with_field_attrs(
            op_kind(
                OpCode::StoreAttr,
                vec![ValueId(0), ValueId(2)],
                vec![],
                "store",
            ),
            8,
            None,
        );
        assert_eq!(res.region_of(&store), MemRegion::GenericHeap);
    }

    #[test]
    fn opaque_attr_spelling_is_generic_heap_even_with_class_attr() {
        // A generic `get_attr` / `set_attr` spelling is NOT a typed-slot op (it
        // may dispatch a dunder), so it is GenericHeap regardless of any stray
        // attrs. (The frontend never stamps `_class` on these, but assert the
        // classification is robust to it.)
        let res = empty_res();
        let ga = with_field_attrs(
            op_kind(
                OpCode::LoadAttr,
                vec![ValueId(0)],
                vec![ValueId(1)],
                "get_attr",
            ),
            8,
            Some("Point"),
        );
        assert_eq!(res.region_of(&ga), MemRegion::GenericHeap);
        let sa = with_field_attrs(
            op_kind(
                OpCode::StoreAttr,
                vec![ValueId(0), ValueId(2)],
                vec![],
                "set_attr_generic_ptr",
            ),
            8,
            Some("Point"),
        );
        assert_eq!(res.region_of(&sa), MemRegion::GenericHeap);
    }

    #[test]
    fn non_escaping_object_field_is_stack_object_even_without_class() {
        // A field op on a proven-non-escaping object root gets the per-object
        // `StackObject` region — derived from the allocation root ALONE, so it
        // stays precise even when the op carries no `_class` attr.
        let root = ValueId(0);
        let mut escape = HashMap::new();
        escape.insert(root, EscapeState::NoEscape);
        let res = AliasAnalysisResult {
            aliases: AliasUnionFind::default(),
            escape,
            alloc_roots: [root].into_iter().collect(),
        };
        // No `_class` attr, but the root is non-escaping ⇒ StackObject.
        let load = with_field_attrs(
            op_kind(OpCode::LoadAttr, vec![root], vec![ValueId(1)], "load"),
            8,
            None,
        );
        assert_eq!(res.region_of(&load), MemRegion::StackObject { root });
        // With a class attr too, StackObject still wins (more precise than the
        // class-shared TypedField).
        let load_c = with_field_attrs(
            op_kind(OpCode::LoadAttr, vec![root], vec![ValueId(1)], "load"),
            8,
            Some("Point"),
        );
        assert_eq!(res.region_of(&load_c), MemRegion::StackObject { root });
    }

    // ── may_alias matrix: TypedField vs every region ───────────────────────

    #[test]
    fn typed_field_may_alias_matrix() {
        let pt0 = MemRegion::TypedField {
            class: "Point".into(),
            offset: 0,
        };
        let pt8 = MemRegion::TypedField {
            class: "Point".into(),
            offset: 8,
        };
        let ln0 = MemRegion::TypedField {
            class: "Line".into(),
            offset: 0,
        };
        // Same class+offset ⇒ may-alias (object identity untracked, oblig. (b)).
        assert!(pt0.may_alias(&pt0.clone()));
        // Different offset ⇒ disjoint.
        assert!(!pt0.may_alias(&pt8));
        // Different class ⇒ disjoint (oblig. (a): distinct classes never share).
        assert!(!pt0.may_alias(&ln0));
        // TypedField vs ContainerElement ⇒ disjoint (oblig. (a)).
        assert!(!pt0.may_alias(&MemRegion::ContainerElement));
        assert!(!MemRegion::ContainerElement.may_alias(&pt0));
        // TypedField vs ModuleDict ⇒ disjoint (oblig. (a)).
        assert!(!pt0.may_alias(&MemRegion::ModuleDict));
        assert!(!MemRegion::ModuleDict.may_alias(&pt0));
        // TypedField vs GenericHeap ⇒ may-alias (oblig. (c): opaque clobbers).
        assert!(pt0.may_alias(&MemRegion::GenericHeap));
        assert!(MemRegion::GenericHeap.may_alias(&pt0));
        // TypedField vs ScalarRegister ⇒ disjoint (no heap footprint).
        assert!(!pt0.may_alias(&MemRegion::ScalarRegister));
        // TypedField vs a distinct StackObject ⇒ disjoint (different object).
        assert!(!pt0.may_alias(&MemRegion::StackObject { root: ValueId(9) }));
    }

    /// Borrow provenance (design 20 interior-borrow keepalive). A `LoadAttr` /
    /// `Index` result records its source object's alias root; a use of the result
    /// keeps the source alive. `OrdAt` (an i64-producing fused read) does NOT.
    #[test]
    fn borrow_provenance_records_loadattr_and_index_sources() {
        use crate::tir::blocks::Terminator;
        use crate::tir::function::TirFunction;
        use crate::tir::ops::{Dialect, TirOp};
        use crate::tir::types::TirType;

        fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
            TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands,
                results,
                attrs: AttrDict::new(),
                source_span: None,
            }
        }

        let mut func = TirFunction::new("bp".into(), vec![], TirType::DynBox);
        let obj = func.fresh_value();
        let h = func.fresh_value(); // LoadAttr(obj)
        let cont = func.fresh_value();
        let key = func.fresh_value();
        let elem = func.fresh_value(); // Index(cont, key)
        let ch = func.fresh_value(); // OrdAt(cont, key) — i64, no borrow
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::LoadAttr, vec![obj], vec![h]));
            b.ops.push(op(OpCode::Index, vec![cont, key], vec![elem]));
            b.ops.push(op(OpCode::OrdAt, vec![cont, key], vec![ch]));
            b.terminator = Terminator::Return { values: vec![] };
        }
        let aliases = build_alias_union_find(&func);
        let canon = |v: ValueId| aliases.root(v);
        let bp = build_borrow_provenance(&func, &aliases);
        assert!(!bp.is_empty());
        // The LoadAttr result keeps `obj` alive.
        assert_eq!(bp.keepalive_roots(h, &canon), vec![aliases.root(obj)]);
        // The Index result keeps the container alive.
        assert_eq!(bp.keepalive_roots(elem, &canon), vec![aliases.root(cont)]);
        // `OrdAt` produces a scalar code point — no borrow keepalive.
        assert!(bp.keepalive_roots(ch, &canon).is_empty());
        // A non-borrow value (the container itself) has no keepalive sources.
        assert!(bp.keepalive_roots(cont, &canon).is_empty());
    }

    /// Borrow provenance is TRANSITIVE: `h2 = LoadAttr(h1); h1 = LoadAttr(obj)` —
    /// a use of `h2` keeps BOTH `h1` and `obj` alive (a chained interior borrow).
    #[test]
    fn borrow_provenance_is_transitive() {
        use crate::tir::blocks::Terminator;
        use crate::tir::function::TirFunction;
        use crate::tir::ops::{Dialect, TirOp};
        use crate::tir::types::TirType;

        fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
            TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands,
                results,
                attrs: AttrDict::new(),
                source_span: None,
            }
        }

        let mut func = TirFunction::new("bpt".into(), vec![], TirType::DynBox);
        let obj = func.fresh_value();
        let h1 = func.fresh_value();
        let h2 = func.fresh_value();
        let entry = func.entry_block;
        {
            let b = func.blocks.get_mut(&entry).unwrap();
            b.ops.push(op(OpCode::LoadAttr, vec![obj], vec![h1]));
            b.ops.push(op(OpCode::LoadAttr, vec![h1], vec![h2]));
            b.terminator = Terminator::Return { values: vec![] };
        }
        let aliases = build_alias_union_find(&func);
        let canon = |v: ValueId| aliases.root(v);
        let bp = build_borrow_provenance(&func, &aliases);
        let mut roots = bp.keepalive_roots(h2, &canon);
        roots.sort_by_key(|r| r.0);
        let mut expected = vec![aliases.root(h1), aliases.root(obj)];
        expected.sort_by_key(|r| r.0);
        assert_eq!(roots, expected, "h2 must keep both h1 and obj alive");
    }
}
