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
    let mut load = op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], "load");
    load.attrs.insert("value".into(), AttrValue::Int(0));
    assert!(!load.can_carry_async_work_poll());
    assert_eq!(load.plain_typed_slot_load(), Some((ValueId(0), 0)));

    let mut store = op_kind(
        OpCode::StoreAttr,
        vec![ValueId(0), ValueId(2)],
        vec![],
        "store",
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
    store
        .attrs
        .insert("_original_kind".into(), AttrValue::Str("store".into()));
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
fn default_plan_keeps_plain_and_guarded_loads_may_dispatch() {
    let plan = super::super::typed_slot_access::TypedSlotAccessPlan::default();
    let res = empty_res();
    let load = with_field_attrs(
        op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], "load"),
        8,
        Some("Point"),
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
            vec![ValueId(0), ValueId(2), ValueId(3)],
            vec![ValueId(1)],
            "guarded_field_get",
        ),
        8,
        Some("Point"),
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
        "get_attr",
        "get_attr_name",
        "get_attr_generic_ptr",
        "get_attr_generic_obj",
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
    // classify OwnedValue (incl. the review's double-free root `slice` and the
    // generator-iterator `iter`) AND must NOT be allowed to reach the benign
    // no-incref passthrough.
    let fresh = [
        Some("builtin_func"),
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
        Some("get_attr_name_default"),
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
        // range()'s bound conversion mints an owned exact int.
        Some("operator_index"),
        // Fused-loop kernels: each returns a fresh owned result tuple.
        Some("vec_sum"),
        Some("vec_prod"),
        Some("vec_min"),
        Some("vec_max"),
        Some("string_split_ws_dict_inc"),
        Some("string_split_sep_dict_inc"),
        Some("dict_str_int_inc"),
    ];
    let owned_alias = [Some("binding_alias")];
    // FAIL-CLOSED: an unrecognized future kind classifies as TransparentAlias
    // (leak-safe), NOT OwnedValue — so the drop pass never double-frees it.
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
            op_kind(OpCode::StoreAttr, vec![root, ValueId(1)], vec![], "store"),
            0,
            None,
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
    let named_constructor = op_kind(OpCode::Copy, vec![ValueId(0)], vec![root], "alloc_class");
    assert_eq!(res.region_of(&named_constructor), MemRegion::GenericHeap);
    let mut poll = op(OpCode::CheckException, vec![], vec![]);
    assert!(poll.mark_async_work_poll());
    assert_eq!(res.region_of(&poll), MemRegion::GenericHeap);
}

// ── region_of: direct physical field regions ─────────────────────

/// Set the physical offset and optional frontend class metadata on a field op.
fn with_field_attrs(mut o: TirOp, offset: i64, class: Option<&str>) -> TirOp {
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    if let Some(c) = class {
        o.attrs.insert("_class".into(), AttrValue::Str(c.into()));
    }
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
    // `_class` documents frontend provenance but does not admit or identify the
    // physical field region.
    let load = with_field_attrs(
        op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], "load"),
        8,
        Some("Point"),
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
            "store",
        ),
        16,
        Some("Line"),
    );
    // Store release behavior is site-contextual, so only the retained typed
    // store plan may refine it beyond GenericHeap.
    assert_eq!(res.region_of(&store), MemRegion::GenericHeap);
}

#[test]
fn base_and_derived_metadata_cannot_disambiguate_one_physical_field() {
    let res = empty_res();
    let base_load = with_field_attrs(
        op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], "load"),
        24,
        Some("Base"),
    );
    let derived_load = with_field_attrs(
        op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(2)], "load"),
        24,
        Some("Derived"),
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
        "inherited Base/Derived views can name the same boxed word"
    );
    assert_eq!(res.region_of(&base_load), MemRegion::GenericHeap);
    assert_eq!(res.region_of(&derived_load), MemRegion::GenericHeap);
}

/// A `Copy` is classified by whether it touches heap memory: a pure SSA
/// move (no `_original_kind`) and the inert debug / source-location / guard
/// markers are `ScalarRegister`; an opaque passthrough carrier stays the
/// conservative `GenericHeap`. Lifecycle trace operations are effectful:
/// entry acquires public namespaces and exit releases frame-owned references.
/// They invalidate memory and exact-slot history despite non-owning results.
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
fn guarded_field_get_3operand_abi_falls_back_to_generic_heap() {
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
    assert_eq!(res.region_of(&get), MemRegion::GenericHeap);
    assert_eq!(
        super::super::typed_slot_access::TypedSlotAccessPlan::default()
            .load_purity_at((BlockId(0), 0), &get),
        LoadPurity::MayDispatch
    );
}

#[test]
fn guarded_field_set_4operand_abi_falls_back_to_generic_heap() {
    // `guarded_field_set` ABI: operands [obj, class_bits, expected_version,
    // val], offset in `value`, class in `_class`. obj is operand[0].
    let res = empty_res();
    let set = with_field_attrs(
        op_kind(
            OpCode::StoreAttr,
            vec![ValueId(0), ValueId(1), ValueId(2), ValueId(3)],
            vec![],
            "guarded_field_set",
        ),
        32,
        Some("Account"),
    );
    assert_eq!(res.region_of(&set), MemRegion::GenericHeap);

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
fn direct_field_without_class_attr_uses_unknown_allocation_identity() {
    // Exact load shape and offset admit the field. Missing `_class` metadata
    // does not erase its physical footprint or mint object identity. Ordinary
    // stores remain GenericHeap without site-specific release-neutral proof.
    let res = empty_res();
    let load = with_field_attrs(
        op_kind(OpCode::LoadAttr, vec![ValueId(0)], vec![ValueId(1)], "load"),
        8,
        None,
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
fn exact_allocation_field_keeps_identity_independent_of_class_and_escape() {
    // Exact allocation-site identity, not `_class` or escape placement, admits
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
        op_kind(OpCode::LoadAttr, vec![root], vec![ValueId(1)], "load"),
        8,
        None,
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
        op_kind(OpCode::LoadAttr, vec![root], vec![ValueId(1)], "load"),
        8,
        Some("Point"),
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
                op_kind(OpCode::LoadAttr, vec![parameter], vec![loaded], "load"),
                0,
                None,
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
    for kind in ["vec_unknown", "vec_sum_i64", "vec_sum_extension"] {
        assert_eq!(
            classify_copy_kind(Some(kind)),
            CopyLowering::TransparentAlias
        );
    }
    for kind in ["vec_sum", "vec_prod", "vec_min", "vec_max"] {
        assert_eq!(classify_copy_kind(Some(kind)), CopyLowering::OwnedValue);
    }
}

#[test]
fn runtime_copy_custody_preserves_borrowed_unpublished_and_move_results() {
    for kind in [
        "dict_set",
        "dict_update_missing",
        "frame_home_load",
        "frame_home_store",
        "function_closure_bits",
        "alloc_class",
        "const_ellipsis",
        "const_not_implemented",
        "cast",
        "widen",
    ] {
        assert_eq!(
            classify_copy_kind(Some(kind)),
            CopyLowering::TransparentAlias,
            "{kind}"
        );
        assert!(!copy_kind_mints_owned_value(kind), "{kind}");
    }
    for kind in [
        "dict_get",
        "class_new",
        "list_int_new",
        "gen_send",
        "json_parse",
        "call_async",
        "list_append",
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
    for kind in ["trace_enter_slot", "trace_exit"] {
        assert_eq!(
            classify_copy_kind(Some(kind)),
            CopyLowering::TransparentAlias
        );
        let trace = op_kind(OpCode::Copy, vec![], vec![], kind);
        assert_eq!(res.region_of(&trace), MemRegion::GenericHeap);
        assert!(res.may_observe_slot(&trace, ValueId(99)));
        assert!(!crate::tir::op_kinds_generated::copy_kind_is_inert_marker_table(kind));
    }
}
