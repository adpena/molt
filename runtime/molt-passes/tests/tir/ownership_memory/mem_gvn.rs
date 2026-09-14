use molt_passes::tir::analysis::AnalysisManager;
use molt_passes::tir::blocks::{Terminator, TirBlock};
use molt_passes::tir::function::TirFunction;
use molt_passes::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use molt_passes::tir::passes::PassStats;
use molt_passes::tir::passes::alias_analysis::AliasAnalysisResult;
use molt_passes::tir::passes::mem_gvn::run;
use molt_passes::tir::types::TirType;
use molt_passes::tir::values::ValueId;

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

fn fixed_boxed_alloc(result: ValueId) -> TirOp {
    let mut allocation = op(OpCode::Alloc, vec![], vec![result]);
    allocation.attrs.insert("value".into(), AttrValue::Int(16));
    allocation
}

/// `obj.<offset> = val` ordinary typed-slot store. Callback-free lowering is
/// admitted only when the shared pristine-slot analysis proves this exact site.
fn store(obj: ValueId, val: ValueId, offset: i64) -> TirOp {
    let mut o = op(OpCode::StoreAttr, vec![obj, val], vec![]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("store".into()));
    o
}

/// `r = obj.<offset>` direct typed-slot load. The exact-site slot plan decides
/// whether this operation is callback-free and eligible for forwarding.
fn load(obj: ValueId, offset: i64, r: ValueId) -> TirOp {
    let mut o = op(OpCode::LoadAttr, vec![obj], vec![r]);
    o.attrs.insert("value".into(), AttrValue::Int(offset));
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("load".into()));
    o
}

fn with_class_view(mut operation: TirOp, class: &str) -> TirOp {
    operation
        .attrs
        .insert("_class".into(), AttrValue::Str(class.into()));
    operation
}

/// An opaque call that clobbers `GenericHeap`.
fn call(args: Vec<ValueId>, r: ValueId) -> TirOp {
    op(OpCode::Call, args, vec![r])
}

fn run_fresh(func: &mut TirFunction) -> PassStats {
    let mut am = AnalysisManager::new();
    run(func, &mut am)
}

// ── 1. Simple same-block store-to-load forwarding ──────────────────────

#[test]
fn forward_same_block_store_to_load() {
    // obj = alloc(16); store(obj, val, 0); r = load(obj, 0); return r
    // → the load becomes Copy(val).
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 1, "the load is forwarded");
    let ops = &func.blocks[&func.entry_block].ops;
    // alloc@0; const@1; store@2; the load@3 becomes IncRef(val)@3 + Copy@4 (the
    // IncRef reproduces the owned-result +1 the load performed).
    assert_eq!(
        ops[3].opcode,
        OpCode::IncRef,
        "owned-ref acquired before the Copy"
    );
    assert_eq!(ops[3].operands, vec![val], "IncRef of the forwarded value");
    assert_eq!(ops[4].opcode, OpCode::Copy, "load rewritten to Copy");
    assert_eq!(ops[4].operands, vec![val], "copies the stored value");
    assert_eq!(ops[4].results, vec![r], "result ValueId preserved");
    assert!(ops[4].attrs.is_empty(), "pure SSA move — no _original_kind");
}

#[test]
fn unknown_value_store_does_not_enable_forwarding() {
    // An arbitrary boxed argument may be the internal missing marker. Even on
    // real allocation backing, storing it does not prove the field is present.
    let mut func = TirFunction::new(
        "unknown_stored_value".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let unknown = ValueId(0);
    let object = func.fresh_value();
    let loaded = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(object));
        entry.ops.push(store(object, unknown, 0));
        entry.ops.push(load(object, 0, loaded));
        entry.terminator = Terminator::Return {
            values: vec![loaded],
        };
    }

    let aliases = AliasAnalysisResult::compute(&func);
    assert_eq!(
        aliases.region_of(&func.blocks[&func.entry_block].ops[2]),
        molt_passes::tir::passes::alias_analysis::MemRegion::GenericHeap,
        "context-free load classification retains its fallback floor"
    );
    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(
        func.blocks[&func.entry_block].ops[2].opcode,
        OpCode::LoadAttr,
        "missing-marker uncertainty must keep the load callback-capable"
    );
}

#[test]
fn replacing_store_is_not_a_forwarding_source() {
    // This receiver intentionally remains an unproved argument: an ordinary
    // store may replace a heap-owned field and run arbitrary finalization.
    let mut func = TirFunction::new(
        "replacing_store".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let loaded = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(store(ValueId(0), ValueId(1), 0));
        entry.ops.push(load(ValueId(0), 0, loaded));
        entry.terminator = Terminator::Return {
            values: vec![loaded],
        };
    }

    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(
        func.blocks[&func.entry_block].ops[1].opcode,
        OpCode::LoadAttr,
        "old-value finalization can reenter and change the slot"
    );
}

#[test]
fn later_store_to_initialized_fresh_slot_is_not_a_forwarding_source() {
    let mut func = TirFunction::new(
        "fresh_heap_old_replacement".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let first = ValueId(0);
    let replacement = ValueId(1);
    let object = func.fresh_value();
    let loaded = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(object));
        entry.ops.push(store(object, first, 0));
        entry.ops.push(store(object, replacement, 0));
        entry.ops.push(load(object, 0, loaded));
        entry.terminator = Terminator::Return {
            values: vec![loaded],
        };
    }

    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(
        func.blocks[&func.entry_block].ops[3].opcode,
        OpCode::LoadAttr,
        "replacing a heap-old value can finalize and reenter before the load"
    );
}

// ── 2. Forward blocked by an interposed call (GenericHeap clobber) ─────

#[test]
fn field_result_escape_keeps_callback_barrier_without_receiver_operands() {
    for allocation in [OpCode::Alloc, OpCode::ObjectNewBound] {
        let mut func = TirFunction::new(
            format!("local_callback_{allocation:?}"),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let object = func.fresh_value();
        let value = func.fresh_value();
        let callback = func.fresh_value();
        let result = func.fresh_value();
        let operands = if allocation == OpCode::Alloc {
            vec![]
        } else {
            vec![ValueId(0)]
        };
        let mut allocation_op = op(allocation, operands, vec![object]);
        allocation_op
            .attrs
            .insert("value".into(), AttrValue::Int(16));
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.extend([
            allocation_op,
            const_int(7, value),
            store(object, value, 0),
            call(vec![ValueId(1)], callback),
            load(object, 0, result),
        ]);
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let aliases = AliasAnalysisResult::compute(&func);
        // Returning the field can expose the receiver's retained graph, but
        // capture does not erase the exact physical allocation identity of a
        // direct load. The unrelated callback remains a whole-heap barrier.
        assert_eq!(
            aliases.region_of(&func.blocks[&func.entry_block].ops[3]),
            molt_passes::tir::passes::alias_analysis::MemRegion::GenericHeap,
        );
        assert_eq!(
            aliases.region_of(&func.blocks[&func.entry_block].ops[4]),
            molt_passes::tir::passes::alias_analysis::MemRegion::GenericHeap,
        );
        assert_eq!(run_fresh(&mut func).values_changed, 0);
        assert_eq!(
            func.blocks[&func.entry_block].ops[4].opcode,
            OpCode::LoadAttr
        );
    }
}

#[test]
fn forward_blocked_by_interposed_call() {
    // obj = alloc(16); store(obj, val, 0); call(obj); r = load(obj, 0)
    // The call is a GenericHeap def between store and load → the load's
    // reaching def is the call, not the store → NOT forwarded.
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
    let val = ValueId(0);
    let obj = func.fresh_value();
    let call_r = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(call(vec![obj], call_r));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 0, "clobbering call blocks forwarding");
    assert_eq!(
        func.blocks[&func.entry_block].ops[3].opcode,
        OpCode::LoadAttr,
        "load stays a real LoadAttr across the call barrier"
    );
}

#[test]
fn forwarding_is_blocked_by_callback_effects_without_root_operands() {
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
        let val = ValueId(0);
        let unrelated_lhs = ValueId(1);
        let unrelated_rhs = ValueId(2);
        let obj = func.fresh_value();
        let callback_result = func.fresh_value();
        let loaded = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(fixed_boxed_alloc(obj));
            entry.ops.push(store(obj, val, 0));
            entry.ops.push(op(
                opcode,
                vec![unrelated_lhs, unrelated_rhs],
                vec![callback_result],
            ));
            entry.ops.push(load(obj, 0, loaded));
            entry.terminator = Terminator::Return {
                values: vec![loaded],
            };
        }

        let stats = run_fresh(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "{opcode:?} can reach a callback that mutates the stored slot"
        );
        assert_eq!(
            func.blocks[&func.entry_block].ops[3].opcode,
            OpCode::LoadAttr,
            "{opcode:?}: the load must remain after the callback barrier"
        );
    }
}

#[test]
fn exact_integer_add_does_not_block_forwarding() {
    let mut func = TirFunction::new("exact_add".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let left = func.fresh_value();
    let right = func.fresh_value();
    let sum = func.fresh_value();
    let loaded = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
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

    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 1,
        "exact I64 addition has no callback and must preserve the reaching store"
    );
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[6].opcode, OpCode::IncRef);
    assert_eq!(ops[7].opcode, OpCode::Copy);
    assert_eq!(ops[7].operands, vec![val]);
    assert_eq!(ops[7].results, vec![loaded]);
}

#[test]
fn marked_async_work_poll_blocks_forwarding() {
    let mut func = TirFunction::new("async_poll".into(), vec![TirType::DynBox], TirType::DynBox);
    let value = ValueId(0);
    let object = func.fresh_value();
    let loaded = func.fresh_value();
    let mut poll = op(OpCode::CheckException, vec![], vec![]);
    assert!(poll.mark_async_work_poll());
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(object));
        entry.ops.push(store(object, value, 0));
        entry.ops.push(poll);
        entry.ops.push(load(object, 0, loaded));
        entry.terminator = Terminator::Return {
            values: vec![loaded],
        };
    }

    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(
        func.blocks[&func.entry_block].ops[3].opcode,
        OpCode::LoadAttr,
        "the marked poll may run arbitrary queued work"
    );
}

// ── 3. Forward blocked by a different offset (must-alias slot) ─────────

#[test]
fn forward_blocked_by_different_offset() {
    // obj = alloc(16); store(obj, val, 8); r = load(obj, 0)
    // Different offset → the load does NOT read the store's bytes → NOT
    // forwarded. Exact-site admission does not invent a stored SSA value for
    // offset 0 from the store at offset 8.
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(const_str("other-slot", val));
        entry.ops.push(store(obj, val, 8));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 0,
        "offset mismatch must block forwarding (different slot)"
    );
    assert_eq!(
        func.blocks[&func.entry_block].ops[3].opcode,
        OpCode::LoadAttr
    );
}

/// A proven store to a different physical word does not disturb offset 0.
#[test]
fn interposed_other_offset_store_does_not_misforward() {
    // obj = alloc(16); store(obj, v0, 0); store(obj, v8, 8); r = load(obj, 0)
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let v0 = func.fresh_value();
    let v8 = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(const_str("offset-zero", v0));
        entry.ops.push(store(obj, v0, 0));
        entry.ops.push(const_int(8, v8));
        entry.ops.push(store(obj, v8, 8));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 1,
        "the disjoint offset-8 store preserves the offset-0 reaching value"
    );
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[5].opcode, OpCode::IncRef);
    assert_eq!(ops[5].operands, vec![v0]);
    assert_eq!(ops[6].opcode, OpCode::Copy);
}

// ── 4. Cross-block forward through a single dominating def ─────────────

#[test]
fn forward_cross_block_through_dominating_store() {
    // bb0: obj = alloc(16); store(obj, val, 0) → bb1 → bb2: r = load(obj, 0)
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let bb1 = func.fresh_block();
    let bb2 = func.fresh_block();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.terminator = Terminator::Branch {
            target: bb1,
            args: vec![],
        };
    }
    func.blocks.insert(
        bb1,
        TirBlock {
            id: bb1,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: bb2,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        bb2,
        TirBlock {
            id: bb2,
            args: vec![],
            ops: vec![load(obj, 0, r)],
            terminator: Terminator::Return { values: vec![r] },
        },
    );
    molt_passes::tir::verify::verify_function(&func)
        .expect("fresh allocation must dominate the cross-block field load");
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 1,
        "cross-block forward through linear chain"
    );
    let bb2_ops = &func.blocks[&bb2].ops;
    assert_eq!(
        bb2_ops[0].opcode,
        OpCode::IncRef,
        "owned-ref acquired in the use block"
    );
    assert_eq!(bb2_ops[0].operands, vec![val]);
    assert_eq!(bb2_ops[1].opcode, OpCode::Copy);
    assert_eq!(bb2_ops[1].operands, vec![val]);
    assert_eq!(bb2_ops[1].results, vec![r]);
}

// ── 5. NO forward through a MemoryPhi merge ────────────────────────────

#[test]
fn forward_blocked_by_memory_phi_merge() {
    // bb0: obj = alloc(16) -> {bb1: store(obj,v1,0), bb2: store(obj,v2,0)}
    // -> bb3: r = load(obj,0)
    // The join places a MemoryPhi; the load reads the phi version, which is
    // not any single store's version → NOT forwarded.
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::Bool],
        TirType::DynBox,
    );
    let v1 = ValueId(0);
    let v2 = ValueId(1);
    let cond = ValueId(2);
    let obj = func.fresh_value();
    let bb1 = func.fresh_block();
    let bb2 = func.fresh_block();
    let bb3 = func.fresh_block();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
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
    molt_passes::tir::verify::verify_function(&func)
        .expect("fresh allocation must dominate both memory-phi predecessors");
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 0,
        "a phi-merged load has no single direct store def — forwarding blocked"
    );
    assert_eq!(func.blocks[&bb3].ops[0].opcode, OpCode::LoadAttr);
}

// ── 6. Redundant-load elimination, same block ──────────────────────────

#[test]
fn redundant_load_elim_same_block() {
    // r1 = load(obj, 0); r2 = load(obj, 0); return r1 + r2
    // No store and no clobber between → both loads read LIVE_ON_ENTRY for
    // the same slot → the second collapses to Copy(r1).
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let r1 = func.fresh_value();
    let r2 = func.fresh_value();
    let sum = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(load(obj, 0, r1));
        entry.ops.push(load(obj, 0, r2));
        entry.ops.push(op(OpCode::Add, vec![r1, r2], vec![sum]));
        entry.terminator = Terminator::Return { values: vec![sum] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 1, "the second load is redundant");
    let ops = &func.blocks[&func.entry_block].ops;
    // load@1 stays the leader; the redundant load@2 becomes IncRef(r1)@2 +
    // Copy(r1)->r2@3 (each owned load duplicates the +1, so r2 must too).
    assert_eq!(ops[1].opcode, OpCode::LoadAttr, "first load is the leader");
    assert_eq!(
        ops[2].opcode,
        OpCode::IncRef,
        "second load's owned +1 is reacquired"
    );
    assert_eq!(ops[2].operands, vec![r1]);
    assert_eq!(ops[3].opcode, OpCode::Copy, "second load reuses the first");
    assert_eq!(ops[3].operands, vec![r1]);
    assert_eq!(ops[3].results, vec![r2]);
}

// ── 7. Redundant-load blocked by a clobber between the two loads ───────

#[test]
fn redundant_load_blocked_by_clobber() {
    // r1 = load(obj, 0); call(obj); r2 = load(obj, 0)
    // The call clobbers the slot (GenericHeap def) → the two loads read
    // DIFFERENT memory versions → the second is NOT redundant.
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let r1 = func.fresh_value();
    let call_r = func.fresh_value();
    let r2 = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(load(obj, 0, r1));
        entry.ops.push(call(vec![obj], call_r));
        entry.ops.push(load(obj, 0, r2));
        entry.terminator = Terminator::Return { values: vec![r2] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 0,
        "a clobber between two loads makes the second non-redundant"
    );
    assert_eq!(
        func.blocks[&func.entry_block].ops[3].opcode,
        OpCode::LoadAttr
    );
}

/// Two loads of the SAME slot in non-dominating sibling blocks must NOT
/// collapse: neither leader dominates the other.
#[test]
fn redundant_load_not_across_sibling_blocks() {
    // bb0 cond → {bb1: r1 = load(obj,0)} / {bb2: r2 = load(obj,0)} → bb3
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::Bool],
        TirType::DynBox,
    );
    let obj = ValueId(0);
    let cond = ValueId(1);
    let bb1 = func.fresh_block();
    let bb2 = func.fresh_block();
    let bb3 = func.fresh_block();
    let r1 = func.fresh_value();
    let r2 = func.fresh_value();
    let arg = func.fresh_value();
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
            ops: vec![load(obj, 0, r1)],
            terminator: Terminator::Branch {
                target: bb3,
                args: vec![r1],
            },
        },
    );
    func.blocks.insert(
        bb2,
        TirBlock {
            id: bb2,
            args: vec![],
            ops: vec![load(obj, 0, r2)],
            terminator: Terminator::Branch {
                target: bb3,
                args: vec![r2],
            },
        },
    );
    func.blocks.insert(
        bb3,
        TirBlock {
            id: bb3,
            args: vec![molt_passes::tir::values::TirValue {
                id: arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return { values: vec![arg] },
        },
    );
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 0,
        "sibling-block loads must not collapse (no dominance)"
    );
    assert_eq!(func.blocks[&bb1].ops[0].opcode, OpCode::LoadAttr);
    assert_eq!(func.blocks[&bb2].ops[0].opcode, OpCode::LoadAttr);
}

// ── 8. Different objects are not forwarded ─────────────────────────────

#[test]
fn forward_blocked_by_different_object() {
    // a = alloc(16); b = alloc(16); store(a, val, 0); r = load(b, 0).
    let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
    let val = ValueId(0);
    let a = func.fresh_value();
    let b = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(a));
        entry.ops.push(fixed_boxed_alloc(b));
        entry.ops.push(store(a, val, 0));
        entry.ops.push(load(b, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let stats = run_fresh(&mut func);
    // Exact-site allocation identity distinguishes the roots, so the store to
    // `a` cannot provide the value loaded from `b`.
    assert_eq!(
        stats.values_changed, 0,
        "distinct object roots must block forwarding"
    );
}

/// A transparent Copy alias of the object is recognized: store through the
/// root, load through an alias of the same root → forwarded.
#[test]
fn forward_through_transparent_alias() {
    // obj = alloc(16); a = Copy(obj); store(obj, val, 0); r = load(a, 0)
    // → Copy(val).
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let a = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(op(OpCode::Copy, vec![obj], vec![a]));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(load(a, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 1,
        "load through a transparent alias of the store target forwards"
    );
    // alloc@0; const@1; Copy(obj)->a@2; store@3; load@4
    // → IncRef(val)@4 + Copy(val)->r@5.
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[4].opcode, OpCode::IncRef);
    assert_eq!(ops[4].operands, vec![val]);
    assert_eq!(ops[5].opcode, OpCode::Copy);
    assert_eq!(ops[5].operands, vec![val]);
}

#[test]
fn same_allocation_base_and_derived_views_forward_one_physical_store() {
    let mut func = TirFunction::new("base_derived_view".into(), vec![], TirType::DynBox);
    let object = func.fresh_value();
    let base_value = func.fresh_value();
    let loaded = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.extend([
            fixed_boxed_alloc(object),
            const_str("owned", base_value),
            with_class_view(store(object, base_value, 0), "Base"),
            with_class_view(load(object, 0, loaded), "Derived"),
        ]);
        entry.terminator = Terminator::Return {
            values: vec![loaded],
        };
    }

    let aliases = AliasAnalysisResult::compute(&func);
    assert_eq!(
        aliases.region_of(&func.blocks[&func.entry_block].ops[3]),
        molt_passes::tir::passes::alias_analysis::MemRegion::GenericHeap,
        "context-free load classification retains its fallback floor"
    );
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 1,
        "class metadata does not split one admitted physical word"
    );
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[3].opcode, OpCode::IncRef);
    assert_eq!(ops[3].operands, vec![base_value]);
    assert_eq!(ops[4].opcode, OpCode::Copy);
    assert_eq!(ops[4].operands, vec![base_value]);
    assert_eq!(ops[4].results, vec![loaded]);
}

#[test]
fn unknown_receiver_field_views_remain_callback_capable() {
    let mut func = TirFunction::new(
        "unknown_base_derived_view".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let object = ValueId(0);
    let base_load = func.fresh_value();
    let derived_load = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.extend([
            with_class_view(load(object, 0, base_load), "Base"),
            with_class_view(load(object, 0, derived_load), "Derived"),
        ]);
        entry.terminator = Terminator::Return {
            values: vec![derived_load],
        };
    }

    let aliases = AliasAnalysisResult::compute(&func);
    for operation in &func.blocks[&func.entry_block].ops {
        assert_eq!(
            aliases.region_of(operation),
            molt_passes::tir::passes::alias_analysis::MemRegion::GenericHeap,
        );
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 0);
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[0].opcode, OpCode::LoadAttr);
    assert_eq!(ops[1].opcode, OpCode::LoadAttr);
}

// ── Production op-shape coverage ───────────────────────────────────────

/// Guarded field reads can fall back to generic attribute lookup and are not
/// precise typed-slot loads, even when they carry an offset/class hint.
#[test]
fn guarded_field_get_three_operand_form_is_not_a_typed_slot_load() {
    let mut o = op(
        OpCode::LoadAttr,
        vec![ValueId(0), ValueId(1), ValueId(2)],
        vec![ValueId(3)],
    );
    o.attrs.insert("value".into(), AttrValue::Int(8));
    o.attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("guarded_field_get".into()),
    );
    assert_eq!(o.plain_typed_slot_load(), None);
}

/// A plain store cannot forward into a guarded read: the guard miss path can
/// invoke generic attribute lookup and observe different state.
#[test]
fn guarded_field_get_fallback_blocks_forwarding() {
    fn gget(obj: ValueId, cls: ValueId, ver: ValueId, offset: i64, r: ValueId) -> TirOp {
        let mut o = op(OpCode::LoadAttr, vec![obj, cls, ver], vec![r]);
        o.attrs.insert("value".into(), AttrValue::Int(offset));
        o.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("guarded_field_get".into()),
        );
        // Preserve the production frontend hint while proving it is metadata,
        // not alias authority: guarded fallback remains whole-heap observable.
        o.attrs
            .insert("_class".into(), AttrValue::Str("Point".into()));
        o
    }
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let cls = ValueId(0);
    let ver = ValueId(1);
    let val = ValueId(2);
    let obj = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(store(obj, val, 8));
        entry.ops.push(gget(obj, cls, ver, 8, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let aliases = AliasAnalysisResult::compute(&func);
    assert_eq!(
        aliases.region_of(&func.blocks[&func.entry_block].ops[2]),
        molt_passes::tir::passes::alias_analysis::MemRegion::GenericHeap,
    );
    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(
        func.blocks[&func.entry_block].ops[2].opcode,
        OpCode::LoadAttr
    );
}

/// Plain direct slot loads still collapse across an unmarked local
/// CheckException status observation.
#[test]
fn redundant_plain_load_across_check_exception_collapses() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let r1 = func.fresh_value();
    let r2 = func.fresh_value();
    let sum = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(load(obj, 8, r1));
        entry.ops.push(op(OpCode::CheckException, vec![], vec![]));
        entry.ops.push(load(obj, 8, r2));
        entry.ops.push(op(OpCode::Add, vec![r1, r2], vec![sum]));
        entry.terminator = Terminator::Return { values: vec![sum] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(
        stats.values_changed, 1,
        "an interposed CheckException must NOT block redundant-load elim"
    );
    let ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(ops[1].opcode, OpCode::LoadAttr);
    assert_eq!(
        ops[2].opcode,
        OpCode::CheckException,
        "the check is preserved"
    );
    assert_eq!(ops[3].opcode, OpCode::IncRef, "the duplicated owned +1");
    assert_eq!(ops[3].operands, vec![r1]);
    assert_eq!(
        ops[4].opcode,
        OpCode::Copy,
        "the second plain load collapses across the local check"
    );
    assert_eq!(ops[4].operands, vec![r1]);
}

// ── Unconditional production path ──────────────────────────────────────

#[test]
fn run_forwards_without_ambient_disable_path() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let r = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(load(obj, 0, r));
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);
    assert_eq!(stats.values_changed, 1, "production pass forwards the load");
    assert_eq!(
        func.blocks[&func.entry_block].ops[3].opcode,
        OpCode::IncRef,
        "forwarded load acquires the owned reference"
    );
    assert_eq!(func.blocks[&func.entry_block].ops[4].opcode, OpCode::Copy);
}

// ── Refcount discipline (the soundness keystone) ───────────────────────

/// EVERY forwarded load must be immediately preceded by an `IncRef` of the
/// SAME source it copies. A typed-slot load returns an OWNED (+1) reference
/// (`object_field_get_ptr_raw` unconditionally `inc_ref_bits`); a bare
/// `Copy` would drop that +1 while the frontend's matching `DecRef` still
/// runs → use-after-free. This test pins the `IncRef(source); Copy(source)`
/// shape so a future "simplify to a plain Copy" regresses LOUDLY here, not
/// as a silent heap-corruption miscompile in production.
#[test]
fn every_forward_acquires_a_reference() {
    // Two forwards in one block (a store-to-load AND a redundant-load), so
    // the descending-index apply order is exercised too:
    //   obj = alloc(16); store(obj,val,0); r1 = load(obj,0);
    //   r2 = load(obj,0); sum=r1+r2
    // r1 forwards from the store; r2 is redundant against r1.
    let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let val = func.fresh_value();
    let r1 = func.fresh_value();
    let r2 = func.fresh_value();
    let sum = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(fixed_boxed_alloc(obj));
        entry.ops.push(const_str("stored", val));
        entry.ops.push(store(obj, val, 0));
        entry.ops.push(load(obj, 0, r1));
        entry.ops.push(load(obj, 0, r2));
        entry.ops.push(op(OpCode::Add, vec![r1, r2], vec![sum]));
        entry.terminator = Terminator::Return { values: vec![sum] };
    }
    let stats = run_fresh(&mut func);
    assert_eq!(stats.values_changed, 2, "both loads forward");
    assert_eq!(stats.ops_added, 2, "one IncRef inserted per forward");

    // Invariant: scanning the block, every `Copy` whose result was an
    // original load result is immediately preceded by `IncRef(sameSource)`.
    let ops = &func.blocks[&func.entry_block].ops;
    let mut checked = 0;
    for (i, o) in ops.iter().enumerate() {
        if o.opcode == OpCode::Copy && (o.results == vec![r1] || o.results == vec![r2]) {
            assert!(i >= 1, "a forwarded Copy must have a preceding op");
            let prev = &ops[i - 1];
            assert_eq!(prev.opcode, OpCode::IncRef, "Copy is preceded by IncRef");
            assert_eq!(
                prev.operands, o.operands,
                "the IncRef acquires exactly the value the Copy forwards"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 2, "both forwarded copies validated");
}
