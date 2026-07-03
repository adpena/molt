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

mod copy_kind;
mod regions;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    AliasMemoryRegionClass, AliasSlotObservation, AliasTransparentAliasRole,
    opcode_alias_memory_region_table, opcode_alias_slot_observation_table,
    opcode_alias_transparent_alias_role_table,
};
use crate::tir::ops::TirOp;
use crate::tir::values::ValueId;

pub use super::escape_analysis::EscapeState;

// Region taxonomy, load-purity gate, and barrier core live in `regions`;
// `MemRegion` / `LoadPurity` are re-exported so external consumers keep using
// the `alias_analysis::MemRegion` / `alias_analysis::LoadPurity` paths.
pub use regions::{LoadPurity, MemRegion};
use regions::{
    classify_load, opcode_is_heap_barrier, opcode_is_rc_barrier, typed_slot_class,
    typed_slot_obj_offset, typed_slot_store,
};

// Copy-lowering ownership classifiers live in `copy_kind`; the crate-facing
// classifiers are re-exported so external consumers keep using the
// `alias_analysis::classify_copy_kind` (etc.) paths. The remaining helpers are
// consumed only by this module's own region/alias logic.
#[cfg(any(feature = "llvm", test))]
pub use copy_kind::copy_kind_reaches_no_incref_passthrough;
pub(crate) use copy_kind::{
    CopyLowering, classify_copy_kind, copy_kind_is_exception_creation_ref,
    copy_kind_is_explicit_no_heap_move, copy_kind_mints_fresh_owned_ref,
    copy_kind_raw_carrier_type,
};
use copy_kind::{copy_is_known_local_alias, copy_kind_is_memory_inert, copy_original_kind};

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
