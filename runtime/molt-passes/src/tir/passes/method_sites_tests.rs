use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// A function over (receiver, argument) parameters.
fn function() -> TirFunction {
    TirFunction::new(
        "f".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    )
}

fn load(func: &mut TirFunction, opcode: OpCode, operands: Vec<ValueId>) -> (TirOp, [ValueId; 2]) {
    let pair = [func.fresh_value(), func.fresh_value()];
    for value in pair {
        func.value_types.insert(value, TirType::DynBox);
    }
    let mut op = TirOp::new(opcode, operands, pair.to_vec());
    op.attrs.insert("name".into(), AttrValue::Str("m".into()));
    (op, pair)
}

fn call(func: &mut TirFunction, operands: Vec<ValueId>) -> (TirOp, ValueId) {
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    (
        TirOp::new(OpCode::CallLoadedMethod, operands, vec![result]),
        result,
    )
}

#[test]
fn each_load_and_the_call_of_its_whole_pair_share_one_site() {
    let mut func = function();
    let (first, [f1, s1]) = load(&mut func, OpCode::LoadMethod, vec![ValueId(0)]);
    let (second, [f2, s2]) = load(&mut func, OpCode::LoadMethod, vec![ValueId(0)]);
    let (super_load, [f3, s3]) = load(
        &mut func,
        OpCode::LoadSuperMethod,
        vec![ValueId(0), ValueId(1)],
    );
    let (call_second, _) = call(&mut func, vec![f2, s2, ValueId(1)]);
    let (call_first, _) = call(&mut func, vec![f1, s1]);
    let (call_super, last) = call(&mut func, vec![f3, s3]);
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops = vec![
        first,
        second,
        super_load,
        call_second,
        call_first,
        call_super,
    ];
    block.terminator = Terminator::Return { values: vec![last] };

    let stats = super::run(&mut func);
    assert_eq!(stats.values_changed, 4);
    crate::tir::verify::verify_function(&func).expect("numbered function verifies");
    let sites: Vec<_> = func.blocks[&entry]
        .ops
        .iter()
        .map(|op| op.method_site())
        .collect();
    assert_eq!(sites, [Some(0), Some(1), None, Some(1), Some(0), None]);
    // The numbers are stable: a second run changes nothing.
    assert_eq!(super::run(&mut func).values_changed, 0);
    let again: Vec<_> = func.blocks[&entry]
        .ops
        .iter()
        .map(|op| op.method_site())
        .collect();
    assert_eq!(again, sites);
}
