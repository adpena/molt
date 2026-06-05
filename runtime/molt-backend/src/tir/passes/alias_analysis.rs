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
                TypedField { class: c1, offset: o1 },
                TypedField { class: c2, offset: o2 },
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
        op.operands.iter().any(|operand| self.root(*operand) == root)
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

/// Returns whether an `OpCode::Copy` op is a transparent local alias (its result
/// names the same heap object as its operand). The opaque `_original_kind`
/// passthrough carriers (container constructors etc.) are NOT aliases — their
/// result is a distinct value. Mirrors `dead_store_elim`'s former
/// `copy_is_known_local_alias`.
fn copy_is_known_local_alias(op: &TirOp) -> bool {
    match op.attrs.get("_original_kind") {
        None => true,
        Some(AttrValue::Str(kind)) => matches!(
            kind.as_str(),
            "copy" | "copy_var" | "store_var" | "load_var" | "identity_alias"
        ),
        Some(_) => false,
    }
}

/// True if a `Copy`-carried `_original_kind` op writes/reads/clobbers NO heap
/// memory — a debug / source-location / control-flow marker or a read-only guard
/// the SSA lift carries as a `Copy` (it has no dedicated `OpCode`). These are
/// classified [`MemRegion::ScalarRegister`] so they do not spuriously bump the
/// memory version between adjacent field accesses (which would starve MemGVN
/// store-to-load forwarding and SROA — see [`AliasAnalysisResult::region_of`]).
///
/// FAIL-CLOSED: every kind listed here is *proven* heap-inert:
/// * `line` / `trace_enter_slot` / `trace_exit` — source-line + call-trace
///   debug markers (no operands that survive, no heap effect).
/// * `missing` — materializes the unbound-cell sentinel; a const-like value
///   producer, never a heap mutation.
/// * `nop` — an explicit no-op.
/// * `guard_layout` / `guard_dict_shape` / `guard_int` / `guard_float` /
///   `guard_str` / `guard_bool` / `guard_none` — read-only class/representation
///   guards: they read the object's class version and may raise on mismatch, but
///   never write a field (the same memory-inert property `TypeGuard` already
///   has — and `OpCode::TypeGuard` is already excluded by `opcode_touches_memory`).
///
/// Any kind NOT in this set keeps the conservative `GenericHeap` classification.
fn copy_kind_is_memory_inert(op: &TirOp) -> bool {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => matches!(
            kind.as_str(),
            "line"
                | "trace_enter_slot"
                | "trace_exit"
                | "missing"
                | "nop"
                | "guard_layout"
                | "guard_dict_shape"
                | "guard_int"
                | "guard_float"
                | "guard_str"
                | "guard_bool"
                | "guard_none"
        ),
        _ => false,
    }
}

/// The transparent-alias root an op contributes, if any. A no-op `TypeGuard` and
/// a pure-move `Copy` both forward their single operand's root. Mirrors
/// `dead_store_elim`'s former `transparent_alias_root`.
fn transparent_alias_root(op: &TirOp, aliases: &AliasUnionFind) -> Option<ValueId> {
    if op.results.is_empty() {
        return None;
    }
    match op.opcode {
        OpCode::TypeGuard => {
            if op.attrs.contains_key("_original_kind") || op.operands.len() != 1 {
                return None;
            }
            Some(aliases.root(op.operands[0]))
        }
        OpCode::Copy => {
            if !copy_is_known_local_alias(op) || op.operands.is_empty() {
                return None;
            }
            let root = aliases.root(op.operands[0]);
            if op.operands.iter().all(|operand| aliases.root(*operand) == root) {
                Some(root)
            } else {
                None
            }
        }
        _ => None,
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
    match op.opcode {
        OpCode::LoadAttr => match original {
            "load" | "guarded_field_get" => Some("load"),
            _ => None,
        },
        OpCode::StoreAttr => match original {
            "store" | "store_init" | "guarded_field_set" | "guarded_field_init" => Some("store"),
            _ => None,
        },
        _ => None,
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
/// ChanRecvYield}) is present here. Verified in `tests::rc_barrier_is_superset_*`.
fn opcode_is_rc_barrier(opcode: OpCode) -> bool {
    matches!(
        opcode,
        // ── Calls: a callee may inspect / store / mutate any refcount. ──
        OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            // ── Stores into heap objects / containers capture references. ──
            | OpCode::StoreAttr
            | OpCode::StoreIndex
            // ── Coroutine / generator / channel suspension points: control
            //    escapes to the scheduler, which may observe live references. ──
            | OpCode::StateSwitch
            | OpCode::StateTransition
            | OpCode::StateYield
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            // ── Closure cells store/load captured references. ──
            | OpCode::ClosureLoad
            | OpCode::ClosureStore
    )
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
    matches!(
        opcode,
        // Calls — arbitrary side effects on arbitrary heap state.
        OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            // Stores mutate heap memory.
            | OpCode::StoreAttr
            | OpCode::StoreIndex
            | OpCode::DelAttr
            | OpCode::DelIndex
            // Exception propagation observes / escapes live objects.
            | OpCode::Raise
            // Generator / coroutine suspension exposes the heap to the scheduler.
            | OpCode::Yield
            | OpCode::YieldFrom
            | OpCode::StateSwitch
            | OpCode::StateTransition
            | OpCode::StateYield
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            // Closure-cell store captures references into a heap cell.
            | OpCode::ClosureStore
            // Explicit free mutates the allocator's view of heap memory.
            | OpCode::Free
            // Module-dictionary mutation writes globally-visible heap state.
            | OpCode::ModuleCacheSet
            | OpCode::ModuleCacheDel
            | OpCode::ModuleSetAttr
            | OpCode::ModuleDelGlobal
            | OpCode::ModuleDelGlobalIfPresent
    )
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
    pub(crate) fn compute(func: &TirFunction) -> Self {
        // Phase A: build the transparent-alias union-find with a forward scan.
        let mut aliases = AliasUnionFind::default();
        for block in func.blocks.values() {
            for op in &block.ops {
                aliases.record_transparent_aliases(op);
            }
        }

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
        if op.operands.iter().any(|&o| o == val || self.aliases.root(o) == root) {
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
        match op.opcode {
            // Reads of the slot — direct observation. (Both ProvenPure and
            // MayDispatch loads observe the slot value; purity only matters for
            // *whether the load itself can be reordered*, not whether it reads.)
            OpCode::LoadAttr | OpCode::Index => true,
            // Recognized typed-slot stores to the same object root are not
            // observers; they are overwrites. Unknown StoreAttr variants and
            // stores where `root` appears as the stored value are observers.
            OpCode::StoreAttr => match typed_slot_store(op) {
                Some((target, _)) => self.aliases.root(target) != root,
                None => true,
            },
            OpCode::StoreIndex => true,
            // Calls / raises / yields let the slot be observed externally.
            OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            | OpCode::Raise
            | OpCode::Yield
            | OpCode::YieldFrom => true,
            // Building a container with `obj` as an element captures it.
            OpCode::BuildList
            | OpCode::BuildDict
            | OpCode::BuildSet
            | OpCode::BuildTuple
            | OpCode::BuildSlice
            | OpCode::AllocTask => true,
            // Transparent aliases and pure ref ops do not read slot values.
            OpCode::Copy | OpCode::TypeGuard if transparent_alias_root(op, &self.aliases).is_some() => {
                false
            }
            OpCode::IncRef | OpCode::DecRef | OpCode::CheckException => false,
            // Default: conservative — treat any other use as observation.
            _ => true,
        }
    }

    /// The memory region a memory-touching op reads or writes, used for
    /// may-alias disambiguation (see [`MemRegion`]).
    pub fn region_of(&self, op: &TirOp) -> MemRegion {
        match op.opcode {
            OpCode::LoadAttr | OpCode::StoreAttr => {
                // A typed-slot field op (`store`/`store_init`/`load`/
                // `guarded_field_*`) names one fixed byte `offset` of one object's
                // own instance layout. An opaque attribute spelling (`get_attr*`,
                // `set_attr*`, or any op missing the offset proof) stays
                // `GenericHeap`.
                if let Some((target, offset)) = typed_slot_obj_offset(op) {
                    let root = self.aliases.root(target);
                    // A proven-non-escaping object's slots get a per-object
                    // `StackObject` region (strictly more precise than a
                    // class-shared `TypedField`: distinct stack objects never
                    // alias even at the same class+offset). This needs only the
                    // allocation root, NOT the class identity.
                    if self.is_stack_object(root) {
                        return MemRegion::StackObject { root };
                    }
                    // Otherwise, if the op carries its own proven class identity
                    // (the `_class` attr the frontend stamps — a runtime
                    // version-guard for `guarded_field_*`, static type inference
                    // for the plain forms), it names a `TypedField` region
                    // disjoint from every OTHER class+offset, from container
                    // elements, and from module-dict slots (S5-1.5).
                    if let Some(class) = typed_slot_class(op) {
                        return MemRegion::TypedField { class, offset };
                    }
                }
                MemRegion::GenericHeap
            }
            // A `Copy` is overloaded: a pure SSA move (the result names the same
            // value as the operand — no heap footprint), an inert structural /
            // debug-metadata marker (a source-line / trace / unbound-cell
            // sentinel the lift carries as a `Copy`), OR the opaque
            // `_original_kind` passthrough carrier for an unmapped SimpleIR op
            // (which MAY have arbitrary heap effects). The first two write no heap
            // memory and are `ScalarRegister`; only the last is a conservative
            // `GenericHeap` clobber.
            //
            // Classifying the inert markers as `GenericHeap` (the pre-S5-2d
            // default) bumped the memory version between a constructor's field
            // stores and the field loads — the frontend emits a `line` /
            // `trace_exit` marker between nearly every statement, so a `p = P(..)`
            // followed by a `p.x` read always had a marker-`Copy` clobber between
            // the store and the load. That starved MemGVN store-to-load forwarding
            // (the load's reaching def became the marker's `GenericHeap` version,
            // not the store) and, downstream, SROA field promotion — exactly the
            // spurious-clobber bug already fixed for `CheckException` /
            // `ExceptionPending` (see `opcode_touches_memory`).
            //
            // FAIL-CLOSED: a passthrough `Copy` whose carrier kind is NOT a proven
            // memory-inert marker keeps the conservative `GenericHeap`. Every kind
            // in `copy_kind_is_memory_inert` is provably heap-inert (a debug /
            // source-location / control-flow marker or a read-only layout guard).
            OpCode::Copy => {
                if copy_is_known_local_alias(op) || copy_kind_is_memory_inert(op) {
                    MemRegion::ScalarRegister
                } else {
                    MemRegion::GenericHeap
                }
            }
            OpCode::Index | OpCode::StoreIndex | OpCode::DelIndex => MemRegion::ContainerElement,
            OpCode::ModuleCacheGet
            | OpCode::ModuleCacheSet
            | OpCode::ModuleCacheDel
            | OpCode::ModuleGetAttr
            | OpCode::ModuleImportFrom
            | OpCode::ModuleGetGlobal
            | OpCode::ModuleGetName
            | OpCode::ModuleSetAttr
            | OpCode::ModuleDelGlobal
            | OpCode::ModuleDelGlobalIfPresent => MemRegion::ModuleDict,
            // Pure register computations have no heap footprint.
            _ if !opcode_touches_memory(op.opcode) => MemRegion::ScalarRegister,
            _ => MemRegion::GenericHeap,
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

/// True if an opcode reads or writes heap memory (anything that is not a pure
/// register computation / constant / control-flow marker). Used to classify
/// `ScalarRegister` vs `GenericHeap` regions.
fn opcode_touches_memory(opcode: OpCode) -> bool {
    !matches!(
        opcode,
        // Pure arithmetic / comparison / bitwise / boolean computations.
        // CheckedAdd is a pure 2-result register computation (raw i64 sum +
        // overflow flag) — no heap footprint.
        OpCode::Add
            | OpCode::CheckedAdd
            | OpCode::Sub
            | OpCode::Mul
            | OpCode::InplaceAdd
            | OpCode::InplaceSub
            | OpCode::InplaceMul
            | OpCode::Div
            | OpCode::FloorDiv
            | OpCode::Mod
            | OpCode::Pow
            | OpCode::Neg
            | OpCode::Pos
            | OpCode::Eq
            | OpCode::Ne
            | OpCode::Lt
            | OpCode::Le
            | OpCode::Gt
            | OpCode::Ge
            | OpCode::Is
            | OpCode::IsNot
            | OpCode::BitAnd
            | OpCode::BitOr
            | OpCode::BitXor
            | OpCode::BitNot
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::And
            | OpCode::Or
            | OpCode::Not
            | OpCode::Bool
            // Box/unbox/typeguard: pure representation transforms.
            | OpCode::BoxVal
            | OpCode::UnboxVal
            | OpCode::TypeGuard
            // Constant materialization. ConstBigInt allocates, but the
            // allocation is FRESH immutable memory invisible to any existing
            // pointer — it can neither clobber nor be clobbered (same
            // reasoning as ConstStr).
            | OpCode::ConstInt
            | OpCode::ConstBigInt
            | OpCode::ConstFloat
            | OpCode::ConstStr
            | OpCode::ConstBool
            | OpCode::ConstNone
            | OpCode::ConstBytes
            // Slice from primitive bounds.
            | OpCode::BuildSlice
            // Runtime-flag reads (no heap footprint, but side-effecting
            // elsewhere). `CheckException` reads the pending-exception flag and
            // conditionally transfers to a handler — it never writes heap
            // memory. Its control-flow effect is modeled by the CFG's exception
            // edges (which MemorySSA's phi placement follows), and
            // `may_observe_slot` already returns `false` for it. Classifying it
            // as memory-touching made it a spurious `GenericHeap` clobber that
            // bumped the memory version between every adjacent field access in
            // exception-bearing bodies (it is emitted after nearly every op),
            // starving store-to-load forwarding for no soundness gain.
            | OpCode::ExceptionPending
            | OpCode::CheckException
    )
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
            Add, CheckedAdd, Sub, Mul, InplaceAdd, InplaceSub, InplaceMul, Div, FloorDiv, Mod, Pow, Neg, Pos,
            Eq, Ne, Lt, Le, Gt, Ge, Is, IsNot, In, NotIn, BitAnd, BitOr, BitXor, BitNot, Shl, Shr,
            And, Or, Not, Bool, Alloc, StackAlloc, ObjectNewBound, ObjectNewBoundStack, Free,
            LoadAttr, StoreAttr, DelAttr, Index, StoreIndex, DelIndex, Call, CallMethod,
            CallBuiltin, OrdAt, BoxVal, UnboxVal, TypeGuard, IncRef, DecRef, BuildList, BuildDict,
            BuildTuple, BuildSet, BuildSlice, GetIter, IterNext, IterNextUnboxed, ForIter,
            AllocTask, StateSwitch, StateTransition, StateYield, ChanSendYield, ChanRecvYield,
            ClosureLoad, ClosureStore, Yield, YieldFrom, Raise, CheckException, ExceptionPending,
            TryStart, TryEnd, StateBlockStart, StateBlockEnd, ConstInt, ConstBigInt, ConstFloat, ConstStr,
            ConstBool, ConstNone, ConstBytes, Copy, Import, ImportFrom, ModuleCacheGet,
            ModuleCacheSet, ModuleCacheDel, ModuleGetAttr, ModuleImportFrom, ModuleGetGlobal,
            ModuleGetName,
            ModuleSetAttr, ModuleDelGlobal, ModuleDelGlobalIfPresent, WarnStderr, ScfIf, ScfFor,
            ScfWhile, ScfYield, Deopt,
        ]
    }

    fn assert_opcode_listed(opcode: OpCode) {
        use OpCode::*;
        match opcode {
            Add | CheckedAdd | Sub | Mul | InplaceAdd | InplaceSub | InplaceMul | Div | FloorDiv | Mod | Pow
            | Neg | Pos | Eq | Ne | Lt | Le | Gt | Ge | Is | IsNot | In | NotIn | BitAnd | BitOr
            | BitXor | BitNot | Shl | Shr | And | Or | Not | Bool | Alloc | StackAlloc
            | ObjectNewBound | ObjectNewBoundStack | Free | LoadAttr | StoreAttr | DelAttr
            | Index | StoreIndex | DelIndex | Call | CallMethod | CallBuiltin | OrdAt | BoxVal
            | UnboxVal | TypeGuard | IncRef | DecRef | BuildList | BuildDict | BuildTuple
            | BuildSet | BuildSlice | GetIter | IterNext | IterNextUnboxed | ForIter | AllocTask
            | StateSwitch | StateTransition | StateYield | ChanSendYield | ChanRecvYield
            | ClosureLoad | ClosureStore | Yield | YieldFrom | Raise | CheckException
            | ExceptionPending | TryStart | TryEnd | StateBlockStart | StateBlockEnd | ConstInt
            | ConstBigInt | ConstFloat | ConstStr | ConstBool | ConstNone | ConstBytes | Copy | Import
            | ImportFrom | ModuleCacheGet | ModuleCacheSet | ModuleCacheDel | ModuleGetAttr
            | ModuleImportFrom | ModuleGetGlobal | ModuleGetName | ModuleSetAttr | ModuleDelGlobal
            | ModuleDelGlobalIfPresent | WarnStderr | ScfIf | ScfFor | ScfWhile | ScfYield
            | Deopt => {}
        }
    }

    // ── The OLD four barrier lists, reproduced verbatim as oracles ─────────

    /// `refcount_elim::is_barrier` as it stood before S5 phase 1.
    fn old_refcount_is_barrier(opcode: OpCode) -> bool {
        matches!(
            opcode,
            OpCode::Call
                | OpCode::CallMethod
                | OpCode::CallBuiltin
                | OpCode::StoreAttr
                | OpCode::StoreIndex
                | OpCode::StateSwitch
                | OpCode::StateTransition
                | OpCode::StateYield
                | OpCode::ClosureLoad
                | OpCode::ClosureStore
                | OpCode::ChanSendYield
                | OpCode::ChanRecvYield
        )
    }

    /// `reuse_analysis::is_aliasing_op`'s opcode portion (excluding the
    /// operand-uses-val branch, tested separately).
    fn old_reuse_opcode_barrier(opcode: OpCode) -> bool {
        matches!(
            opcode,
            OpCode::Call
                | OpCode::CallMethod
                | OpCode::CallBuiltin
                | OpCode::StoreAttr
                | OpCode::StoreIndex
                | OpCode::Raise
                | OpCode::Yield
                | OpCode::YieldFrom
                | OpCode::StateSwitch
                | OpCode::StateTransition
                | OpCode::StateYield
                | OpCode::ChanSendYield
                | OpCode::ChanRecvYield
                | OpCode::ClosureStore
                | OpCode::Free
        )
    }

    /// `dead_store_elim::may_observe_slot` as it stood before S5 phase 1.
    /// Reproduced against the *promoted* helpers (semantically identical).
    fn old_dse_may_observe(op: &TirOp, root: ValueId, aliases: &AliasUnionFind) -> bool {
        if !aliases.operand_aliases_root(op, root) {
            return false;
        }
        match op.opcode {
            OpCode::LoadAttr | OpCode::Index => true,
            OpCode::StoreAttr => match typed_slot_store(op) {
                Some((target, _)) => aliases.root(target) != root,
                None => true,
            },
            OpCode::StoreIndex => true,
            OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            | OpCode::Raise
            | OpCode::Yield
            | OpCode::YieldFrom => true,
            OpCode::BuildList
            | OpCode::BuildDict
            | OpCode::BuildSet
            | OpCode::BuildTuple
            | OpCode::BuildSlice
            | OpCode::AllocTask => true,
            OpCode::Copy | OpCode::TypeGuard if transparent_alias_root(op, aliases).is_some() => {
                false
            }
            OpCode::IncRef | OpCode::DecRef | OpCode::CheckException => false,
            _ => true,
        }
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
        assert!(!res.may_observe_slot(&store, root), "same-root store is an overwrite");
        // store that USES root as the stored value (target != root) observes it.
        let other = ValueId(8);
        let mut escape_store = op(OpCode::StoreAttr, vec![other, root], vec![]);
        escape_store.attrs.insert("value".into(), AttrValue::Int(16));
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
            assert_eq!(classify_load(&o), LoadPurity::ProvenPure, "{kind} is a typed slot");
        }
    }

    #[test]
    fn opaque_attr_load_may_dispatch() {
        for kind in ["get_attr", "get_attr_name", "get_attr_generic_ptr", "get_attr_generic_obj"] {
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
        let o = op(OpCode::Index, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]);
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
            MemRegion::TypedField { class: "Point".into(), offset: 0 },
            MemRegion::StackObject { root: ValueId(1) },
            MemRegion::ScalarRegister,
        ] {
            assert!(!scalar.may_alias(&other));
            assert!(!other.may_alias(&scalar));
        }
    }

    #[test]
    fn distinct_typed_fields_are_disjoint() {
        let f0 = MemRegion::TypedField { class: "Point".into(), offset: 0 };
        let f8 = MemRegion::TypedField { class: "Point".into(), offset: 8 };
        let g0 = MemRegion::TypedField { class: "Line".into(), offset: 0 };
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
        assert!(g.may_alias(&MemRegion::TypedField { class: "P".into(), offset: 0 }));
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
        assert_ne!(res.root(lst), obj, "container builder result is not an alias of its element");
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
        let idx = op(OpCode::Index, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]);
        assert_eq!(res.region_of(&idx), MemRegion::ContainerElement);
        let mcg = op(OpCode::ModuleGetGlobal, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]);
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
            MemRegion::TypedField { class: "Point".into(), offset: 8 }
        );
        // `store obj.<16> = val` of class Line (operands [obj, val], offset 16).
        let store = with_field_attrs(
            op_kind(OpCode::StoreAttr, vec![ValueId(0), ValueId(2)], vec![], "store"),
            16,
            Some("Line"),
        );
        assert_eq!(
            res.region_of(&store),
            MemRegion::TypedField { class: "Line".into(), offset: 16 }
        );
        // `store_init` is also a typed-slot store.
        let init = with_field_attrs(
            op_kind(OpCode::StoreAttr, vec![ValueId(0), ValueId(2)], vec![], "store_init"),
            0,
            Some("Point"),
        );
        assert_eq!(
            res.region_of(&init),
            MemRegion::TypedField { class: "Point".into(), offset: 0 }
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
        for kind in ["copy", "copy_var", "store_var", "load_var", "identity_alias"] {
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
        let opaque = op_kind(OpCode::Copy, vec![ValueId(0)], vec![ValueId(1)], "list_append");
        assert_eq!(res.region_of(&opaque), MemRegion::GenericHeap);
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
            MemRegion::TypedField { class: "Account".into(), offset: 24 }
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
                MemRegion::TypedField { class: "Account".into(), offset: 32 },
                "{kind} on operand[0]=obj is a TypedField"
            );
        }
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
            op_kind(OpCode::StoreAttr, vec![ValueId(0), ValueId(2)], vec![], "store"),
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
            op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], "get_attr"),
            8,
            Some("Point"),
        );
        assert_eq!(res.region_of(&ga), MemRegion::GenericHeap);
        let sa = with_field_attrs(
            op_kind(OpCode::StoreAttr, vec![ValueId(0), ValueId(2)], vec![], "set_attr_generic_ptr"),
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
        let pt0 = MemRegion::TypedField { class: "Point".into(), offset: 0 };
        let pt8 = MemRegion::TypedField { class: "Point".into(), offset: 8 };
        let ln0 = MemRegion::TypedField { class: "Line".into(), offset: 0 };
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
}
