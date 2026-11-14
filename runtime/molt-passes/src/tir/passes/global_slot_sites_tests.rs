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
        cell: false,
        init_once: false,
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

fn cell(index: u32) -> GlobalSlotRef {
    GlobalSlotRef {
        cell: true,
        ..slot(index)
    }
}

#[test]
fn a_cell_read_reads_the_static_block_and_a_cell_store_makes_no_call() {
    let mut func = function();
    let read = func.fresh_value();
    func.value_types.insert(read, TirType::DynBox);
    let mut get = TirOp::new(
        OpCode::ModuleGetGlobal,
        vec![ValueId(0), ValueId(1)],
        vec![read],
    );
    get.set_global_slot(Some(&cell(4)));
    let mut module_store = TirOp::new(
        OpCode::ModuleSetAttr,
        vec![ValueId(0), ValueId(1), ValueId(2)],
        vec![],
    );
    module_store.set_global_slot(Some(&cell(1)));
    let dict_result = func.fresh_value();
    func.value_types.insert(dict_result, TirType::DynBox);
    let mut frame_store = TirOp::new(
        OpCode::Copy,
        vec![ValueId(0), ValueId(1), ValueId(2)],
        vec![dict_result],
    );
    frame_store.origin = Some(molt_ir::OpKind::DictSet);
    frame_store.set_global_slot(Some(&cell(2)));
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.extend([get, module_store, frame_store]);
    block.terminator = Terminator::Return { values: vec![read] };

    super::run(&mut func);
    verify(&func);
    let ops = entry_ops(&func);
    let kinds: Vec<_> = ops.iter().map(|op| op.opcode).collect();
    assert_eq!(
        kinds,
        [
            OpCode::StaticCellBase,
            OpCode::GlobalSlotGet,
            OpCode::StaticCellBase,
            OpCode::StaticCellSet,
            OpCode::StaticCellBase,
            OpCode::StaticCellSet,
        ],
        "no runtime call, no module id, no slot base"
    );
    for base in [&ops[0], &ops[2], &ops[4]] {
        assert_eq!(
            base.attrs.get("s_value"),
            Some(&AttrValue::Str("demo".into()))
        );
    }
    assert_eq!(ops[1].operands, [ops[0].results[0], ValueId(0), ValueId(1)]);
    assert_eq!(ops[1].results, [read]);
    assert_eq!(ops[1].global_slot(), Some(cell(4)));
    for (store, base, index) in [(&ops[3], &ops[2], 1), (&ops[5], &ops[4], 2)] {
        assert_eq!(store.operands, [base.results[0], ValueId(2)]);
        assert!(store.results.is_empty());
        assert_eq!(store.global_slot(), Some(cell(index)));
        assert_eq!(store.origin, None);
    }
}

/// An init-dominated cell read (HF-846) is one load with no operands; a
/// checked cell read beside it keeps its base, its unbound test and its
/// by-name operands.
#[test]
fn an_init_dominated_cell_read_is_one_load_without_operands() {
    let mut func = function();
    let (dominated, checked) = (func.fresh_value(), func.fresh_value());
    func.value_types.insert(dominated, TirType::DynBox);
    func.value_types.insert(checked, TirType::DynBox);
    let once = GlobalSlotRef {
        init_once: true,
        ..cell(5)
    };
    let mut first = TirOp::new(
        OpCode::ModuleGetGlobal,
        vec![ValueId(0), ValueId(1)],
        vec![dominated],
    );
    first.set_global_slot(Some(&once));
    let mut second = TirOp::new(
        OpCode::ModuleGetGlobal,
        vec![ValueId(0), ValueId(1)],
        vec![checked],
    );
    second.set_global_slot(Some(&cell(5)));
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops.extend([
        first,
        second,
        TirOp::new(OpCode::WarnStderr, vec![checked], vec![]),
    ]);
    block.terminator = Terminator::Return {
        values: vec![dominated],
    };

    super::run(&mut func);
    verify(&func);
    let ops = entry_ops(&func);
    let kinds: Vec<_> = ops.iter().map(|op| op.opcode).collect();
    assert_eq!(
        kinds,
        [
            OpCode::StaticCellLoad,
            OpCode::StaticCellBase,
            OpCode::GlobalSlotGet,
            OpCode::WarnStderr,
        ]
    );
    assert!(ops[0].operands.is_empty(), "no base, no name, no module");
    assert_eq!(ops[0].results, [dominated]);
    assert_eq!(ops[0].global_slot(), Some(once));
    assert_eq!(ops[0].origin, None);
    assert_eq!(ops[2].operands, [ops[1].results[0], ValueId(0), ValueId(1)]);
    assert_eq!(ops[2].results, [checked]);
}
