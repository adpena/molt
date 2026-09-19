use super::*;
use crate::tir::ops::{AttrDict, Dialect, OpCode};

fn allocation(opcode: OpCode, payload: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(payload));
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![ValueId(0)],
        results: vec![ValueId(1)],
        attrs,
        source_span: None,
    }
}

#[test]
fn allocation_extent_is_boxed_word_aligned_and_excludes_instance_dict() {
    let op = allocation(OpCode::ObjectNewBound, 24);
    let layout = boxed_allocation_layout(&op).unwrap();
    assert_eq!(layout.initial_value, OldSlotValue::FreshEmpty);
    assert_eq!(layout.result, ValueId(1));
    for offset in [0, 8] {
        assert!(layout.admits_field_offset(offset));
    }
    for offset in [-8, -1, 1, 7, 9, 16, 24, i64::MAX - 7] {
        assert!(!layout.admits_field_offset(offset), "{offset}");
    }
    assert!(
        !boxed_allocation_layout(&allocation(OpCode::ObjectNewBound, 8))
            .unwrap()
            .admits_field_offset(0)
    );
    for payload in [-8, 0, 7, 9, 23, i64::MAX] {
        assert!(boxed_allocation_layout(&allocation(OpCode::ObjectNewBound, payload)).is_none());
    }
}

#[test]
fn raw_boxed_layout_has_no_class_dictionary_tail() {
    let mut raw = allocation(OpCode::Alloc, 16);
    raw.operands.clear();
    let layout = boxed_allocation_layout(&raw).unwrap();
    assert_eq!(layout.initial_value, OldSlotValue::BoxedNeutral);
    assert!(layout.admits_field_offset(0));
    assert!(layout.admits_field_offset(8));
    assert!(!layout.admits_field_offset(16));
    raw.operands.push(ValueId(0));
    assert!(
        boxed_allocation_layout(&raw).is_none(),
        "raw payload size must be static"
    );
}

#[test]
fn allocation_shape_and_payload_are_required() {
    for defect in ["operand", "result", "missing", "type", "opcode"] {
        let mut op = allocation(OpCode::ObjectNewBound, 24);
        match defect {
            "operand" => op.operands.clear(),
            "result" => op.results.push(ValueId(2)),
            "missing" => {
                op.attrs.remove("value");
            }
            "type" => {
                op.attrs.insert("value".into(), AttrValue::Bool(true));
            }
            "opcode" => op.opcode = OpCode::Alloc,
            _ => unreachable!(),
        }
        assert!(boxed_allocation_layout(&op).is_none(), "{defect}");
    }
}

#[test]
fn fixed_layout_admission_covers_the_entire_opcode_and_operand_domain() {
    use crate::tir::op_kinds_generated::ALL_OPCODES;

    for &opcode in ALL_OPCODES {
        for operand_count in 0..=3 {
            for result_count in 0..=2 {
                let mut op = allocation(opcode, 24);
                op.operands = vec![ValueId(0); operand_count];
                op.results = (1..=result_count).map(ValueId).collect();
                let expected = result_count == 1
                    && ((opcode == OpCode::Alloc && operand_count == 0)
                        || (opcode == OpCode::ObjectNewBound && operand_count == 1));
                assert_eq!(
                    boxed_allocation_layout(&op).is_some(),
                    expected,
                    "{opcode:?}/{operand_count}/{result_count}"
                );
            }
        }
    }
}

#[test]
fn initialization_and_replacement_modes_preserve_incoming_heap_ownership() {
    for old_value in [OldSlotValue::FreshEmpty, OldSlotValue::BoxedNeutral] {
        for incoming_boxed_neutral in [false, true] {
            let facts = TypedSlotStoreFacts {
                old_value,
                incoming_boxed_neutral,
            };
            assert_eq!(
                facts.lowering_mode(),
                if old_value == OldSlotValue::BoxedNeutral && incoming_boxed_neutral {
                    TypedSlotStoreMode::DirectNonHeap
                } else {
                    TypedSlotStoreMode::FreshInit
                }
            );
        }
    }
}

use crate::tir::blocks::TirBlock;
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

fn field(opcode: OpCode, object: ValueId, value: ValueId, offset: i64) -> TirOp {
    let load = opcode == OpCode::LoadAttr;
    let mut access = if load {
        op(opcode, vec![object], vec![value])
    } else {
        op(opcode, vec![object, value], vec![])
    };
    access.attrs.insert("value".into(), AttrValue::Int(offset));
    access.attrs.insert(
        "_original_kind".into(),
        AttrValue::Str(if load { "load" } else { "store" }.into()),
    );
    access
}

fn fixture(class: bool) -> (TirFunction, ValueId) {
    let mut func = TirFunction::new("slot_access".into(), vec![TirType::DynBox], TirType::None);
    let object = func.fresh_value();
    let mut alloc = allocation(
        if class {
            OpCode::ObjectNewBound
        } else {
            OpCode::Alloc
        },
        24,
    );
    alloc.results = vec![object];
    if !class {
        alloc.operands.clear();
    }
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(alloc);
    entry.terminator = Terminator::Return { values: vec![] };
    (func, object)
}

fn append(func: &mut TirFunction, operation: TirOp) -> AccessSite {
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let site = (entry.id, entry.ops.len());
    entry.ops.push(operation);
    site
}

fn read(func: &mut TirFunction, object: ValueId, offset: i64) -> AccessSite {
    let result = func.fresh_value();
    append(func, field(OpCode::LoadAttr, object, result, offset))
}

fn plan(func: &TirFunction) -> TypedSlotAccessPlan {
    for_function(func, &mut AnalysisManager::new())
}

#[test]
fn load_admission_distinguishes_zero_missing_presence_and_extent() {
    for class in [false, true] {
        for value_kind in ["initial", "none", "heap", "unknown", "missing"] {
            for offset in [0, 1, 16, 24] {
                let (mut func, object) = fixture(class);
                if value_kind != "initial" {
                    let value = if value_kind == "unknown" {
                        ValueId(0)
                    } else {
                        let result = func.fresh_value();
                        let mut producer = op(
                            match value_kind {
                                "none" => OpCode::ConstNone,
                                "heap" => OpCode::ConstStr,
                                "missing" => OpCode::Copy,
                                _ => unreachable!(),
                            },
                            vec![],
                            vec![result],
                        );
                        if value_kind == "heap" {
                            producer
                                .attrs
                                .insert("value".into(), AttrValue::Str("owned".into()));
                        } else if value_kind == "missing" {
                            producer
                                .attrs
                                .insert("_original_kind".into(), AttrValue::Str("missing".into()));
                        }
                        append(&mut func, producer);
                        result
                    };
                    append(&mut func, field(OpCode::StoreAttr, object, value, offset));
                }
                let site = read(&mut func, object, offset);
                let admitted = (offset == 0 || (!class && offset == 16))
                    && match value_kind {
                        "initial" => !class,
                        "none" | "heap" => true,
                        _ => false,
                    };
                assert_eq!(
                    plan(&func).loads.contains(&site),
                    admitted,
                    "class={class} value={value_kind} offset={offset}"
                );
            }
        }
    }
}

#[test]
fn proven_reads_preserve_presence_but_never_restore_pristine_writes() {
    let (mut func, object) = fixture(false);
    let value = func.fresh_value();
    append(&mut func, op(OpCode::ConstNone, vec![], vec![value]));
    let first_store = append(&mut func, field(OpCode::StoreAttr, object, value, 0));
    let first = read(&mut func, object, 0);
    let second = read(&mut func, object, 0);
    let next_store = append(&mut func, field(OpCode::StoreAttr, object, value, 0));
    let third = read(&mut func, object, 0);
    let facts = plan(&func);
    assert!(facts.stores.contains_key(&first_store));
    assert_eq!(facts.loads, BTreeSet::from([first, second]));
    assert!(!facts.stores.contains_key(&next_store));
    assert!(!facts.loads.contains(&third));
    assert!(
        facts.dead_stores.is_empty(),
        "observed writes cannot be removed"
    );
}

#[test]
fn unproved_reads_clobber_other_receivers_without_operand_overlap() {
    for kind in ["load", "guarded_field_get", "get_attr"] {
        let (mut func, object) = fixture(false);
        let first = read(&mut func, object, 0);
        let unknown = read(&mut func, ValueId(0), 0);
        func.blocks.get_mut(&unknown.0).unwrap().ops[unknown.1]
            .attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        let last = read(&mut func, object, 0);
        let facts = plan(&func);
        assert_eq!(facts.loads, BTreeSet::from([first]), "{kind}");
        assert!(!facts.loads.contains(&last));
    }
}

#[test]
fn callbacks_and_dictionary_observation_revoke_load_admission() {
    for opcode in [OpCode::Call, OpCode::DecRef, OpCode::StoreAttr] {
        let (mut func, object) = fixture(false);
        let first = read(&mut func, object, 0);
        append(&mut func, op(opcode, vec![object], vec![]));
        let last = read(&mut func, object, 0);
        let facts = plan(&func);
        assert_eq!(facts.loads, BTreeSet::from([first]), "{opcode:?}");
        assert!(!facts.loads.contains(&last));
    }
}

#[test]
fn linear_normal_edges_transfer_access_facts_not_exception_or_back_edges() {
    for boundary in ["normal", "exception", "join", "backedge"] {
        let (mut func, object) = fixture(false);
        let entry_id = func.entry_block;
        let successor = func.fresh_block();
        let other = func.fresh_block();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&entry_id).unwrap();
        entry.terminator = Terminator::Branch {
            target: successor,
            args: vec![],
        };
        if boundary == "exception" {
            let mut transfer = op(OpCode::CheckException, vec![], vec![]);
            transfer.attrs.insert("value".into(), AttrValue::Int(42));
            // A later allocation cannot lend end-of-block facts to this edge.
            entry.ops.insert(0, transfer);
            func.label_id_map.insert(successor.0, 42);
            func.has_exception_handling = true;
        } else if boundary == "join" {
            entry.terminator = Terminator::CondBranch {
                cond: ValueId(0),
                then_block: successor,
                then_args: vec![],
                else_block: other,
                else_args: vec![],
            };
            func.blocks.insert(
                other,
                TirBlock {
                    id: other,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Branch {
                        target: successor,
                        args: vec![],
                    },
                },
            );
        }
        func.blocks.insert(
            successor,
            TirBlock {
                id: successor,
                args: vec![],
                ops: vec![field(OpCode::LoadAttr, object, result, 0)],
                terminator: if boundary == "backedge" {
                    Terminator::Branch {
                        target: successor,
                        args: vec![],
                    }
                } else {
                    Terminator::Return { values: vec![] }
                },
            },
        );
        assert_eq!(
            plan(&func).loads.contains(&(successor, 0)),
            boundary == "normal",
            "{boundary}"
        );
    }
}

#[test]
fn mid_block_exception_exit_keeps_stores_visible_to_handler() {
    let (mut func, object) = fixture(false);
    let value = func.fresh_value();
    append(&mut func, op(OpCode::ConstNone, vec![], vec![value]));
    let first = append(&mut func, field(OpCode::StoreAttr, object, value, 0));
    let handler = func.fresh_block();
    let numerator = func.fresh_value();
    let denominator = func.fresh_value();
    for (result, integer) in [(numerator, 1), (denominator, 0)] {
        let mut constant = op(OpCode::ConstInt, vec![], vec![result]);
        constant
            .attrs
            .insert("value".into(), AttrValue::Int(integer));
        append(&mut func, constant);
    }
    let quotient = func.fresh_value();
    append(
        &mut func,
        op(OpCode::Div, vec![numerator, denominator], vec![quotient]),
    );
    let mut check = op(OpCode::CheckException, vec![], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(42));
    append(&mut func, check);
    let normal_read = read(&mut func, object, 0);
    let second = append(&mut func, field(OpCode::StoreAttr, object, value, 0));
    func.has_exception_handling = true;
    func.label_id_map.insert(handler.0, 42);
    let handler_value = func.fresh_value();
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![field(OpCode::LoadAttr, object, handler_value, 0)],
            terminator: Terminator::Return {
                values: vec![object],
            },
        },
    );
    let facts = plan(&func);
    assert!(facts.stores.contains_key(&first));
    assert!(!facts.stores.contains_key(&second));
    assert!(facts.loads.contains(&normal_read));
    assert!(!facts.loads.contains(&(handler, 0)));
    assert!(
        facts.dead_stores.is_empty(),
        "the handler can observe the first store"
    );
    let mut forwarded = func.clone();
    let stats = crate::tir::passes::mem_gvn::run(&mut forwarded, &mut AnalysisManager::new());
    assert_eq!(
        stats.values_changed, 1,
        "only the normal-path read forwards"
    );
    assert_eq!(forwarded.blocks[&handler].ops[0].opcode, OpCode::LoadAttr);
    // The edge alone must protect the store, independently of the normal read.
    func.blocks
        .get_mut(&normal_read.0)
        .unwrap()
        .ops
        .remove(normal_read.1);
    assert!(plan(&func).dead_stores.is_empty());
    let stats = crate::tir::passes::dead_store_elim::run(&mut func, &mut AnalysisManager::new());
    assert_eq!(
        stats.ops_removed, 0,
        "real DSE preserves handler-visible writes"
    );
}

#[test]
fn a_proven_read_only_observes_its_exact_allocation() {
    let (mut func, first) = fixture(false);
    let second = func.fresh_value();
    let mut allocation = allocation(OpCode::Alloc, 8);
    allocation.operands.clear();
    allocation.results = vec![second];
    append(&mut func, allocation);
    let value = func.fresh_value();
    append(&mut func, op(OpCode::ConstNone, vec![], vec![value]));
    let old = append(&mut func, field(OpCode::StoreAttr, first, value, 0));
    let access = read(&mut func, second, 0);
    append(&mut func, field(OpCode::StoreAttr, first, value, 0));
    let facts = plan(&func);
    assert!(facts.loads.contains(&access));
    assert_eq!(facts.dead_stores, BTreeSet::from([old]));
}

#[test]
fn storing_an_escaped_allocation_keeps_presence_not_pristine_identity() {
    let (mut func, first) = fixture(false);
    let second = func.fresh_value();
    let mut allocation = allocation(OpCode::Alloc, 8);
    allocation.operands.clear();
    allocation.results = vec![second];
    append(&mut func, allocation);
    let alias = func.fresh_value();
    append(&mut func, op(OpCode::Copy, vec![second], vec![alias]));
    append(&mut func, field(OpCode::StoreAttr, first, alias, 0));
    let loaded = read(&mut func, first, 0);
    let escaped = read(&mut func, second, 0);
    let facts = plan(&func);
    assert!(
        facts.loads.contains(&loaded),
        "an allocated object is not missing"
    );
    assert!(
        !facts.loads.contains(&escaped),
        "published objects have lost pristine backing proof"
    );
}
