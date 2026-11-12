use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::ir::GlobalSlotRef;

fn slot(index: u32) -> GlobalSlotRef {
    GlobalSlotRef {
        module: "demo".into(),
        index,
    }
}

/// A function over (module, name, value) parameters.
fn function() -> TirFunction {
    TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::Str, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    )
}

fn entry_ops(func: &TirFunction) -> Vec<TirOp> {
    func.blocks[&func.entry_block].ops.clone()
}

fn verify(func: &TirFunction) {
    crate::tir::verify::verify_function(func).expect("rewritten function verifies");
}

#[test]
fn a_slotted_read_becomes_a_base_and_a_slot_read_with_the_by_name_operands() {
    let mut func = function();
    let read = func.fresh_value();
    func.value_types.insert(read, TirType::DynBox);
    let mut op = TirOp::new(
        OpCode::ModuleGetGlobal,
        vec![ValueId(0), ValueId(1)],
        vec![read],
    );
    op.set_global_slot(Some(&slot(3)));
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(op);
    block.terminator = Terminator::Return { values: vec![read] };

    super::run(&mut func);
    verify(&func);
    let ops = entry_ops(&func);
    let kinds: Vec<_> = ops.iter().map(|op| op.opcode).collect();
    assert_eq!(
        kinds,
        [
            OpCode::ModuleSlot,
            OpCode::GlobalSlotBase,
            OpCode::GlobalSlotGet
        ]
    );
    assert_eq!(
        ops[0].attrs.get("s_value"),
        Some(&AttrValue::Str("demo".into()))
    );
    assert_eq!(ops[1].operands, [ops[0].results[0]]);
    let get = &ops[2];
    assert_eq!(get.operands, [ops[1].results[0], ValueId(0), ValueId(1)]);
    assert_eq!(get.results, [read], "the read keeps its result identity");
    assert_eq!(get.global_slot(), Some(slot(3)));
}

#[test]
fn slotted_stores_call_the_store_by_index_with_their_by_name_fallback() {
    let mut func = function();
    let mut module_store = TirOp::new(
        OpCode::ModuleSetAttr,
        vec![ValueId(0), ValueId(1), ValueId(2)],
        vec![],
    );
    module_store.set_global_slot(Some(&slot(1)));
    let dict_result = func.fresh_value();
    func.value_types.insert(dict_result, TirType::DynBox);
    let mut frame_store = TirOp::new(
        OpCode::Copy,
        vec![ValueId(0), ValueId(1), ValueId(2)],
        vec![dict_result],
    );
    frame_store.origin = Some(molt_ir::OpKind::DictSet);
    frame_store.set_global_slot(Some(&slot(2)));
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(module_store);
    block.ops.push(frame_store);
    block.terminator = Terminator::Return {
        values: vec![ValueId(2)],
    };

    super::run(&mut func);
    verify(&func);
    let calls: Vec<_> = entry_ops(&func)
        .into_iter()
        .filter(|op| op.opcode == OpCode::Call)
        .collect();
    assert_eq!(calls.len(), 2);
    for call in &calls {
        assert_eq!(
            call.attrs.get("s_value"),
            Some(&AttrValue::Str("molt_global_slot_set".into()))
        );
        assert_eq!(call.global_slot(), None, "a call carries no slot fact");
        assert_eq!(call.operands.len(), 5);
        assert_eq!(&call.operands[3..], [ValueId(1), ValueId(2)]);
    }
    assert_eq!(calls[0].operands[2], ValueId(0), "module-level: the module");
    let none = calls[1].operands[2];
    assert!(
        entry_ops(&func)
            .iter()
            .any(|op| op.opcode == OpCode::ConstNone && op.results == [none])
    );
}

#[test]
fn a_store_whose_result_is_read_keeps_its_by_name_op() {
    let mut func = function();
    let dict_result = func.fresh_value();
    func.value_types.insert(dict_result, TirType::DynBox);
    let mut frame_store = TirOp::new(
        OpCode::Copy,
        vec![ValueId(0), ValueId(1), ValueId(2)],
        vec![dict_result],
    );
    frame_store.origin = Some(molt_ir::OpKind::DictSet);
    frame_store.set_global_slot(Some(&slot(2)));
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.push(frame_store);
    block.terminator = Terminator::Return {
        values: vec![dict_result],
    };

    super::run(&mut func);
    let ops = entry_ops(&func);
    let store = ops.iter().find(|op| op.opcode == OpCode::Copy).unwrap();
    assert_eq!(store.origin, Some(molt_ir::OpKind::DictSet));
    assert_eq!(store.global_slot(), None);
}

#[test]
fn reads_of_one_module_share_one_base_after_the_cleanup_round() {
    let mut func = function();
    let first = func.fresh_value();
    let second = func.fresh_value();
    func.value_types.insert(first, TirType::DynBox);
    func.value_types.insert(second, TirType::DynBox);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    for (result, index) in [(first, 0), (second, 4)] {
        let mut op = TirOp::new(
            OpCode::ModuleGetGlobal,
            vec![ValueId(0), ValueId(1)],
            vec![result],
        );
        op.set_global_slot(Some(&slot(index)));
        block.ops.push(op);
    }
    block.terminator = Terminator::Return {
        values: vec![second],
    };

    let target = crate::tir::target_info::TargetInfo::native_release_fast();
    assert!(crate::tir::pass_manager::run_global_slot_sites(
        &mut func, &target
    ));
    verify(&func);
    let ops = entry_ops(&func);
    let count = |opcode| ops.iter().filter(|op| op.opcode == opcode).count();
    assert_eq!(count(OpCode::ModuleSlot), 1, "one module identity");
    assert_eq!(count(OpCode::GlobalSlotBase), 1, "one base per activation");
    let base = ops
        .iter()
        .find(|op| op.opcode == OpCode::GlobalSlotBase)
        .unwrap()
        .results[0];
    let gets: Vec<_> = ops
        .iter()
        .filter(|op| op.opcode == OpCode::GlobalSlotGet)
        .collect();
    assert_eq!(gets.len(), 2, "both reads stay: each may run Python");
    // GVN leaves a merged value as a Copy of its leader.
    let leader = |mut value: ValueId| {
        while let Some(copy) = ops
            .iter()
            .find(|op| op.opcode == OpCode::Copy && op.results == [value])
        {
            value = copy.operands[0];
        }
        value
    };
    assert!(
        gets.iter().all(|get| leader(get.operands[0]) == base),
        "{ops:#?}"
    );
}
