use super::super::typed_slot_access::LoadPurity;
use super::*;
use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::TirValue;

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        origin: None,
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn op_kind(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    kind: molt_ir::OpKind,
) -> TirOp {
    let mut o = op(opcode, operands, results);
    o.origin = Some(kind);
    o
}

// ── The OLD four barrier lists, reproduced verbatim as oracles ─────────

const OLD_REFCOUNT_BARRIER_OPCODES: &[OpCode] = &[
    OpCode::Call,
    OpCode::CallMethod,
    OpCode::CallLoadedMethod,
    OpCode::CallBuiltin,
    OpCode::StoreAttr,
    OpCode::StoreIndex,
    OpCode::StateSwitch,
    OpCode::StateTransition,
    OpCode::StateYield,
    OpCode::ClosureLoad,
    OpCode::ClosureStore,
];

const OLD_DSE_DIRECT_OBSERVERS: &[OpCode] = &[
    OpCode::LoadAttr,
    OpCode::LoadMethod,
    OpCode::LoadSuperMethod,
    OpCode::Index,
    OpCode::StoreIndex,
    OpCode::Call,
    OpCode::CallMethod,
    OpCode::CallLoadedMethod,
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
        return match op.plain_typed_slot_store() {
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

/// `is_rc_barrier ⊇ refcount_elim::is_barrier` for EVERY opcode.
#[test]
fn rc_barrier_is_conservative_superset_of_old_refcount_list() {
    for &opcode in crate::tir::op_kinds_generated::ALL_OPCODES {
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

/// `may_observe_slot ⊇ dead_store_elim::may_observe_slot` for every opcode in
/// the old predicate's aliasing case. Callback-capable operations are now more
/// conservative and can observe roots absent from their operands.
#[test]
fn dse_observe_is_conservative_superset_of_old_may_observe() {
    let root = ValueId(3);
    let res = AliasAnalysisResult {
        exact_scalar_types: HashMap::new(),
        aliases: AliasUnionFind::default(),
        escape: HashMap::new(),
        alloc_roots: HashSet::new(),
    };
    for &opcode in crate::tir::op_kinds_generated::ALL_OPCODES {
        // Aliasing case: op uses `root`.
        let aliasing = op(opcode, vec![root], vec![ValueId(50)]);
        let old = old_dse_may_observe(&aliasing, root, &res.aliases);
        let new = res.may_observe_slot(&aliasing, root);
        assert!(
            !old || new,
            "{opcode:?}: old may_observe_slot=true but new=false (aliasing case) — \
             UNSOUND (would drop an observable store)"
        );
    }
}

#[test]
fn arbitrary_heap_effect_observes_roots_absent_from_operands() {
    let root = ValueId(3);
    let res = empty_res();
    for opcode in [
        OpCode::Add,
        OpCode::Index,
        OpCode::ModuleGetAttr,
        OpCode::ModuleGetName,
        OpCode::ModuleImportFrom,
        OpCode::FrameContextSet,
    ] {
        let callback = op(opcode, vec![ValueId(40), ValueId(41)], vec![ValueId(42)]);
        assert!(
            res.may_observe_slot(&callback, root),
            "{opcode:?}: callback can observe a captured root without receiving it"
        );
    }
}

#[test]
fn check_exception_callback_effect_is_instance_sensitive() {
    let root = ValueId(3);
    let res = empty_res();
    let plain = op(OpCode::CheckException, vec![], vec![]);
    assert!(!res.may_observe_slot(&plain, root));
    assert_eq!(res.region_of(&plain), MemRegion::ScalarRegister);

    let mut poll = plain.clone();
    assert!(poll.mark_async_work_poll());
    assert!(res.may_observe_slot(&poll, root));
    assert_eq!(res.region_of(&poll), MemRegion::GenericHeap);
}

#[test]
fn local_memory_operations_cannot_claim_async_work_observation_role() {
    let mut load = op_kind(
        OpCode::LoadAttr,
        vec![ValueId(0)],
        vec![ValueId(1)],
        molt_ir::OpKind::Load,
    );
    load.attrs.insert("value".into(), AttrValue::Int(0));
    assert!(!load.can_carry_async_work_poll());
    assert_eq!(load.plain_typed_slot_load(), Some((ValueId(0), 0)));

    let mut store = op_kind(
        OpCode::StoreAttr,
        vec![ValueId(0), ValueId(2)],
        vec![],
        molt_ir::OpKind::Store,
    );
    store.attrs.insert("value".into(), AttrValue::Int(0));
    assert!(!store.can_carry_async_work_poll());
    assert_eq!(store.plain_typed_slot_store(), Some((ValueId(0), 0)));
}

/// An ordinary store remains an observer unless the shared pristine-slot
/// analysis has separately proved its old release callback-free.
#[test]
fn typed_slot_store_requires_contextual_release_proof() {
    let root = ValueId(3);
    let val = ValueId(4);
    let res = AliasAnalysisResult {
        exact_scalar_types: HashMap::new(),
        aliases: AliasUnionFind::default(),
        escape: HashMap::new(),
        alloc_roots: HashSet::new(),
    };
    let mut store = op(OpCode::StoreAttr, vec![root, val], vec![]);
    store.attrs.insert("value".into(), AttrValue::Int(0));
    store.origin = Some(molt_ir::OpKind::Store);
    assert!(
        res.may_observe_slot(&store, root),
        "replacing store may run an arbitrary old-value destructor"
    );
    assert!(
        !res.may_observe_slot_with_boxed_neutral_old_value(&store, root),
        "a pristine local root and boxed-neutral old value discharge every release path"
    );
    assert!(
        !res.may_observe_slot_with_boxed_neutral_old_value(&store, ValueId(9)),
        "a fully discharged replacing store does not observe an unrelated root"
    );
    // store that USES root as the stored value (target != root) observes it.
    let other = ValueId(8);
    let mut escape_store = op(OpCode::StoreAttr, vec![other, root], vec![]);
    escape_store
        .attrs
        .insert("value".into(), AttrValue::Int(16));
    escape_store.origin = Some(molt_ir::OpKind::Store);
    assert!(
        res.may_observe_slot(&escape_store, root),
        "storing root into another object observes/escapes it"
    );
}

// ── LoadPurity dunder gate ─────────────────────────────────────────────

#[test]
fn default_plan_keeps_plain_and_guarded_loads_may_dispatch() {
    let plan = super::super::typed_slot_access::TypedSlotAccessPlan::default();
    let res = empty_res();
    let load = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![ValueId(0)],
            vec![ValueId(1)],
            molt_ir::OpKind::Load,
        ),
        8,
    );
    assert_eq!(
        plan.load_purity_at((BlockId(0), 0), &load),
        LoadPurity::MayDispatch
    );
    assert_eq!(res.region_of(&load), MemRegion::GenericHeap);
    assert!(
        res.may_observe_slot(&load, ValueId(9)),
        "an unadmitted load fallback may observe a root absent from its operands"
    );

    let guarded = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![ValueId(0), ValueId(3)],
            vec![ValueId(1)],
            molt_ir::OpKind::GuardedFieldGet,
        ),
        8,
    );
    assert_eq!(
        plan.load_purity_at((BlockId(0), 1), &guarded),
        LoadPurity::MayDispatch
    );
}

#[test]
fn opaque_attr_load_may_dispatch() {
    let plan = super::super::typed_slot_access::TypedSlotAccessPlan::default();
    for kind in [
        molt_ir::OpKind::GetAttr,
        molt_ir::OpKind::GetAttrName,
        molt_ir::OpKind::GetAttrGenericPtr,
        molt_ir::OpKind::GetAttrGenericObj,
    ] {
        let o = op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], kind);
        assert_eq!(
            plan.load_purity_at((BlockId(0), 0), &o),
            LoadPurity::MayDispatch,
            "{kind} can dispatch __getattr__/__getattribute__"
        );
    }
    // A LoadAttr with no kind annotation is conservatively opaque.
    let bare = op(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)]);
    assert_eq!(
        plan.load_purity_at((BlockId(0), 0), &bare),
        LoadPurity::MayDispatch
    );
}

#[test]
fn index_always_may_dispatch() {
    // Index can dispatch __getitem__ regardless of any attr.
    let o = op(
        OpCode::Index,
        vec![ValueId(0), ValueId(1)],
        vec![ValueId(2)],
    );
    assert_eq!(
        super::super::typed_slot_access::TypedSlotAccessPlan::default()
            .load_purity_at((BlockId(0), 0), &o),
        LoadPurity::MayDispatch
    );
}

// ── MemRegion may-alias ────────────────────────────────────────────────

#[test]
fn scalar_register_aliases_nothing() {
    let scalar = MemRegion::ScalarRegister;
    for other in [
        MemRegion::GenericHeap,
        MemRegion::ContainerElement,
        MemRegion::ModuleDict,
        MemRegion::Field {
            allocation: None,
            offset: 0,
        },
        MemRegion::LocalAllocation { root: ValueId(1) },
        MemRegion::ScalarRegister,
    ] {
        assert!(!scalar.may_alias(&other));
        assert!(!other.may_alias(&scalar));
    }
}

#[test]
fn field_aliasing_uses_allocation_identity_and_word_overlap() {
    let unknown0 = MemRegion::Field {
        allocation: None,
        offset: 0,
    };
    let unknown8 = MemRegion::Field {
        allocation: None,
        offset: 8,
    };
    let first0 = MemRegion::Field {
        allocation: Some(ValueId(1)),
        offset: 0,
    };
    let first8 = MemRegion::Field {
        allocation: Some(ValueId(1)),
        offset: 8,
    };
    let second0 = MemRegion::Field {
        allocation: Some(ValueId(2)),
        offset: 0,
    };
    assert!(
        unknown0.may_alias(&first0),
        "unknown receiver may be allocation 1"
    );
    assert!(
        unknown0.may_alias(&second0),
        "unknown receiver may be allocation 2"
    );
    assert!(
        !unknown0.may_alias(&unknown8),
        "different words are disjoint"
    );
    assert!(!first0.may_alias(&first8), "different words are disjoint");
    assert!(
        first0.may_alias(&first0.clone()),
        "same allocation and word may alias"
    );
    assert!(
        !first0.may_alias(&second0),
        "distinct proven allocations are disjoint"
    );
}

#[test]
fn distinct_local_allocations_are_disjoint() {
    let a = MemRegion::LocalAllocation { root: ValueId(1) };
    let b = MemRegion::LocalAllocation { root: ValueId(2) };
    assert!(!a.may_alias(&b));
    assert!(a.may_alias(&a.clone()));
    // Callback/destruction effects remain observable regardless of placement.
    assert!(a.may_alias(&MemRegion::GenericHeap));
    assert!(MemRegion::GenericHeap.may_alias(&a));
}

#[test]
fn field_alias_matrix_preserves_word_overlap_and_callback_barriers() {
    let field = MemRegion::Field {
        allocation: Some(ValueId(1)),
        offset: 0,
    };
    for (other, expected) in [
        (
            MemRegion::Field {
                allocation: Some(ValueId(1)),
                offset: 0,
            },
            true,
        ),
        (
            MemRegion::Field {
                allocation: Some(ValueId(1)),
                offset: 7,
            },
            true,
        ),
        (
            MemRegion::Field {
                allocation: Some(ValueId(1)),
                offset: 8,
            },
            false,
        ),
        (
            MemRegion::Field {
                allocation: Some(ValueId(2)),
                offset: 0,
            },
            false,
        ),
        (MemRegion::LocalAllocation { root: ValueId(1) }, true),
        (MemRegion::LocalAllocation { root: ValueId(2) }, false),
        (
            MemRegion::Field {
                allocation: None,
                offset: 0,
            },
            true,
        ),
        (MemRegion::ModuleDict, false),
        (MemRegion::ContainerElement, false),
        (MemRegion::GenericHeap, true),
        (MemRegion::ScalarRegister, false),
    ] {
        assert_eq!(field.may_alias(&other), expected, "{other:?}");
        assert_eq!(other.may_alias(&field), expected, "symmetric {other:?}");
    }
    let field = |offset| MemRegion::Field {
        allocation: None,
        offset,
    };
    assert!(
        field(0).may_alias(&field(7)),
        "partial boxed-word overlap is not disjoint"
    );
    assert!(!field(0).may_alias(&field(8)));
    assert!(
        !field(0).may_alias(&field(i64::MAX)),
        "extent checks cannot overflow"
    );
}

#[test]
fn generic_heap_aliases_opaque_regions() {
    let g = MemRegion::GenericHeap;
    assert!(g.may_alias(&MemRegion::ContainerElement));
    assert!(g.may_alias(&MemRegion::ModuleDict));
    assert!(g.may_alias(&MemRegion::GenericHeap));
    assert!(g.may_alias(&MemRegion::Field {
        allocation: None,
        offset: 0
    }));
}

// ── AliasUnionFind ─────────────────────────────────────────────────────

#[test]
fn transparent_copy_chain_resolves_to_root() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
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
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let obj = ValueId(0);
    let lst = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    // lst = Copy[list_new] obj  — result is a NEW container, not an alias.
    entry.ops.push(op_kind(
        OpCode::Copy,
        vec![obj],
        vec![lst],
        molt_ir::OpKind::ListNew,
    ));
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
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let obj = ValueId(0);
    let alias = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(op_kind(
        OpCode::Copy,
        vec![obj],
        vec![alias],
        molt_ir::OpKind::BindingAlias,
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

/// Every `origin` classifies into exactly one [`CopyLowering`] bucket,
/// and the derived custody, inertness, and passthrough predicates agree with
/// that generated classifier. Non-owning custody alone does not prove source
/// identity; alias-root unions use the separate shared `no_heap_alias_source`
/// fact and its declared-shape validation.
#[test]
fn copy_lowering_classes_are_total_and_disjoint() {
    // A representative sample spanning the buckets, plus the bare-Copy
    // (None) case and the bug-repro fresh-value kinds the review flagged.
    let alias = [
        None,
        Some(molt_ir::OpKind::Copy),
        Some(molt_ir::OpKind::CopyVar),
        Some(molt_ir::OpKind::StoreVar),
        Some(molt_ir::OpKind::LoadVar),
        Some(molt_ir::OpKind::IdentityAlias),
        // validate-and-pass-through guards: result == operand 0, no incref.
        Some(molt_ir::OpKind::GuardTag),
        Some(molt_ir::OpKind::GuardType),
    ];
    let inert = [
        Some(molt_ir::OpKind::Line),
        Some(molt_ir::OpKind::Missing),
        Some(molt_ir::OpKind::Nop),
        Some(molt_ir::OpKind::GuardLayout),
        Some(molt_ir::OpKind::GuardInt),
        Some(molt_ir::OpKind::GuardFloat),
        Some(molt_ir::OpKind::GuardStr),
        Some(molt_ir::OpKind::GuardBool),
        Some(molt_ir::OpKind::GuardNone),
    ];
    // The fresh-value kinds the drop pass releases independently. Each MUST
    // classify OwnedValue (incl. the review's double-free root `slice` and the
    // generator-iterator `iter`) AND must NOT be allowed to reach the benign
    // no-incref passthrough.
    let fresh = [
        Some(molt_ir::OpKind::BuiltinFunc),
        Some(molt_ir::OpKind::Slice),
        Some(molt_ir::OpKind::SliceNew),
        Some(molt_ir::OpKind::StringFormat),
        Some(molt_ir::OpKind::ReprFromObj),
        Some(molt_ir::OpKind::IntFromObj),
        Some(molt_ir::OpKind::FloatFromObj),
        Some(molt_ir::OpKind::Contains),
        Some(molt_ir::OpKind::ClassmethodNew),
        Some(molt_ir::OpKind::CodeNew),
        Some(molt_ir::OpKind::DataclassNewValues),
        Some(molt_ir::OpKind::StrFromObj),
        Some(molt_ir::OpKind::Iter),
        Some(molt_ir::OpKind::Aiter),
        Some(molt_ir::OpKind::Enumerate),
        Some(molt_ir::OpKind::FuncNew),
        Some(molt_ir::OpKind::FuncNewClosure),
        Some(molt_ir::OpKind::GetAttrNameDefault),
        Some(molt_ir::OpKind::DictKeys),
        Some(molt_ir::OpKind::DictValues),
        Some(molt_ir::OpKind::DictItems),
        Some(molt_ir::OpKind::DictFromObj),
        Some(molt_ir::OpKind::ObjectNew),
        Some(molt_ir::OpKind::PropertyNew),
        Some(molt_ir::OpKind::ComplexFromObj),
        Some(molt_ir::OpKind::ListNew),
        Some(molt_ir::OpKind::ListPop),
        Some(molt_ir::OpKind::DictNew),
        Some(molt_ir::OpKind::TupleNew),
        Some(molt_ir::OpKind::StringJoin),
        Some(molt_ir::OpKind::StaticmethodNew),
        // range()'s bound conversion mints an owned exact int.
        Some(molt_ir::OpKind::OperatorIndex),
        // Fused-loop kernels: each returns a fresh owned result tuple.
        Some(molt_ir::OpKind::VecSum),
        Some(molt_ir::OpKind::VecProd),
        Some(molt_ir::OpKind::VecMin),
        Some(molt_ir::OpKind::VecMax),
        Some(molt_ir::OpKind::StringSplitWsDictInc),
        Some(molt_ir::OpKind::StringSplitSepDictInc),
        Some(molt_ir::OpKind::DictStrIntInc),
    ];
    let owned_alias = [Some(molt_ir::OpKind::BindingAlias)];

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
            CopyLowering::OwnedValue,
            "{k:?} mints a fresh owned value"
        );
        assert!(
            !copy_kind_reaches_no_incref_passthrough(k),
            "{k:?} must NOT reach the benign passthrough — a OwnedValue that fell \
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
}

/// The exact double-free vector from the adversarial review: a `Copy` carrying
/// `origin = "slice"` (the `s[-5:]` subscript) must NOT be unioned into
/// its source operand's alias root. If it were treated as a transparent alias,
/// the drop pass would drop the slice and its source as one group — but they
/// are two independent owned references on a correct (OwnedValue) backend.
#[test]
fn slice_subscript_copy_is_a_fresh_value_not_an_alias() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::Str],
        TirType::Str,
        molt_ir::FunctionReturnAbi::Value,
    );
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
        molt_ir::OpKind::Slice,
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

#[test]
fn unpack_sequence_results_are_independent_owned_roots() {
    let mut func = TirFunction::new(
        "unpack".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let sequence = ValueId(0);
    let first = func.fresh_value();
    let second = func.fresh_value();
    for value in [first, second] {
        func.value_types.insert(value, TirType::DynBox);
    }
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let mut unpack = op(OpCode::UnpackSequence, vec![sequence], vec![first, second]);
    unpack.attrs.insert("value".into(), AttrValue::Int(2));
    entry.ops.push(unpack);
    entry.terminator = Terminator::Return { values: vec![] };

    let res = AliasAnalysisResult::compute(&func);
    assert_ne!(res.root(first), res.root(sequence));
    assert_ne!(res.root(second), res.root(sequence));
    assert_ne!(
        res.root(first),
        res.root(second),
        "each unpack output carries an independent runtime +1 reference"
    );
}

// ── Escape map plumbing + S1 caching ───────────────────────────────────

#[test]
fn escape_map_matches_escape_analysis_and_caches() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Value,
    );
    let class_ref = ValueId(0);
    let inst = func.fresh_value();
    let load = func.fresh_value();
    let none = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .push(op(OpCode::ObjectNewBound, vec![class_ref], vec![inst]));
    entry.ops.push(op(OpCode::Is, vec![inst, inst], vec![load]));
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
fn region_of_gates_coarse_regions_on_arbitrary_heap_effects() {
    let add = op(OpCode::Add, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]);
    let dynamic = AliasAnalysisResult {
        exact_scalar_types: HashMap::new(),
        aliases: AliasUnionFind::default(),
        escape: HashMap::new(),
        alloc_roots: HashSet::new(),
    };
    assert_eq!(dynamic.region_of(&add), MemRegion::GenericHeap);
    let idx = op(
        OpCode::Index,
        vec![ValueId(0), ValueId(1)],
        vec![ValueId(2)],
    );
    assert_eq!(dynamic.region_of(&idx), MemRegion::GenericHeap);
    let module_callback = op(
        OpCode::ModuleGetGlobal,
        vec![ValueId(0), ValueId(1)],
        vec![ValueId(2)],
    );
    assert_eq!(dynamic.region_of(&module_callback), MemRegion::GenericHeap);
    let cache_get = op(OpCode::ModuleCacheGet, vec![ValueId(0)], vec![ValueId(2)]);
    assert_eq!(dynamic.region_of(&cache_get), MemRegion::GenericHeap);
    assert!(dynamic.may_observe_slot(&cache_get, ValueId(9)));

    let exact = AliasAnalysisResult {
        exact_scalar_types: HashMap::from([(ValueId(0), TirType::I64), (ValueId(1), TirType::I64)]),
        aliases: AliasUnionFind::default(),
        escape: HashMap::new(),
        alloc_roots: HashSet::new(),
    };
    assert_eq!(exact.region_of(&add), MemRegion::ScalarRegister);
    assert!(!exact.may_observe_slot(&add, ValueId(9)));
}

#[test]
fn allocation_callback_boundaries_preserve_only_independent_slot_facts() {
    let res = empty_res();
    let stored_root = ValueId(0);
    let independent = ValueId(1);

    for opcode in [
        OpCode::Alloc,
        OpCode::StackAlloc,
        OpCode::BuildList,
        OpCode::BuildTuple,
        OpCode::BuildSlice,
    ] {
        let allocation = op(opcode, vec![independent], vec![ValueId(2)]);
        assert!(
            !res.may_observe_slot(&allocation, stored_root),
            "{opcode:?} must preserve a pristine unrelated slot"
        );
    }

    for opcode in [OpCode::BuildDict, OpCode::BuildSet, OpCode::ObjectNewBound] {
        let allocation = op(opcode, vec![independent], vec![ValueId(2)]);
        assert!(
            res.may_observe_slot(&allocation, stored_root),
            "{opcode:?} must retain its callback/finalizer floor"
        );
    }

    for opcode in [OpCode::BuildList, OpCode::BuildTuple, OpCode::BuildSlice] {
        let capturing_allocation = op(opcode, vec![stored_root], vec![ValueId(2)]);
        assert!(
            res.may_observe_slot(&capturing_allocation, stored_root),
            "{opcode:?} must still observe a retained source root"
        );
    }
}

#[test]
fn raw_allocation_writes_share_proven_local_storage_without_erasing_effects() {
    let root = ValueId(2);
    let mut res = empty_res();
    res.escape.insert(root, EscapeState::NoEscape);
    res.alloc_roots.insert(root);
    for opcode in [OpCode::Alloc, OpCode::StackAlloc] {
        let mut allocation = op(opcode, vec![], vec![root]);
        allocation.attrs.insert("value".into(), AttrValue::Int(8));
        assert_eq!(
            res.region_of(&allocation),
            MemRegion::LocalAllocation { root }
        );
        assert!(!res.region_of(&allocation).may_alias(&MemRegion::ModuleDict));
        assert!(
            res.region_of(&allocation)
                .may_alias(&MemRegion::GenericHeap)
        );
        assert!(
            res.region_of(&allocation)
                .may_alias(&MemRegion::LocalAllocation { root })
        );
        assert!(
            !super::super::effects::op_is_pure_movable_with_types(
                &allocation,
                &res.exact_scalar_types,
            ),
            "storage disjointness must not make allocation pure or movable"
        );
        let field = with_field_attrs(
            op_kind(
                OpCode::StoreAttr,
                vec![root, ValueId(1)],
                vec![],
                molt_ir::OpKind::Store,
            ),
            0,
        );
        assert_eq!(res.region_of(&field), MemRegion::GenericHeap);

        let mut captured = res.clone();
        captured.escape.insert(root, EscapeState::GlobalEscape);
        assert_eq!(captured.region_of(&allocation), MemRegion::GenericHeap);
        assert_eq!(empty_res().region_of(&allocation), MemRegion::GenericHeap);
        for results in [vec![], vec![root, ValueId(3)]] {
            allocation.results = results;
            assert_eq!(res.region_of(&allocation), MemRegion::GenericHeap);
        }
    }
}

#[test]
fn fresh_result_identity_does_not_disprove_constructor_or_poll_callbacks() {
    let root = ValueId(2);
    let mut res = empty_res();
    res.escape.insert(root, EscapeState::NoEscape);
    res.alloc_roots.insert(root);
    for opcode in [
        OpCode::ObjectNewBound,
        OpCode::BuildDict,
        OpCode::BuildSet,
        OpCode::AllocTask,
    ] {
        let constructor = op(opcode, vec![ValueId(0)], vec![root]);
        assert_eq!(res.region_of(&constructor), MemRegion::GenericHeap);
    }
    let named_constructor = op_kind(
        OpCode::Copy,
        vec![ValueId(0)],
        vec![root],
        molt_ir::OpKind::AllocClass,
    );
    assert_eq!(res.region_of(&named_constructor), MemRegion::GenericHeap);
    let mut poll = op(OpCode::CheckException, vec![], vec![]);
    assert!(poll.mark_async_work_poll());
    assert_eq!(res.region_of(&poll), MemRegion::GenericHeap);
}

// ── region_of: direct physical field regions ─────────────────────

/// Set the physical offset on a field op.
fn with_field_attrs(mut o: TirOp, offset: i64) -> TirOp {
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o
}

fn empty_res() -> AliasAnalysisResult {
    AliasAnalysisResult {
        exact_scalar_types: HashMap::new(),
        aliases: AliasUnionFind::default(),
        escape: HashMap::new(),
        alloc_roots: HashSet::new(),
    }
}

#[test]
fn plain_load_has_physical_projection_but_context_free_accesses_are_generic() {
    let res = empty_res();
    let load = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![ValueId(0)],
            vec![ValueId(1)],
            molt_ir::OpKind::Load,
        ),
        8,
    );
    assert_eq!(
        res.typed_slot_region(&load),
        MemRegion::Field {
            allocation: None,
            offset: 8
        }
    );
    assert_eq!(res.region_of(&load), MemRegion::GenericHeap);
    let store = with_field_attrs(
        op_kind(
            OpCode::StoreAttr,
            vec![ValueId(0), ValueId(2)],
            vec![],
            molt_ir::OpKind::Store,
        ),
        16,
    );
    // Store release behavior is site-contextual, so only the retained typed
    // store plan may refine it beyond GenericHeap.
    assert_eq!(res.region_of(&store), MemRegion::GenericHeap);
}

#[test]
fn loads_of_one_offset_share_one_physical_field() {
    let res = empty_res();
    let base_load = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![ValueId(0)],
            vec![ValueId(1)],
            molt_ir::OpKind::Load,
        ),
        24,
    );
    let derived_load = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![ValueId(0)],
            vec![ValueId(2)],
            molt_ir::OpKind::Load,
        ),
        24,
    );
    let base_region = res.typed_slot_region(&base_load);
    let derived_region = res.typed_slot_region(&derived_load);
    assert_eq!(
        base_region,
        MemRegion::Field {
            allocation: None,
            offset: 24,
        }
    );
    assert_eq!(base_region, derived_region);
    assert!(
        base_region.may_alias(&derived_region),
        "two loads of one offset name the same boxed word"
    );
    assert_eq!(res.region_of(&base_load), MemRegion::GenericHeap);
    assert_eq!(res.region_of(&derived_load), MemRegion::GenericHeap);
}

/// A `Copy` is classified by whether it touches heap memory: a pure SSA
/// move (no `origin`) and the inert debug / source-location / guard
/// markers are `ScalarRegister`; an opaque passthrough carrier stays the
/// conservative `GenericHeap`. Lifecycle trace operations are effectful:
/// entry acquires public namespaces and exit releases frame-owned references.
/// They invalidate memory and exact-slot history despite non-owning results.
#[test]
fn copy_region_pure_and_inert_markers_are_scalar() {
    let res = empty_res();

    // Pure SSA move (no `origin`): identity plumbing, no heap.
    let pure_move = op(OpCode::Copy, vec![ValueId(0)], vec![ValueId(1)]);
    assert_eq!(res.region_of(&pure_move), MemRegion::ScalarRegister);

    // Known-local-alias kinds are pure moves too.
    for kind in [
        molt_ir::OpKind::Copy,
        molt_ir::OpKind::CopyVar,
        molt_ir::OpKind::StoreVar,
        molt_ir::OpKind::LoadVar,
        molt_ir::OpKind::IdentityAlias,
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
        molt_ir::OpKind::Line,
        molt_ir::OpKind::Missing,
        molt_ir::OpKind::Nop,
        molt_ir::OpKind::GuardLayout,
        molt_ir::OpKind::GuardInt,
        molt_ir::OpKind::GuardFloat,
        molt_ir::OpKind::GuardStr,
        molt_ir::OpKind::GuardBool,
        molt_ir::OpKind::GuardNone,
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
        molt_ir::OpKind::ListAppend,
    );
    assert_eq!(res.region_of(&opaque), MemRegion::GenericHeap);

    let owned_alias = op_kind(
        OpCode::Copy,
        vec![ValueId(0)],
        vec![ValueId(1)],
        molt_ir::OpKind::BindingAlias,
    );
    assert_eq!(res.region_of(&owned_alias), MemRegion::GenericHeap);
}

#[test]
fn guarded_field_get_abi_falls_back_to_generic_heap() {
    // `guarded_field_get` ABI (HF-220): operands [obj, expected_version],
    // offset in `value`, class statement in `class_definition`.
    let res = empty_res();
    let get = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![ValueId(0), ValueId(2)], // obj, version
            vec![ValueId(3)],
            molt_ir::OpKind::GuardedFieldGet,
        ),
        24,
    );
    assert_eq!(res.region_of(&get), MemRegion::GenericHeap);
    assert_eq!(
        super::super::typed_slot_access::TypedSlotAccessPlan::default()
            .load_purity_at((BlockId(0), 0), &get),
        LoadPurity::MayDispatch
    );
}

#[test]
fn guarded_field_set_abi_falls_back_to_generic_heap() {
    // `guarded_field_set` ABI (HF-220): operands [obj, expected_version, val],
    // offset in `value`, class statement in `class_definition`.
    let res = empty_res();
    let set = with_field_attrs(
        op_kind(
            OpCode::StoreAttr,
            vec![ValueId(0), ValueId(2), ValueId(3)],
            vec![],
            molt_ir::OpKind::GuardedFieldSet,
        ),
        32,
    );
    assert_eq!(res.region_of(&set), MemRegion::GenericHeap);
}

#[test]
fn direct_field_uses_unknown_allocation_identity() {
    // Exact load shape and offset admit the field. The physical footprint does
    // not mint object identity. Ordinary stores remain GenericHeap without
    // site-specific release-neutral proof.
    let res = empty_res();
    let load = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![ValueId(0)],
            vec![ValueId(1)],
            molt_ir::OpKind::Load,
        ),
        8,
    );
    assert_eq!(
        res.typed_slot_region(&load),
        MemRegion::Field {
            allocation: None,
            offset: 8,
        }
    );
    assert_eq!(res.region_of(&load), MemRegion::GenericHeap);
    let store = with_field_attrs(
        op_kind(
            OpCode::StoreAttr,
            vec![ValueId(0), ValueId(2)],
            vec![],
            molt_ir::OpKind::Store,
        ),
        8,
    );
    assert_eq!(res.region_of(&store), MemRegion::GenericHeap);
}

#[test]
fn opaque_attr_spelling_is_generic_heap_even_with_field_attrs() {
    // A generic `get_attr` / `set_attr` spelling is NOT a typed-slot op (it
    // may dispatch a dunder), so it is GenericHeap regardless of any stray
    // field attrs.
    let res = empty_res();
    let ga = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![ValueId(0)],
            vec![ValueId(1)],
            molt_ir::OpKind::GetAttr,
        ),
        8,
    );
    assert_eq!(res.region_of(&ga), MemRegion::GenericHeap);
    let sa = with_field_attrs(
        op_kind(
            OpCode::StoreAttr,
            vec![ValueId(0), ValueId(2)],
            vec![],
            molt_ir::OpKind::SetAttrGenericPtr,
        ),
        8,
    );
    assert_eq!(res.region_of(&sa), MemRegion::GenericHeap);
}

#[test]
fn exact_allocation_field_keeps_identity_independent_of_escape() {
    // Exact allocation-site identity, not escape placement, admits
    // the per-allocation field refinement.
    let root = ValueId(0);
    let mut escape = HashMap::new();
    escape.insert(root, EscapeState::GlobalEscape);
    let res = AliasAnalysisResult {
        exact_scalar_types: HashMap::new(),
        aliases: AliasUnionFind::default(),
        escape,
        alloc_roots: [root].into_iter().collect(),
    };
    let load = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![root],
            vec![ValueId(1)],
            molt_ir::OpKind::Load,
        ),
        8,
    );
    assert_eq!(
        res.typed_slot_region(&load),
        MemRegion::Field {
            allocation: Some(root),
            offset: 8,
        }
    );
    assert_eq!(res.region_of(&load), MemRegion::GenericHeap);
    // Class metadata cannot change allocation identity.
    let load_c = with_field_attrs(
        op_kind(
            OpCode::LoadAttr,
            vec![root],
            vec![ValueId(1)],
            molt_ir::OpKind::Load,
        ),
        8,
    );
    assert_eq!(
        res.typed_slot_region(&load_c),
        MemRegion::Field {
            allocation: Some(root),
            offset: 8,
        }
    );
    assert_eq!(res.region_of(&load_c), MemRegion::GenericHeap);
}

#[test]
fn cfg_parameter_that_may_select_external_does_not_mint_allocation_identity() {
    let mut func = TirFunction::new(
        "mixed_field_receiver".into(),
        vec![TirType::DynBox, TirType::Bool],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let external = ValueId(0);
    let control = ValueId(1);
    let allocation = func.fresh_value();
    let parameter = func.fresh_value();
    let loaded = func.fresh_value();
    let join = func.fresh_block();

    let mut alloc = op(OpCode::Alloc, vec![], vec![allocation]);
    alloc.attrs.insert("value".into(), AttrValue::Int(16));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(alloc);
    entry.terminator = Terminator::CondBranch {
        cond: control,
        then_block: join,
        then_args: vec![allocation],
        else_block: join,
        else_args: vec![external],
    };
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: parameter,
                ty: TirType::DynBox,
            }],
            ops: vec![with_field_attrs(
                op_kind(
                    OpCode::LoadAttr,
                    vec![parameter],
                    vec![loaded],
                    molt_ir::OpKind::Load,
                ),
                0,
            )],
            terminator: Terminator::Return {
                values: vec![loaded],
            },
        },
    );

    let res = AliasAnalysisResult::compute(&func);
    assert!(res.alloc_roots.contains(&res.root(allocation)));
    assert!(
        !res.alloc_roots.contains(&res.root(parameter)),
        "a CFG merge that may select an external object is not an allocation definition"
    );
    assert_eq!(
        res.typed_slot_region(&func.blocks[&join].ops[0]),
        MemRegion::Field {
            allocation: None,
            offset: 0,
        }
    );
    assert_eq!(
        res.region_of(&func.blocks[&join].ops[0]),
        MemRegion::GenericHeap
    );
}

// ── may_alias matrix: Field vs every region ───────────────────────────

#[test]
fn field_may_alias_matrix() {
    let unknown0 = MemRegion::Field {
        allocation: None,
        offset: 0,
    };
    let unknown8 = MemRegion::Field {
        allocation: None,
        offset: 8,
    };
    let known0 = MemRegion::Field {
        allocation: Some(ValueId(1)),
        offset: 0,
    };
    assert!(unknown0.may_alias(&known0));
    assert!(!unknown0.may_alias(&unknown8));
    assert!(!known0.may_alias(&MemRegion::ContainerElement));
    assert!(!MemRegion::ContainerElement.may_alias(&known0));
    assert!(!known0.may_alias(&MemRegion::ModuleDict));
    assert!(!MemRegion::ModuleDict.may_alias(&known0));
    assert!(known0.may_alias(&MemRegion::GenericHeap));
    assert!(MemRegion::GenericHeap.may_alias(&known0));
    assert!(!known0.may_alias(&MemRegion::ScalarRegister));
    assert!(!known0.may_alias(&MemRegion::LocalAllocation { root: ValueId(9) }));
    assert!(unknown0.may_alias(&MemRegion::LocalAllocation { root: ValueId(9) }));
}

#[test]
fn fresh_result_ownership_is_not_inferred_from_operation_spelling() {
    for kind in [
        molt_ir::OpKind::VecSum,
        molt_ir::OpKind::VecProd,
        molt_ir::OpKind::VecMin,
        molt_ir::OpKind::VecMax,
    ] {
        assert_eq!(classify_copy_kind(Some(kind)), CopyLowering::OwnedValue);
    }
}

#[test]
fn runtime_copy_custody_preserves_borrowed_unpublished_and_move_results() {
    for kind in [
        molt_ir::OpKind::DictSet,
        molt_ir::OpKind::DictUpdateMissing,
        molt_ir::OpKind::FrameHomeLoad,
        molt_ir::OpKind::FrameHomeStore,
        molt_ir::OpKind::FunctionClosureBits,
        molt_ir::OpKind::AllocClass,
        molt_ir::OpKind::ConstEllipsis,
        molt_ir::OpKind::ConstNotImplemented,
        molt_ir::OpKind::Cast,
        molt_ir::OpKind::Widen,
    ] {
        assert_eq!(
            classify_copy_kind(Some(kind)),
            CopyLowering::TransparentAlias,
            "{kind}"
        );
        assert!(!copy_kind_mints_owned_value(kind), "{kind}");
    }
    for kind in [
        molt_ir::OpKind::DictGet,
        molt_ir::OpKind::ListIntNew,
        molt_ir::OpKind::GenSend,
        molt_ir::OpKind::CallAsync,
        molt_ir::OpKind::ListAppend,
    ] {
        assert_eq!(
            classify_copy_kind(Some(kind)),
            CopyLowering::OwnedValue,
            "{kind}"
        );
        assert!(!copy_kind_is_explicit_no_heap_move(Some(kind)), "{kind}");
    }
}

#[test]
fn trace_lifecycle_is_nonowning_but_observes_arbitrary_heap() {
    let res = empty_res();
    for kind in [molt_ir::OpKind::TraceEnterSlot, molt_ir::OpKind::TraceExit] {
        assert_eq!(
            classify_copy_kind(Some(kind)),
            CopyLowering::TransparentAlias
        );
        let mut trace = op_kind(OpCode::Copy, vec![], vec![], kind);
        if kind == molt_ir::OpKind::TraceEnterSlot {
            trace
                .attrs
                .insert("s_value".into(), AttrValue::Str("frame_owner".into()));
        }
        assert_eq!(res.region_of(&trace), MemRegion::GenericHeap);
        assert!(res.may_observe_slot(&trace, ValueId(99)));
        assert!(!crate::tir::op_kinds_generated::copy_kind_is_inert_marker_table(kind));
    }
}
