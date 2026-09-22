use molt_passes::tir::analysis::AnalysisManager;
use molt_passes::tir::blocks::{BlockId, Terminator, TirBlock};
use molt_passes::tir::function::TirFunction;
use molt_passes::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use molt_passes::tir::passes::PassStats;
use molt_passes::tir::passes::dead_store_elim::run;
use molt_passes::tir::types::TirType;
use molt_passes::tir::values::ValueId;

fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn run_fresh(func: &mut TirFunction) -> PassStats {
    run(func, &mut AnalysisManager::new())
}

/// All fixture operands are real parameters or explicit producers. In
/// particular, unknown parameters never stand in for immediate constants.
fn fixture() -> TirFunction {
    let mut func = TirFunction::new(
        "dse".into(),
        vec![TirType::DynBox; 4],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func
}

fn push(func: &mut TirFunction, op: TirOp) {
    func.blocks.get_mut(&func.entry_block).unwrap().ops.push(op);
}

fn integer(func: &mut TirFunction, value: i64) -> ValueId {
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::I64);
    let mut op = make_op(OpCode::ConstInt, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Int(value));
    push(func, op);
    result
}

fn allocate(func: &mut TirFunction) -> ValueId {
    let result = func.fresh_value();
    let mut op = make_op(OpCode::ObjectNewBound, vec![ValueId(0)], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Int(24));
    push(func, op);
    result
}

fn make_store(object: ValueId, value: ValueId, offset: i64, kind: &str) -> TirOp {
    let mut op = make_op(OpCode::StoreAttr, vec![object, value], vec![]);
    op.attrs.insert("value".into(), AttrValue::Int(offset));
    op.attrs
        .insert("_original_kind".into(), AttrValue::Str(kind.into()));
    op
}

fn store(func: &mut TirFunction, object: ValueId, value: ValueId, offset: i64, kind: &str) {
    push(func, make_store(object, value, offset, kind));
}

fn stores(func: &TirFunction) -> Vec<&TirOp> {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::StoreAttr)
        .collect()
}

fn typed_load(func: &mut TirFunction, object: ValueId) -> TirOp {
    let result = func.fresh_value();
    let mut op = make_op(OpCode::LoadAttr, vec![object], vec![result]);
    op.attrs
        .insert("_original_kind".into(), AttrValue::Str("load".into()));
    op.attrs
        .insert("_class".into(), AttrValue::Str("Fixture".into()));
    op.attrs.insert("value".into(), AttrValue::Int(0));
    op
}

#[test]
fn scalar_constructor_chains_preserve_owned_final_stores() {
    let mut func = fixture();
    let zero = integer(&mut func, 0);
    let one = integer(&mut func, 1);
    let two = integer(&mut func, 2);
    let object = allocate(&mut func);
    store(&mut func, object, zero, 0, "store");
    store(&mut func, object, zero, 8, "store");
    store(&mut func, object, one, 0, "store");
    store(&mut func, object, two, 8, "store");
    let stats = run_fresh(&mut func);
    assert_eq!(stats.ops_removed, 2);
    assert_eq!(stores(&func).len(), 2);
    assert_eq!(stores(&func)[0].operands, vec![object, one]);
    assert_eq!(stores(&func)[1].operands, vec![object, two]);
}

#[test]
fn triple_store_same_offset_kills_first_two() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    for _ in 0..3 {
        store(&mut func, object, value, 0, "store");
    }
    assert_eq!(run_fresh(&mut func).ops_removed, 2);
    assert_eq!(stores(&func).len(), 1);
}

#[test]
fn different_offsets_and_objects_are_not_overwrites() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let first = allocate(&mut func);
    let second = allocate(&mut func);
    store(&mut func, first, value, 0, "store");
    store(&mut func, first, value, 8, "store");
    store(&mut func, second, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(stores(&func).len(), 3);
}

#[test]
fn transparent_aliases_preserve_scalar_store_elimination() {
    for duplicate_operand in [false, true] {
        let mut func = fixture();
        let value = integer(&mut func, 1);
        let object = allocate(&mut func);
        store(&mut func, object, value, 0, "store");
        let alias = func.fresh_value();
        let args = if duplicate_operand {
            vec![object, object]
        } else {
            vec![object]
        };
        push(&mut func, make_op(OpCode::Copy, args, vec![alias]));
        store(&mut func, alias, value, 0, "store");
        assert_eq!(run_fresh(&mut func).ops_removed, 1);
        assert_eq!(stores(&func).len(), 1);
    }
}

#[test]
fn read_or_escape_permanently_revokes_pristine_custody() {
    for observer_kind in [
        "load",
        "call",
        "store_index",
        "set_attr_name",
        "capture_store",
    ] {
        let mut func = fixture();
        let value = integer(&mut func, 1);
        let object = allocate(&mut func);
        store(&mut func, object, value, 0, "store");
        let observer = match observer_kind {
            "load" => typed_load(&mut func, object),
            "call" => make_op(
                OpCode::Call,
                vec![ValueId(2), object],
                vec![func.fresh_value()],
            ),
            "store_index" => make_op(OpCode::StoreIndex, vec![object, value, value], vec![]),
            "set_attr_name" => make_store(object, value, 0, "set_attr_name"),
            "capture_store" => make_store(ValueId(1), object, 0, "store"),
            _ => unreachable!(),
        };
        push(&mut func, observer);
        // A later store cannot regain a pristine-slot fact after the earlier
        // capture or materialized-dict observation. Repeated stores stay live.
        store(&mut func, object, value, 0, "store");
        store(&mut func, object, value, 0, "store");
        assert_eq!(run_fresh(&mut func).ops_removed, 0, "{observer_kind}");
    }
}

/// Harvested from the shared heap-authority lane, with actual immediate store
/// values so this checks the callback barrier rather than the heap-value gate.
#[test]
fn callback_effects_without_root_operands_block_elim() {
    for opcode in [
        OpCode::Add,
        OpCode::Index,
        OpCode::ModuleGetAttr,
        OpCode::ModuleGetName,
        OpCode::ModuleImportFrom,
        OpCode::DecRef,
    ] {
        let mut func = fixture();
        let value = integer(&mut func, 1);
        let object = allocate(&mut func);
        store(&mut func, object, value, 0, "store");
        let callback = if opcode == OpCode::DecRef {
            make_op(opcode, vec![ValueId(1)], vec![])
        } else {
            make_op(
                opcode,
                vec![ValueId(2), ValueId(3)],
                vec![func.fresh_value()],
            )
        };
        push(&mut func, callback);
        store(&mut func, object, value, 0, "store");
        assert_eq!(run_fresh(&mut func).ops_removed, 0, "{opcode:?}");
        assert_eq!(stores(&func).len(), 2, "{opcode:?}");
    }
}

#[test]
fn exact_integer_add_does_not_block_elim() {
    let mut func = fixture();
    let one = integer(&mut func, 1);
    let two = integer(&mut func, 2);
    let object = allocate(&mut func);
    store(&mut func, object, one, 0, "store");
    let sum = func.fresh_value();
    func.value_types.insert(sum, TirType::I64);
    push(&mut func, make_op(OpCode::Add, vec![one, two], vec![sum]));
    store(&mut func, object, sum, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 1);
    assert_eq!(stores(&func).len(), 1);
    assert_eq!(stores(&func)[0].operands, vec![object, sum]);
}

#[test]
fn callback_before_a_new_allocation_does_not_poison_its_pristine_history() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let result = func.fresh_value();
    push(
        &mut func,
        make_op(OpCode::Call, vec![ValueId(2)], vec![result]),
    );
    let object = allocate(&mut func);
    store(&mut func, object, value, 0, "store");
    store(&mut func, object, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 1);
}

#[test]
fn inherited_receivers_never_acquire_fresh_allocation_custody() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    store(&mut func, ValueId(0), value, 0, "store");
    store(&mut func, ValueId(0), value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(stores(&func).len(), 2);
}

#[test]
fn initialized_candidate_payload_discharge_first_plain_scalar_store_release() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    store(&mut func, object, value, 0, "store");
    store(&mut func, object, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 1);
    assert_eq!(stores(&func).len(), 1);
}

#[test]
fn heap_and_wide_integer_initializers_preserve_retain_and_displaced_release() {
    for wide_integer in [false, true] {
        let mut func = fixture();
        let value = integer(&mut func, 1);
        // ConstInt outside inline47 is a legitimate RawI64FullDeopt carrier,
        // but writing it into a boxed field can allocate a heap BigInt.
        let heap = if wide_integer {
            integer(&mut func, i64::MAX)
        } else {
            ValueId(1)
        };
        let object = allocate(&mut func);
        store(&mut func, object, heap, 0, "store");
        store(&mut func, object, value, 0, "store");
        // The preceding old release can reenter and replace the scalar field.
        // It cannot establish neutral contents for this next overwrite.
        store(&mut func, object, value, 0, "store");
        assert_eq!(run_fresh(&mut func).ops_removed, 0);
        assert_eq!(stores(&func).len(), 3);
    }
}

#[test]
fn heap_value_between_neutral_stores_keeps_all_ownership_operations() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    store(&mut func, object, value, 0, "store");
    store(&mut func, object, ValueId(1), 0, "store");
    store(&mut func, object, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(stores(&func).len(), 3);
}

#[test]
fn incoming_type_annotations_are_not_boxed_neutral_proofs() {
    for hint in [TirType::I64, TirType::F64, TirType::Bool] {
        let mut func = fixture();
        func.value_types.insert(ValueId(1), hint);
        let value = integer(&mut func, 1);
        let object = allocate(&mut func);
        store(&mut func, object, ValueId(1), 0, "store");
        store(&mut func, object, value, 0, "store");
        assert_eq!(run_fresh(&mut func).ops_removed, 0);
    }
}

#[test]
fn unrelated_replacing_store_can_finalize_a_captured_observer() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    store(&mut func, object, value, 0, "store");
    // The old field of this different receiver can finalize an object whose
    // callback observes our pending store, without object in these operands.
    store(&mut func, ValueId(1), value, 0, "store");
    store(&mut func, object, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(stores(&func).len(), 3);
}

#[test]
fn captured_class_object_final_initializer_stays_live() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    let result = func.fresh_value();
    push(
        &mut func,
        make_op(OpCode::Call, vec![ValueId(2), object], vec![result]),
    );
    store(&mut func, object, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(stores(&func).len(), 1);
}

#[test]
fn class_object_returned_from_block_keeps_final_store() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    store(&mut func, object, value, 0, "store");
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Return {
        values: vec![object],
    };
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
}

#[test]
fn class_object_field_read_in_dominated_block_keeps_stores() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    store(&mut func, object, value, 0, "store");
    store(&mut func, object, value, 8, "store");
    let next_id = BlockId(1);
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: next_id,
        args: vec![],
    };
    let load = typed_load(&mut func, object);
    func.blocks.insert(
        next_id,
        TirBlock {
            id: next_id,
            args: vec![],
            ops: vec![load],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(stores(&func).len(), 2);
}

#[test]
fn unconditional_cross_block_overwrite_consumes_exact_entry_history() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    store(&mut func, object, value, 0, "store");
    let next_id = BlockId(1);
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: next_id,
        args: vec![],
    };
    func.blocks.insert(
        next_id,
        TirBlock {
            id: next_id,
            args: vec![],
            ops: vec![make_store(object, value, 0, "store")],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    assert_eq!(run_fresh(&mut func).ops_removed, 1);
    assert_eq!(stores(&func).len(), 1);
    assert_eq!(func.blocks[&next_id].ops[0].opcode, OpCode::StoreAttr);
}

#[test]
fn finalizer_bearing_class_allocation_does_not_erase_final_fields() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    func.blocks
        .get_mut(&func.entry_block)
        .unwrap()
        .ops
        .last_mut()
        .unwrap()
        .attrs
        .insert("defines_del".into(), AttrValue::Bool(true));
    store(&mut func, object, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
}

#[test]
fn malformed_or_result_producing_store_cannot_orphan_a_definition() {
    for defect in ["result", "offset", "arity", "variant"] {
        let mut func = fixture();
        let value = integer(&mut func, 1);
        let object = allocate(&mut func);
        let mut op = make_store(object, value, 0, "store");
        match defect {
            "result" => op.results.push(func.fresh_value()),
            "offset" => {
                op.attrs.remove("value");
            }
            "arity" => {
                op.operands.pop();
            }
            "variant" => {
                op.attrs.insert(
                    "_original_kind".into(),
                    AttrValue::Str("set_attr_name".into()),
                );
            }
            _ => unreachable!(),
        }
        let result = op.results.first().copied();
        push(&mut func, op);
        if let Some(value) = result {
            func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Return {
                values: vec![value],
            };
        }
        assert_eq!(run_fresh(&mut func).ops_removed, 0, "{defect}");
        assert_eq!(stores(&func).len(), 1, "{defect}");
    }
}

#[test]
fn class_layout_allocation_revokes_earlier_root_history() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let first = allocate(&mut func);
    store(&mut func, first, value, 0, "store");
    // Binding layout metadata can release an old class value and reenter.
    let second = allocate(&mut func);
    store(&mut func, second, value, 0, "store");
    store(&mut func, first, value, 0, "store");
    store(&mut func, second, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(stores(&func).len(), 4);
}

#[test]
fn known_boxed_bool_float_and_none_values_remain_optimizable() {
    for (opcode, ty, value) in [
        (
            OpCode::ConstBool,
            TirType::Bool,
            Some(AttrValue::Bool(true)),
        ),
        (
            OpCode::ConstFloat,
            TirType::F64,
            Some(AttrValue::Float(1.5)),
        ),
        (OpCode::ConstNone, TirType::None, None),
    ] {
        let mut func = fixture();
        let result = func.fresh_value();
        func.value_types.insert(result, ty);
        let mut op = make_op(opcode, vec![], vec![result]);
        if let Some(value) = value {
            op.attrs.insert("value".into(), value);
        }
        push(&mut func, op);
        let object = allocate(&mut func);
        store(&mut func, object, result, 0, "store");
        store(&mut func, object, result, 0, "store");
        assert_eq!(run_fresh(&mut func).ops_removed, 1, "{opcode:?}");
        assert_eq!(stores(&func).len(), 1);
    }
}

#[test]
fn observer_through_a_transparent_alias_keeps_original_slot_live() {
    let mut func = fixture();
    let value = integer(&mut func, 1);
    let object = allocate(&mut func);
    let alias = func.fresh_value();
    push(&mut func, make_op(OpCode::Copy, vec![object], vec![alias]));
    store(&mut func, object, value, 0, "store");
    let result = func.fresh_value();
    push(
        &mut func,
        make_op(OpCode::Call, vec![ValueId(2), alias], vec![result]),
    );
    store(&mut func, object, value, 0, "store");
    assert_eq!(run_fresh(&mut func).ops_removed, 0);
    assert_eq!(stores(&func).len(), 2);
}

#[test]
fn terminal_fields_survive_unknown_absent_and_positive_finalizer_metadata() {
    for finalizer in [None, Some(false), Some(true)] {
        for late_mutation in [false, true] {
            let mut func = fixture();
            let value = integer(&mut func, 7);
            let object = allocate(&mut func);
            if let Some(finalizer) = finalizer {
                func.blocks
                    .get_mut(&func.entry_block)
                    .unwrap()
                    .ops
                    .last_mut()
                    .unwrap()
                    .attrs
                    .insert("defines_del".into(), AttrValue::Bool(finalizer));
            }
            store(&mut func, object, value, 0, "store");
            if late_mutation {
                // The callback can mutate the object's class via a global;
                // it does not need the instance as an operand.
                let result = func.fresh_value();
                push(
                    &mut func,
                    make_op(OpCode::Call, vec![ValueId(2)], vec![result]),
                );
            }
            assert_eq!(run_fresh(&mut func).ops_removed, 0);
            assert_eq!(stores(&func).len(), 1);
        }
    }
}
