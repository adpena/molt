//! Memory-region taxonomy, load-purity gate, and the barrier opcode core.
//!
//! The TBAA-style [`MemRegion`] partition, the Python-dunder [`LoadPurity`]
//! soundness gate, the typed-slot store/field helpers, and the opcode-only
//! RC/heap barrier predicates. Split out of `alias_analysis.rs` as a move-only
//! decomposition; the union-find, borrow provenance, and cached
//! [`super::AliasAnalysisResult`] consume these via the parent module.

use crate::tir::op_kinds_generated::{
    AliasTypedSlotRole, opcode_alias_typed_slot_role_table, opcode_is_alias_rc_barrier_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

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
pub(super) fn classify_load(op: &TirOp) -> LoadPurity {
    match op.opcode {
        OpCode::LoadAttr if load_attr_is_typed_slot(&op.attrs) => LoadPurity::ProvenPure,
        _ => LoadPurity::MayDispatch,
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
pub(super) fn typed_slot_store(op: &TirOp) -> Option<(ValueId, i64)> {
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
pub(super) fn typed_slot_obj_offset(op: &TirOp) -> Option<(ValueId, i64)> {
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
pub(super) fn typed_slot_class(op: &TirOp) -> Option<String> {
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
pub(super) fn opcode_is_rc_barrier(opcode: OpCode) -> bool {
    opcode_is_alias_rc_barrier_table(opcode)
}
