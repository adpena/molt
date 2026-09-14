use std::collections::HashSet;

use super::super::alias_analysis::{AliasAnalysisResult, MemRegion};
use super::*;
use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

// -- builders -----------------------------------------------------------

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

fn const_int(value: i64, result: ValueId) -> TirOp {
    let mut result_op = op(OpCode::ConstInt, vec![], vec![result]);
    result_op
        .attrs
        .insert("value".into(), AttrValue::Int(value));
    result_op
}

fn const_str(value: &str, result: ValueId) -> TirOp {
    let mut result_op = op(OpCode::ConstStr, vec![], vec![result]);
    result_op
        .attrs
        .insert("value".into(), AttrValue::Str(value.into()));
    result_op
}

/// An ordinary typed-slot store `obj.<offset> = val` of class `Point`.
fn store(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
    store_of(obj, val, offset, "Point")
}

/// An ordinary typed-slot store with an explicit class name.
fn store_of(obj: ValueId, val: ValueId, offset: i64, class: &str) -> TirOp {
    let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("store".into()));
    o.attrs
        .insert("_class".into(), AttrValue::Str(class.into()));
    o
}

/// A typed-slot store with no class metadata; old-value facts are independent.
fn store_no_class(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
    let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("store".into()));
    o
}

/// A direct typed-slot load `r = obj.<offset>` of class `Point`. The exact-site
/// slot plan decides whether it is callback-free and becomes a MemoryUse.
fn load(obj: ValueId, offset: i64, r: ValueId) -> TirOp {
    load_of(obj, offset, r, "Point")
}

/// A typed-slot load with an explicit class name.
fn load_of(obj: ValueId, offset: i64, r: ValueId, class: &str) -> TirOp {
    let mut o = op(OpCode::LoadAttr, vec![obj], vec![r]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("load".into()));
    o.attrs
        .insert("_class".into(), AttrValue::Str(class.into()));
    o
}

/// A direct typed-slot load with no class metadata.
fn load_no_class(obj: ValueId, offset: i64, r: ValueId) -> TirOp {
    let mut o = op(OpCode::LoadAttr, vec![obj], vec![r]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("load".into()));
    o
}

/// An opaque call that clobbers `GenericHeap`.
fn call(args: Vec<ValueId>, r: ValueId) -> TirOp {
    op(OpCode::Call, args, vec![r])
}

fn allocation(result: ValueId, bytes: i64) -> TirOp {
    let mut result_op = op(OpCode::Alloc, vec![], vec![result]);
    result_op
        .attrs
        .insert("value".into(), AttrValue::Int(bytes));
    result_op
}

fn alias_of(func: &TirFunction) -> AliasAnalysisResult {
    // The alias analysis's `compute` is private; route through the public
    // S1 manager to obtain the same cached result a consumer would.
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::passes::alias_analysis::AliasAnalysis;
    let mut am = AnalysisManager::new();
    am.get::<AliasAnalysis>(func).clone()
}

fn run(func: &TirFunction) -> MemorySsaResult {
    let alias = alias_of(func);
    compute_standalone(func, &alias)
}

// ── Test 1: straight-line def-use forwarding ───────────────────────────

#[test]
fn single_block_store_then_load_has_direct_reaching_def() {
    // entry: obj = alloc(8); store(obj, val, 0); r = load(obj, 0); return r
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(allocation(obj, 8));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let mem = run(&func);
    let store_ver = mem
        .def_at(func.entry_block, 2)
        .expect("store defines a version");
    let load_reaching = mem
        .reaching_def_for_use(func.entry_block, 3)
        .expect("load is a tracked use");
    assert_eq!(
        load_reaching, store_ver,
        "the load must read exactly the dominating store's version"
    );
    assert!(mem.is_direct_def_of_use(store_ver, func.entry_block, 3));
}

// ── CheckException is not a clobber ─────────────────────────────────────

#[test]
fn check_exception_between_store_and_load_does_not_clobber() {
    // alloc(8); store(obj, val, 0); check_exception; r = load(obj, 0)
    //
    // `CheckException` reads the pending-exception flag — it never writes
    // heap memory (its handler-edge control flow is modeled by the CFG, and
    // `may_observe_slot` is false for it). It must NOT bump the memory
    // version between the store and the load: it is emitted after nearly
    // every op in exception-bearing bodies, so classifying it as a
    // GenericHeap def starves store-to-load forwarding function-wide.
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(allocation(obj, 8));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(op(OpCode::CheckException, vec![], vec![]));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let mem = run(&func);
    let store_ver = mem
        .def_at(func.entry_block, 2)
        .expect("store defines a version");
    assert!(
        mem.def_at(func.entry_block, 3).is_none(),
        "CheckException must not be a MemoryDef"
    );
    let reaching = mem
        .reaching_def_for_use(func.entry_block, 4)
        .expect("load is a tracked use");
    assert_eq!(
        reaching, store_ver,
        "the load must still read the store's version across CheckException"
    );
    assert!(mem.is_direct_def_of_use(store_ver, func.entry_block, 4));
}

// ── AnalysisManager registration ────────────────────────────────────────

#[test]
fn analysis_manager_registration_matches_compute_standalone() {
    // The S1 manager path (`am.get::<MemorySSA>`) must yield exactly the
    // result `compute_standalone` produces over the alias substrate.
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(allocation(obj, 8));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let direct = run(&func);
    use crate::tir::analysis::AnalysisManager;
    let mut am = AnalysisManager::new();
    let via_manager = am.get::<MemorySSA>(&func);
    assert_eq!(via_manager.next_version, direct.next_version);
    assert_eq!(
        via_manager.def_at(func.entry_block, 2),
        direct.def_at(func.entry_block, 2),
    );
    assert_eq!(
        via_manager.reaching_def_for_use(func.entry_block, 3),
        direct.reaching_def_for_use(func.entry_block, 3),
    );
}

// ── Test 2: store-store kill (last store dominates the read) ───────────

#[test]
fn store_store_kills_earlier_version_for_load() {
    // alloc(8); store(obj, v1, 0); store(obj, v2, 0); r = load(obj, 0)
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let v1 = func.fresh_value();
    let v2 = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(allocation(obj, 8));
        entry.ops.push(const_int(1, v1));
        entry.ops.push(store(obj, v1, 0));
        entry.ops.push(const_str("second", v2));
        entry.ops.push(store(obj, v2, 0));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let mem = run(&func);
    let first = mem.def_at(func.entry_block, 2).unwrap();
    let second = mem.def_at(func.entry_block, 4).unwrap();
    let reaching = mem.reaching_def_for_use(func.entry_block, 5).unwrap();
    assert_eq!(
        reaching, second,
        "load reads the SECOND store (it kills the first)"
    );
    assert_ne!(
        reaching, first,
        "the first store is killed by the overwrite"
    );
    // The second def flows through the first (the clobber chain is intact).
    assert_eq!(mem.def_version_of(second), Some(first));
}

// ── Test 3: may_alias-blocked forwarding (distinct offsets) ────────────

#[test]
fn distinct_offsets_have_independent_reaching_defs() {
    // Fresh boxed storage proves both initial stores release-neutral. Local
    // fields retain byte offsets, so store@8 does not clobber load@0.
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let v1 = func.fresh_value();
    let v2 = func.fresh_value();
    let r0 = func.fresh_value();
    let r8 = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(allocation(obj, 16));
        entry.ops.push(const_str("left", v1));
        entry.ops.push(store(obj, v1, 0));
        entry.ops.push(const_str("right", v2));
        entry.ops.push(store(obj, v2, 8));
        entry.ops.push(load(obj, 0, r0));
        entry.ops.push(load(obj, 8, r8));
        entry.terminator = Terminator::Return {
            values: vec![r0, r8],
        };
    }
    let mem = run(&func);
    let store0 = mem.def_at(func.entry_block, 2).unwrap();
    let store8 = mem.def_at(func.entry_block, 4).unwrap();
    let load0_reaching = mem.reaching_def_for_use(func.entry_block, 5).unwrap();
    let load8_reaching = mem.reaching_def_for_use(func.entry_block, 6).unwrap();
    // Offset disambiguation: each load reaches the store of ITS offset.
    assert_eq!(
        load0_reaching, store0,
        "load@0 reaches store@0 (store@8 is a disjoint field)"
    );
    assert_eq!(load8_reaching, store8, "load@8 reaches store@8");
    // The clobber chain is still store0 ← store8 (store@8 flows through
    // store@0 — they are ordered defs, just disjoint regions).
    assert_eq!(mem.def_version_of(store8), Some(store0));
    // Forwarding is now unblocked for BOTH loads.
    assert!(mem.is_direct_def_of_use(store0, func.entry_block, 5));
    assert!(mem.is_direct_def_of_use(store8, func.entry_block, 6));
}

#[test]
fn unproved_replacing_store_clobbers_a_distinct_class() {
    // Byte/class disjointness is not callback disjointness: replacement can
    // invoke an old value's destructor that mutates the other class instance.
    let mut func = TirFunction::new(
        "f".into(),
        vec![
            TirType::DynBox,
            TirType::DynBox,
            TirType::DynBox,
            TirType::DynBox,
        ],
        TirType::DynBox,
    );
    let p = ValueId(0);
    let l = func.fresh_value();
    let v1 = ValueId(2);
    let v2 = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut construct = op(OpCode::ObjectNewBound, vec![ValueId(1)], vec![l]);
        construct.attrs.insert("value".into(), AttrValue::Int(16));
        entry.ops.push(construct);
        entry.ops.push(const_str("present", v2));
        entry.ops.push(store_of(l, v2, 0, "Line"));
        entry.ops.push(store_of(p, v1, 0, "Point"));
        entry.ops.push(load_of(l, 0, r, "Line"));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let mem = run(&func);
    let replacing = mem.def_at(func.entry_block, 3).unwrap();
    let fallback_load = mem.def_at(func.entry_block, 4).unwrap();
    assert_eq!(
        mem.def_version_of(fallback_load),
        Some(replacing),
        "an unrelated replacing Point store can run a finalizer that rewrites Line"
    );
    assert!(mem.reaching_def_for_use(func.entry_block, 4).is_none());
}

#[test]
fn distinct_fresh_roots_at_same_offset_do_not_clobber() {
    let mut func = TirFunction::new("fresh_roots".into(), vec![], TirType::DynBox);
    let p = func.fresh_value();
    let q = func.fresh_value();
    let pv = func.fresh_value();
    let qv = func.fresh_value();
    let r = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops = vec![
        allocation(p, 8),
        allocation(q, 8),
        const_str("p", pv),
        store_no_class(p, pv, 0),
        const_str("q", qv),
        store_no_class(q, qv, 0),
        load_no_class(p, 0, r),
    ];
    entry.terminator = Terminator::Return { values: vec![r] };
    let mem = run(&func);
    assert!(mem.slot_access.stores.contains_key(&(func.entry_block, 3)));
    assert!(mem.slot_access.stores.contains_key(&(func.entry_block, 5)));
    assert_eq!(
        mem.reaching_def_for_use(func.entry_block, 6),
        mem.def_at(func.entry_block, 3)
    );
}

#[test]
fn distinct_offset_replacement_keeps_its_finalizer_clobber() {
    let mut func = TirFunction::new(
        "releasing_offset".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let object = func.fresh_value();
    let r = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops = vec![
        allocation(object, 16),
        store(object, ValueId(0), 0),
        store(object, ValueId(1), 8),
        store(object, ValueId(0), 8),
        load(object, 0, r),
    ];
    entry.terminator = Terminator::Return { values: vec![r] };
    let mem = run(&func);
    assert!(!mem.slot_access.stores.contains_key(&(func.entry_block, 3)));
    let fallback_load = mem.def_at(func.entry_block, 4).unwrap();
    assert_eq!(
        mem.def_version_of(fallback_load),
        mem.def_at(func.entry_block, 3),
        "replacing unknown boxed old value at offset8 can rewrite offset0"
    );
    assert!(mem.reaching_def_for_use(func.entry_block, 4).is_none());
}

#[test]
fn same_class_offset_store_still_clobbers() {
    // A same-class same-offset store BETWEEN a store and a load IS a clobber:
    // object identity is untracked, so two `Point.x@0` accesses may-alias.
    let mut func = TirFunction::new(
        "f".into(),
        vec![
            TirType::DynBox,
            TirType::DynBox,
            TirType::DynBox,
            TirType::DynBox,
        ],
        TirType::DynBox,
    );
    let a = ValueId(0);
    let b = ValueId(1); // possibly the same Point as `a` at runtime
    let v1 = ValueId(2);
    let v2 = ValueId(3);
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(store_of(a, v1, 0, "Point")); // op 0
        entry.ops.push(store_of(b, v2, 0, "Point")); // op 1 — same class+offset
        entry.ops.push(load_of(a, 0, r, "Point")); // op 2
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let mem = run(&func);
    let store_b = mem.def_at(func.entry_block, 1).unwrap();
    let fallback_load = mem.def_at(func.entry_block, 2).unwrap();
    assert_eq!(
        mem.def_version_of(fallback_load),
        Some(store_b),
        "a same-class+offset store on a possibly-different object still clobbers"
    );
    assert!(mem.reaching_def_for_use(func.entry_block, 2).is_none());
}

#[test]
fn classless_replacing_store_remains_a_generic_heap_clobber() {
    // Old-value release, not missing class metadata, makes the offset8 store
    // clobber an offset0 load. The receiver is an unproved argument.
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let obj = ValueId(0);
    let v1 = ValueId(1);
    let v2 = ValueId(2);
    let r0 = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(store_no_class(obj, v1, 0)); // op 0 — GenericHeap
        entry.ops.push(store_no_class(obj, v2, 8)); // op 1 — GenericHeap
        entry.ops.push(load_no_class(obj, 0, r0)); // op 2 — physical Field
        entry.terminator = Terminator::Return { values: vec![r0] };
    }
    let mem = run(&func);
    let store8 = mem.def_at(func.entry_block, 1).unwrap();
    let fallback_load = mem.def_at(func.entry_block, 2).unwrap();
    assert_eq!(
        mem.def_version_of(fallback_load),
        Some(store8),
        "fail-closed: a class-less typed-slot load reaches the most-recent GenericHeap store"
    );
    assert!(mem.reaching_def_for_use(func.entry_block, 2).is_none());
}

// ── Test 4: cross-block phi placement at a diamond join ────────────────

#[test]
fn unknown_value_store_does_not_prove_inline_load_presence() {
    let mut func = TirFunction::new(
        "unknown_stored_value".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let unknown = ValueId(0);
    let object = func.fresh_value();
    let loaded = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops = vec![
        allocation(object, 8),
        store_no_class(object, unknown, 0),
        load_no_class(object, 0, loaded),
    ];
    entry.terminator = Terminator::Return {
        values: vec![loaded],
    };

    let mem = run(&func);
    assert!(mem.slot_access.stores.contains_key(&(func.entry_block, 1)));
    assert!(
        !mem.slot_access.loads.contains(&(func.entry_block, 2)),
        "an unknown SSA value may be the internal missing marker"
    );
    assert!(mem.def_at(func.entry_block, 2).is_some());
    assert!(mem.reaching_def_for_use(func.entry_block, 2).is_none());
}

#[test]
fn phi_placed_at_join_of_two_stores() {
    // bb0 -> {bb1: store(obj,v1,0), bb2: store(obj,v2,0)} -> bb3: r = load(obj,0)
    let mut func = TirFunction::new(
        "f".into(),
        vec![
            TirType::DynBox,
            TirType::DynBox,
            TirType::DynBox,
            TirType::Bool,
        ],
        TirType::DynBox,
    );
    let obj = ValueId(0);
    let v1 = ValueId(1);
    let v2 = ValueId(2);
    let cond = ValueId(3);
    let bb1 = func.fresh_block();
    let bb2 = func.fresh_block();
    let bb3 = func.fresh_block();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond,
            then_block: bb1,
            then_args: vec![],
            else_block: bb2,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        bb1,
        TirBlock {
            id: bb1,
            args: vec![],
            ops: vec![store(obj, v1, 0)],
            terminator: Terminator::Branch {
                target: bb3,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        bb2,
        TirBlock {
            id: bb2,
            args: vec![],
            ops: vec![store(obj, v2, 0)],
            terminator: Terminator::Branch {
                target: bb3,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        bb3,
        TirBlock {
            id: bb3,
            args: vec![],
            ops: vec![load(obj, 0, r)],
            terminator: Terminator::Return { values: vec![r] },
        },
    );
    let mem = run(&func);
    let phi = mem
        .block_phis
        .get(&bb3)
        .copied()
        .expect("a memory phi at the join");
    let fallback_load = mem.def_at(bb3, 0).unwrap();
    assert_eq!(
        mem.def_version_of(fallback_load),
        Some(phi),
        "the load reads the join phi, not either branch store"
    );
    assert!(mem.reaching_def_for_use(bb3, 0).is_none());
    // The phi has two incomings, one per branch, each that branch's store.
    match mem.access(phi) {
        Some(MemAccess::Phi { incoming, .. }) => {
            assert_eq!(incoming.len(), 2, "phi merges both predecessor edges");
            let store1 = mem.def_at(bb1, 0).unwrap();
            let store2 = mem.def_at(bb2, 0).unwrap();
            let versions: HashSet<MemVersion> = incoming.iter().map(|(_, v)| *v).collect();
            assert!(versions.contains(&store1), "incoming includes bb1's store");
            assert!(versions.contains(&store2), "incoming includes bb2's store");
        }
        other => panic!("expected a Phi access, got {other:?}"),
    }
    // The forwarding query must NOT claim a single direct def (it is a phi).
    let store1 = mem.def_at(bb1, 0).unwrap();
    assert!(
        !mem.is_direct_def_of_use(store1, bb3, 0),
        "a phi-merged load has no single direct store def — forwarding must be blocked"
    );
}

// ── Test 5: call-barrier via the alias-oracle region classification ────

#[test]
fn generic_heap_call_kills_typed_field_load_reaching_def() {
    // bb0: store(obj, v1, 0); call(obj); r = load(obj, 0)
    // The call is a GenericHeap def (the alias oracle widens Call), which
    // may_alias-es the typed-slot load — so the load reaches the CALL's
    // version, not the store's. Forwarding is correctly blocked.
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let obj = ValueId(0);
    let v1 = ValueId(1);
    let call_r = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(store(obj, v1, 0)); // op 0
        entry.ops.push(call(vec![obj], call_r)); // op 1 — GenericHeap def
        entry.ops.push(load(obj, 0, r)); // op 2
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let mem = run(&func);
    let store_ver = mem.def_at(func.entry_block, 0).unwrap();
    let call_ver = mem
        .def_at(func.entry_block, 1)
        .expect("the call is a memory def");
    let fallback_load = mem.def_at(func.entry_block, 2).unwrap();
    assert_eq!(
        mem.def_version_of(fallback_load),
        Some(call_ver),
        "load reaches the clobbering call, not the store"
    );
    assert_ne!(
        mem.def_version_of(fallback_load),
        Some(store_ver),
        "the call kills the store's reaching-def relationship"
    );
    assert!(
        !mem.is_direct_def_of_use(store_ver, func.entry_block, 2),
        "store-to-load forwarding across a call barrier must be blocked"
    );
}

// ── Callback effects clobber unrelated typed fields ─────────────────────

#[test]
fn callback_effects_without_root_operands_are_memory_defs() {
    for opcode in [
        OpCode::Add,
        OpCode::Index,
        OpCode::ModuleGetAttr,
        OpCode::ModuleGetName,
        OpCode::ModuleImportFrom,
    ] {
        let mut func = TirFunction::new(
            format!("callback_{opcode:?}"),
            vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let obj = ValueId(0);
        let val = ValueId(1);
        let unrelated = ValueId(2);
        let callback_result = func.fresh_value();
        let loaded = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(op(
                opcode,
                vec![unrelated, unrelated],
                vec![callback_result],
            ));
            entry.ops.push(load(obj, 0, loaded));
            entry.terminator = Terminator::Return {
                values: vec![loaded],
            };
        }
        let alias = alias_of(&func);
        assert_eq!(
            alias.region_of(&func.blocks[&func.entry_block].ops[1]),
            MemRegion::GenericHeap,
            "{opcode:?}"
        );
        let mem = compute_standalone(&func, &alias);
        let store_ver = mem.def_at(func.entry_block, 0).unwrap();
        let callback_ver = mem
            .def_at(func.entry_block, 1)
            .expect("callback-capable op is a MemoryDef");
        let fallback_load = mem.def_at(func.entry_block, 2).unwrap();
        assert_eq!(
            mem.def_version_of(fallback_load),
            Some(callback_ver),
            "{opcode:?}"
        );
        assert_ne!(
            mem.def_version_of(fallback_load),
            Some(store_ver),
            "{opcode:?}"
        );
        assert!(mem.reaching_def_for_use(func.entry_block, 2).is_none());
    }
}

#[test]
fn exact_integer_add_does_not_clobber_typed_field() {
    let mut func = TirFunction::new("exact_add".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let left = func.fresh_value();
    let right = func.fresh_value();
    let sum = func.fresh_value();
    let loaded = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(allocation(obj, 8));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(const_int(4, left));
        entry.ops.push(const_int(5, right));
        entry
            .ops
            .push(op(OpCode::Add, vec![left, right], vec![sum]));
        entry.ops.push(load(obj, 0, loaded));
        entry.terminator = Terminator::Return {
            values: vec![loaded],
        };
    }
    let alias = alias_of(&func);
    assert_eq!(
        alias.region_of(&func.blocks[&func.entry_block].ops[5]),
        MemRegion::ScalarRegister
    );
    let mem = compute_standalone(&func, &alias);
    let store_ver = mem.def_at(func.entry_block, 2).unwrap();
    assert!(mem.def_at(func.entry_block, 5).is_none());
    assert_eq!(
        mem.reaching_def_for_use(func.entry_block, 6),
        Some(store_ver)
    );
}

#[test]
fn marked_async_work_check_exception_clobbers_memory() {
    let mut func = TirFunction::new(
        "async_poll".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let loaded = func.fresh_value();
    let mut poll = op(OpCode::CheckException, vec![], vec![]);
    assert!(poll.mark_async_work_poll());
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(store(ValueId(0), ValueId(1), 0));
        entry.ops.push(poll);
        entry.ops.push(load(ValueId(0), 0, loaded));
        entry.terminator = Terminator::Return {
            values: vec![loaded],
        };
    }
    let mem = run(&func);
    let poll_ver = mem
        .def_at(func.entry_block, 1)
        .expect("marked async-work poll is a MemoryDef");
    let fallback_load = mem.def_at(func.entry_block, 2).unwrap();
    assert_eq!(mem.def_version_of(fallback_load), Some(poll_ver));
    assert!(mem.reaching_def_for_use(func.entry_block, 2).is_none());
}

// ── Test 7: loop back-edge phi placement ───────────────────────────────

#[test]
fn loop_back_edge_places_memory_phi_at_header() {
    // preheader(entry) -> header; header: store(obj, v, 0) then cond back to
    // header or exit. The store on the back edge forces a memory phi at the
    // header (its own def reaches it on the back edge).
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::Bool],
        TirType::None,
    );
    let obj = ValueId(0);
    let v = ValueId(1);
    let cond = ValueId(2);
    let header = func.fresh_block();
    let exit = func.fresh_block();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![store(obj, v, 0)],
            terminator: Terminator::CondBranch {
                cond,
                then_block: header,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);

    let mem = run(&func);
    // The header is in the dominance frontier of itself (back-edge join),
    // so a memory phi must be placed there.
    let phi = mem.block_phis.get(&header).copied();
    assert!(
        phi.is_some(),
        "a back-edge loop header must receive a memory phi"
    );
    let phi = phi.unwrap();
    match mem.access(phi) {
        Some(MemAccess::Phi { incoming, .. }) => {
            // Two incoming edges: the preheader (entry) and the back edge
            // (header's own exit version).
            assert_eq!(incoming.len(), 2, "header phi merges preheader + back edge");
            let header_store = mem.def_at(header, 0).unwrap();
            let from_back: Vec<MemVersion> = incoming
                .iter()
                .filter(|(b, _)| *b == header)
                .map(|(_, v)| *v)
                .collect();
            assert_eq!(
                from_back,
                vec![header_store],
                "the back edge carries the header store's version into the phi"
            );
            let from_pre: Vec<MemVersion> = incoming
                .iter()
                .filter(|(b, _)| *b == func.entry_block)
                .map(|(_, v)| *v)
                .collect();
            assert_eq!(
                from_pre,
                vec![LIVE_ON_ENTRY],
                "the preheader carries live-on-entry into the phi"
            );
        }
        other => panic!("expected a header Phi, got {other:?}"),
    }
}

// ── Structural invariants ──────────────────────────────────────────────

#[test]
fn empty_function_has_no_memory_accesses() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    let mem = run(&func);
    assert!(mem.defs.is_empty(), "no memory ops ⇒ no Def/Phi nodes");
    assert!(mem.block_op_to_def.is_empty());
    assert!(mem.block_op_to_use_def.is_empty());
    assert!(mem.block_phis.is_empty());
    // exit_def for entry is LIVE_ON_ENTRY.
    assert_eq!(mem.exit_def.get(&func.entry_block), Some(&LIVE_ON_ENTRY));
    assert_eq!(mem.next_version, 1, "only LIVE_ON_ENTRY consumed");
}

#[test]
fn typed_slot_store_value_extracts_target_value_offset() {
    let s = store(ValueId(3), ValueId(7), 8);
    // A non-store op yields None.
    let l = load(ValueId(3), 8, ValueId(9));
    assert_eq!(typed_slot_store_value(&l), None);

    assert_eq!(
        typed_slot_store_value(&s),
        Some((ValueId(3), ValueId(7), 8)),
        "the structural projection does not assert release neutrality"
    );
}

#[test]
fn use_node_carries_region_and_reaching_def() {
    // store(obj, val, 0); r = load(obj, 0) — the `uses` map records the load
    // as a full `Use` node carrying its physical field region and
    // the reaching def. This pins the `MemAccess::Use` fields as load-bearing
    // for the S5-2b MemGVN consumer.
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(allocation(obj, 8));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let mem = run(&func);
    let store_ver = mem.def_at(func.entry_block, 2).unwrap();
    let use_node = mem
        .uses
        .get(&(func.entry_block, 3))
        .expect("the load is recorded as a Use node");
    match use_node {
        MemAccess::Use {
            def_ver,
            block,
            op_idx,
            region,
        } => {
            assert_eq!(*def_ver, store_ver, "Use reads the store's version");
            assert_eq!(*block, func.entry_block);
            assert_eq!(*op_idx, 3);
            // The admitted load carries its exact allocation identity and offset.
            assert_eq!(
                *region,
                MemRegion::Field {
                    allocation: Some(obj),
                    offset: 0
                }
            );
        }
        other => panic!("expected a Use node, got {other:?}"),
    }
    // The Use node's accessor helpers agree.
    assert_eq!(use_node.defined_version(), None, "a Use defines no version");
    assert_eq!(use_node.block(), func.entry_block);
}
