//! Unit tests for the SROA pass. See the module docs for the soundness model.

use super::*;
use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::ops::{AttrDict, AttrValue, Dialect};
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

/// `obj = ObjectNewBoundStack(cls)` with payload size on the `value` attr (the
/// escape-pass / verifier contract for a stack object).
fn stack_alloc(cls: ValueId, result: ValueId, payload: i64) -> TirOp {
    let mut o = op(OpCode::ObjectNewBoundStack, vec![cls], vec![result]);
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

/// `obj.<offset> = val` typed-slot init store (`_original_kind = store_init`).
fn store_init(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
    let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("store_init".into()));
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
        .filter(|o| o.opcode == OpCode::ObjectNewBoundStack)
        .count()
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. The bench_struct pattern: construct-and-mutate, never observed.
// ─────────────────────────────────────────────────────────────────────────────

/// `obj = ObjectNewBoundStack; store_init(obj,c0,0); store_init(obj,c0,8);
///  store(obj,c0,0); store(obj,c1,8); return` — the object is never loaded or
/// escaped, every stored value is a fits-inline constant. SROA removes ALL
/// stores; the alloc is then dead (DCE removes it, not SROA).
#[test]
fn bench_struct_pattern_removes_all_stores() {
    let mut func = TirFunction::new("main".into(), vec![TirType::DynBox], TirType::None);
    let cls = ValueId(0); // class ref param
    let c0 = func.fresh_value();
    let c1 = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(0, c0));
        entry.ops.push(const_int(1, c1));
        entry.ops.push(stack_alloc(cls, obj, 40));
        entry.ops.push(store_init(obj, c0, 0));
        entry.ops.push(store_init(obj, c0, 8));
        entry.ops.push(store(obj, c0, 0));
        entry.ops.push(store(obj, c1, 8));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    assert_eq!(n_stores(&func), 4, "four stores before SROA");
    let stats = run_fresh(&mut func);
    assert_eq!(stats.ops_removed, 4, "all four stores removed");
    assert_eq!(n_stores(&func), 0, "no StoreAttr survives");
    // The alloc itself is left for DCE (it is now unreferenced and
    // ObjectNewBoundStack is not side-effecting).
    assert_eq!(
        n_allocs(&func),
        1,
        "SROA removes stores, DCE removes the alloc"
    );
}

/// Same pattern but the stored value is a function parameter typed `Bool` — an
/// always-immediate type. SROA fires (refcount-neutral by type, no range proof
/// needed).
#[test]
fn bool_typed_store_value_is_neutral() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::Bool],
        TirType::None,
    );
    let cls = ValueId(0);
    let b = ValueId(1); // Bool-typed param
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(stack_alloc(cls, obj, 32));
        entry.ops.push(store(obj, b, 0));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.ops_removed, 1, "Bool-typed store is removed");
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Blocked when the object is observed (a surviving load).
// ─────────────────────────────────────────────────────────────────────────────

/// `obj = ObjectNewBoundStack; store(obj,c,0); r = load(obj,0); return r` — the
/// surviving load observes the object, so SROA refuses (the residue is not
/// store-only; in production MemGVN would have forwarded this load first).
#[test]
fn blocked_when_object_has_surviving_load() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
    let cls = ValueId(0);
    let c = func.fresh_value();
    let obj = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(7, c));
        entry.ops.push(stack_alloc(cls, obj, 32));
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

/// `obj = ObjectNewBoundStack; store(obj,c,0); return obj` — the object escapes
/// via the return terminator. SROA refuses.
#[test]
fn blocked_when_object_escapes_via_return() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
    let cls = ValueId(0);
    let c = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(7, c));
        entry.ops.push(stack_alloc(cls, obj, 32));
        entry.ops.push(store(obj, c, 0));
        entry.terminator = Terminator::Return { values: vec![obj] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.ops_removed, 0, "an escaping object blocks SROA");
    assert_eq!(n_stores(&func), 1, "the store is preserved");
}

/// `obj = ObjectNewBoundStack; store(obj,c,0); call(obj); return` — passing the
/// object to an opaque call escapes/observes it. SROA refuses.
#[test]
fn blocked_when_object_passed_to_call() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let cls = ValueId(0);
    let c = func.fresh_value();
    let obj = func.fresh_value();
    let call_r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(7, c));
        entry.ops.push(stack_alloc(cls, obj, 32));
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

/// `obj = ObjectNewBoundStack; store(obj, x, 0); return` where `x` is an
/// `I64`-typed parameter with NO value-range proof — it may be a heap BigInt, so
/// removing the store could unbalance the slot's incref. SROA refuses.
#[test]
fn blocked_when_store_value_is_unproven_int() {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::I64],
        TirType::None,
    );
    let cls = ValueId(0);
    let x = ValueId(1); // I64 param, unbounded → MaybeBigInt
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(stack_alloc(cls, obj, 32));
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
    let cls = ValueId(0);
    let big = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(1 << 60, big));
        entry.ops.push(stack_alloc(cls, obj, 32));
        entry.ops.push(store(obj, big, 0));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.ops_removed, 0, "a BigInt const store blocks SROA");
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Blocked when one promotable object is stored into another (capture/escape).
// ─────────────────────────────────────────────────────────────────────────────

/// `a = ObjectNewBoundStack; b = ObjectNewBoundStack; store(a, b, 0); return` —
/// `b` is captured into `a`'s slot. Neither is promotable: `a`'s store value is
/// a candidate root (escape), and `b` is referenced as a store value (blocker).
#[test]
fn blocked_when_object_stored_into_another() {
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let cls = ValueId(0);
    let a = func.fresh_value();
    let b = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(stack_alloc(cls, a, 32));
        entry.ops.push(stack_alloc(cls, b, 32));
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
fn run_removes_stack_stores_without_ambient_disable_path() {
    let mut func = TirFunction::new("main".into(), vec![TirType::DynBox], TirType::None);
    let cls = ValueId(0);
    let c0 = func.fresh_value();
    let obj = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(0, c0));
        entry.ops.push(stack_alloc(cls, obj, 32));
        entry.ops.push(store(obj, c0, 0));
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);
    assert_eq!(
        stats.ops_removed, 1,
        "production SROA removes the dead store"
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
    let cls = ValueId(0);
    let c0 = func.fresh_value();
    let c1 = func.fresh_value();
    let obj = func.fresh_value();
    let b1 = func.fresh_block();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(0, c0));
        entry.ops.push(const_int(1, c1));
        entry.ops.push(stack_alloc(cls, obj, 40));
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
    assert_eq!(stats.ops_removed, 2, "both cross-block stores removed");
    assert_eq!(n_stores(&func), 0);
}
