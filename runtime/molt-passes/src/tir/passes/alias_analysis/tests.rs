use super::*;
use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
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
        UnpackSequence,
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
        | UnpackSequence
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
        | ScfYield => {}
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

#[test]
fn unpack_sequence_results_are_independent_owned_roots() {
    let mut func = TirFunction::new("unpack".into(), vec![TirType::DynBox], TirType::None);
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
