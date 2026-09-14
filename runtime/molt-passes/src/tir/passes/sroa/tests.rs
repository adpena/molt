//! Unit tests for the SROA pass. See the module docs for the soundness model.

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::PassStats;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::run;

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

/// Owned raw boxed allocation with a statically known complete payload extent.
fn raw_alloc(result: ValueId, payload: i64) -> TirOp {
    let mut o = op(OpCode::Alloc, vec![], vec![result]);
    o.attrs.insert("value".into(), AttrValue::Int(payload));
    o
}

/// `obj.<offset> = val` typed-slot store (`_original_kind = store`).
fn store(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
    let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("store".into()));
    o
}

/// `r = obj.<offset>` proven-pure typed-slot load.
fn load(obj: ValueId, offset: i64, r: ValueId) -> TirOp {
    let mut o = op(OpCode::LoadAttr, vec![obj], vec![r]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("load".into()));
    o
}

/// `r = ConstInt(v)`.
fn const_int(v: i64, r: ValueId) -> TirOp {
    let mut o = op(OpCode::ConstInt, vec![], vec![r]);
    o.attrs.insert("value".into(), AttrValue::Int(v));
    o
}

fn run_fresh(func: &mut TirFunction) -> PassStats {
    let mut am = AnalysisManager::new();
    run(func, &mut am)
}

fn n_stores(func: &TirFunction) -> usize {
    func.blocks
        .values()
        .flat_map(|b| &b.ops)
        .filter(|o| o.opcode == OpCode::StoreAttr)
        .count()
}

fn n_allocs(func: &TirFunction) -> usize {
    func.blocks
        .values()
        .flat_map(|b| &b.ops)
        .filter(|o| o.opcode == OpCode::Alloc)
        .count()
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. The bench_struct pattern: construct-and-mutate, never observed.
// ─────────────────────────────────────────────────────────────────────────────

/// `obj = Alloc; store(obj,c0,0); store(obj,c0,8);
///  store(obj,c0,0); store(obj,c1,8); return` — the object is never loaded or
/// escaped, every stored value is a fits-inline constant. SROA removes ALL
/// stores and the complete allocation in the same rewrite.
#[test]
fn bench_struct_pattern_removes_all_stores() {
    let mut func = TirFunction::new("main".into(), vec![TirType::DynBox], TirType::None);

    let c0 = func.fresh_value();
    let c1 = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(0, c0));
        entry.ops.push(const_int(1, c1));
        entry.ops.push(raw_alloc(obj, 40));
        entry.ops.push(store(obj, c0, 0));
        entry.ops.push(store(obj, c0, 8));
        entry.ops.push(store(obj, c0, 0));
        entry.ops.push(store(obj, c1, 8));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    assert_eq!(n_stores(&func), 4, "four stores before SROA");
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.ops_removed, 5,
        "all four stores and allocation removed"
    );
    assert_eq!(n_stores(&func), 0, "no StoreAttr survives");
    assert_eq!(n_allocs(&func), 0, "SROA removes the complete allocation");
}

#[test]
fn scalar_replacement_erases_owned_candidate_as_one_lifetime_unit() {
    for observed in [false, true] {
        let mut func = TirFunction::new(
            "owned_candidate".into(),
            vec![TirType::DynBox],
            TirType::None,
        );
        let object = func.fresh_value();
        let copied = func.fresh_value();
        let scalar = func.fresh_value();
        let loaded = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.extend([
            const_int(1, scalar),
            raw_alloc(object, 16),
            op(OpCode::Copy, vec![object], vec![copied]),
            op(OpCode::IncRef, vec![copied], vec![]),
            store(copied, scalar, 0),
        ]);
        if observed {
            entry.ops.push(load(copied, 0, loaded));
        }
        entry.ops.push(op(OpCode::DecRef, vec![copied], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };
        let stats = run_fresh(&mut func);
        assert_eq!(stats.ops_removed, if observed { 0 } else { 5 });
        let ops = &func.blocks[&func.entry_block].ops;
        assert_eq!(ops.iter().any(|op| op.opcode == OpCode::Alloc), observed);
        assert_eq!(ops.iter().any(|op| op.opcode == OpCode::DecRef), observed);
        assert_eq!(ops.iter().any(|op| op.opcode == OpCode::IncRef), observed);
    }
}

#[test]
fn class_allocation_sealing_and_class_release_timing_are_observable() {
    let mut func = TirFunction::new(
        "class_lifetime".into(),
        vec![TirType::DynBox],
        TirType::None,
    );
    let class = func.fresh_value();
    let object = func.fresh_value();
    let value = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    // The class's defining owner is dropped before the observable call.
    // Its instance hold must keep class-dictionary finalizers alive until
    // the instance's drop. Construction itself may also seal class metadata.
    entry
        .ops
        .push(op(OpCode::Call, vec![ValueId(0)], vec![class]));
    let mut allocation = op(OpCode::ObjectNewBound, vec![class], vec![object]);
    allocation.attrs.insert("value".into(), AttrValue::Int(16));
    entry.ops.extend([
        const_int(1, value),
        allocation,
        store(object, value, 0),
        op(OpCode::DecRef, vec![class], vec![]),
        op(OpCode::Call, vec![ValueId(0)], vec![]),
        op(OpCode::DecRef, vec![object], vec![]),
    ]);
    entry.terminator = Terminator::Return { values: vec![] };
    let before = entry.ops.clone();
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    let after = &func.blocks[&func.entry_block].ops;
    assert_eq!(after.len(), before.len());
    for (after, before) in after.iter().zip(&before) {
        assert_eq!(after.opcode, before.opcode);
        assert_eq!(after.operands, before.operands);
        assert_eq!(after.results, before.results);
        assert_eq!(after.attrs, before.attrs);
    }
}

/// Same pattern but the stored value is an exact `ConstBool` producer. A Bool
/// annotation alone admits subclasses and is not a refcount-neutral proof.
#[test]
fn exact_bool_store_value_is_neutral() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);

    let b = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(op(OpCode::ConstBool, vec![], vec![b]));
        entry.ops.push(raw_alloc(obj, 32));
        entry.ops.push(store(obj, b, 0));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.ops_removed, 2,
        "Bool-typed store and allocation are removed"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Blocked when the object is observed (a surviving load).
// ─────────────────────────────────────────────────────────────────────────────

/// `obj = Alloc; store(obj,c,0); r = load(obj,0); return r` — the
/// surviving load observes the object, so SROA refuses (the residue is not
/// store-only; in production MemGVN would have forwarded this load first).
#[test]
fn blocked_when_object_has_surviving_load() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);

    let c = func.fresh_value();
    let obj = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(7, c));
        entry.ops.push(raw_alloc(obj, 32));
        entry.ops.push(store(obj, c, 0));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.ops_removed, 0, "a surviving load blocks SROA");
    assert_eq!(n_stores(&func), 1, "the store is preserved");
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Blocked when the object escapes (returned).
// ─────────────────────────────────────────────────────────────────────────────

/// `obj = Alloc; store(obj,c,0); return obj` — the object escapes
/// via the return terminator. SROA refuses.
#[test]
fn blocked_when_object_escapes_via_return() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);

    let c = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(7, c));
        entry.ops.push(raw_alloc(obj, 32));
        entry.ops.push(store(obj, c, 0));
        entry.terminator = Terminator::Return { values: vec![obj] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.ops_removed, 0, "an escaping object blocks SROA");
    assert_eq!(n_stores(&func), 1, "the store is preserved");
}

/// `obj = Alloc; store(obj,c,0); call(obj); return` — passing the
/// object to an opaque call escapes/observes it. SROA refuses.
#[test]
fn blocked_when_object_passed_to_call() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);

    let c = func.fresh_value();
    let obj = func.fresh_value();
    let call_r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(7, c));
        entry.ops.push(raw_alloc(obj, 32));
        entry.ops.push(store(obj, c, 0));
        entry.ops.push(op(OpCode::Call, vec![obj], vec![call_r]));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.ops_removed, 0,
        "passing the object to a call blocks SROA"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Blocked when a stored value is not provably refcount-neutral (BigInt/heap).
// ─────────────────────────────────────────────────────────────────────────────

/// `obj = Alloc; store(obj, x, 0); return` where `x` is an
/// `I64`-typed parameter with NO value-range proof — it may be a heap BigInt, so
/// removing the store could unbalance the slot's incref. SROA refuses.
#[test]
fn blocked_when_store_value_is_unproven_int() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::I64],
        TirType::None,
    );

    let x = ValueId(1); // I64 param, unbounded → MaybeBigInt
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(raw_alloc(obj, 32));
        entry.ops.push(store(obj, x, 0));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.ops_removed, 0,
        "an unproven (possibly-BigInt) int store blocks SROA"
    );
    assert_eq!(n_stores(&func), 1, "the store is preserved");
}

/// A `ConstInt(1 << 60)` is a heap BigInt literal (does not fit the inline
/// window). Storing it is NOT refcount-neutral. SROA refuses.
#[test]
fn blocked_when_store_value_is_bigint_const() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);

    let big = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(1 << 60, big));
        entry.ops.push(raw_alloc(obj, 32));
        entry.ops.push(store(obj, big, 0));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.ops_removed, 0, "a BigInt const store blocks SROA");
}

#[test]
fn range_only_impostor_does_not_prove_refcount_neutrality() {
    let mut func = TirFunction::new(
        "range_only".into(),
        vec![TirType::DynBox, TirType::I64],
        TirType::None,
    );
    let mask = func.fresh_value();
    let narrowed = func.fresh_value();
    let object = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int(1, mask));
    entry
        .ops
        .push(op(OpCode::BitAnd, vec![ValueId(1), mask], vec![narrowed]));
    entry.ops.push(raw_alloc(object, 32));
    entry.ops.push(store(object, narrowed, 0));
    entry.terminator = Terminator::Return { values: vec![] };

    let ranges = crate::representation_facts::value_range_for(&func);
    assert!(ranges.fits_inline_int47(narrowed));
    assert!(
        !crate::tir::type_refine::extract_exact_scalar_map(&func).contains_key(&narrowed),
        "a range derived from an annotation-admitting operand is not exact provenance"
    );
    assert!(
        !crate::representation_facts::non_heap_boxed_values_for(&func, &ranges).contains(&narrowed)
    );
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(n_stores(&func), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Blocked when one promotable object is stored into another (capture/escape).
// ─────────────────────────────────────────────────────────────────────────────

/// `a = Alloc; b = Alloc; store(a, b, 0); return` —
/// `b` is captured into `a`'s slot. Neither is promotable: `a`'s store value is
/// a candidate root (escape), and `b` is referenced as a store value (blocker).
#[test]
fn blocked_when_object_stored_into_another() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);

    let a = func.fresh_value();
    let b = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(raw_alloc(a, 32));
        entry.ops.push(raw_alloc(b, 32));
        entry.ops.push(store(a, b, 0));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.ops_removed, 0,
        "storing one stack object into another blocks both"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. Unconditional production path.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn run_removes_raw_boxed_stores_without_ambient_disable_path() {
    let mut func = TirFunction::new("main".into(), vec![TirType::DynBox], TirType::None);

    let c0 = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(0, c0));
        entry.ops.push(raw_alloc(obj, 32));
        entry.ops.push(store(obj, c0, 0));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);
    assert_eq!(
        stats.ops_removed, 2,
        "production SROA removes the dead object"
    );
    assert_eq!(n_stores(&func), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. Multi-block: a store in a successor block of the same non-escaping object.
// ─────────────────────────────────────────────────────────────────────────────

/// Stores split across two blocks, object never observed. SROA removes both.
#[test]
fn removes_stores_across_blocks() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);

    let c0 = func.fresh_value();
    let c1 = func.fresh_value();
    let obj = func.fresh_value();
    let b1 = func.fresh_block();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(0, c0));
        entry.ops.push(const_int(1, c1));
        entry.ops.push(raw_alloc(obj, 40));
        entry.ops.push(store(obj, c0, 0));
        entry.terminator = Terminator::Branch {
            target: b1,
            args: vec![],
        };
    }
    func.blocks.insert(
        b1,
        TirBlock {
            id: b1,
            args: vec![],
            ops: vec![store(obj, c1, 8)],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.ops_removed, 3,
        "both cross-block stores and allocation removed"
    );
    assert_eq!(n_stores(&func), 0);
}

#[test]
fn annotated_scalar_field_value_does_not_prove_refcount_neutrality() {
    for hint in [TirType::F64, TirType::I64] {
        let mut func = TirFunction::new(
            "annotated".into(),
            vec![TirType::DynBox, hint.clone()],
            TirType::None,
        );
        let object = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(raw_alloc(object, 16));
        entry.ops.push(store(object, ValueId(1), 0));
        entry.terminator = Terminator::Return { values: vec![] };
        assert_eq!(run_fresh(&mut func).ops_removed, 0, "{hint:?}");
        assert_eq!(n_stores(&func), 1);
    }
}

#[test]
fn field_removal_requires_shared_fixed_layout_extent() {
    // Raw storage has no dictionary tail; every aligned word inside its
    // requested extent is a field. Out-of-bounds accesses must not disappear.
    for (payload, offset, removable) in [
        (16, 0, true),
        (24, 8, true),
        (16, -1, false),
        (24, 1, false),
        (16, 8, true),
        (24, 16, true),
        (16, i64::MAX, false),
        (8, 0, true),
        (0, 0, false),
        (-8, 0, false),
    ] {
        let mut func = TirFunction::new("offset".into(), vec![TirType::DynBox], TirType::None);
        let constant = func.fresh_value();
        let object = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.extend([
            const_int(0, constant),
            raw_alloc(object, payload),
            store(object, constant, offset),
        ]);
        entry.terminator = Terminator::Return { values: vec![] };
        assert_eq!(
            run_fresh(&mut func).ops_removed,
            2 * usize::from(removable),
            "payload={payload}, offset={offset}"
        );
        assert_eq!(n_stores(&func), usize::from(!removable));
    }
}

#[test]
fn nonheap_overwrite_cannot_erase_prior_heap_slot_ownership() {
    for heap_first in [true, false] {
        let mut func =
            TirFunction::new("slot_lifetime".into(), vec![TirType::DynBox], TirType::None);

        let object = func.fresh_value();
        let heap_value = func.fresh_value();
        let scalar = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(raw_alloc(object, 24));
        let mut heap = op(OpCode::ConstBigInt, vec![], vec![heap_value]);
        heap.attrs.insert(
            "s_value".into(),
            AttrValue::Str("123456789012345678901234567890".into()),
        );
        entry.ops.push(heap);
        entry.ops.push(const_int(7, scalar));
        let values = if heap_first {
            [heap_value, scalar]
        } else {
            [scalar, heap_value]
        };
        for value in values {
            entry.ops.push(store(object, value, 0));
        }
        entry.terminator = Terminator::Return { values: vec![] };
        let stats = run_fresh(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(
            n_stores(&func),
            2,
            "the whole root must be refcount-neutral"
        );
    }
}

#[test]
fn raw_i64_carriers_that_box_to_bigint_retain_slot_ownership() {
    for value in [i64::MIN, -(1_i64 << 46) - 1, 1_i64 << 46, i64::MAX] {
        let mut func = TirFunction::new("boxed_slot".into(), vec![TirType::DynBox], TirType::None);
        let raw = func.fresh_value();
        let copied = func.fresh_value();
        let zero = func.fresh_value();
        let object = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.extend([
            const_int(value, raw),
            op(OpCode::Copy, vec![raw], vec![copied]),
            const_int(0, zero),
            raw_alloc(object, 24),
            store(object, copied, 0),
            store(object, zero, 0),
        ]);
        entry.terminator = Terminator::Return { values: vec![] };
        let ranges = crate::representation_facts::value_range_for(&func);
        let carrier = crate::representation_facts::non_heap_values_for(&func, &ranges);
        let boxed = crate::representation_facts::non_heap_boxed_values_for(&func, &ranges);
        for value in [raw, copied] {
            assert!(carrier.contains(&value), "the SSA carrier is non-owning");
            assert!(!boxed.contains(&value), "field boxing can allocate BigInt");
        }
        assert!(boxed.contains(&zero));
        assert_eq!(run_fresh(&mut func).ops_removed, 0);
        assert_eq!(n_stores(&func), 2);
    }
}

#[test]
fn finalizer_bearing_allocation_artifact_cannot_erase_fields() {
    let mut func = TirFunction::new("finalizer".into(), vec![TirType::DynBox], TirType::None);
    let value = func.fresh_value();
    let object = func.fresh_value();
    let mut allocation = raw_alloc(object, 24);
    allocation
        .attrs
        .insert("defines_del".into(), AttrValue::Bool(true));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_int(1, value), allocation, store(object, value, 0)]);
    entry.terminator = Terminator::Return { values: vec![] };
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(n_stores(&func), 1);
}
