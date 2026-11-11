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
