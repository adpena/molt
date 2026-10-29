use super::*;
use crate::tir::blocks::Terminator;
use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;

#[test]
fn wasm_reachability_never_rewrites_existing_release_custody() {
    let target = TargetInfo::wasm_release_fast();
    let mut function = TirFunction::new(
        "wasm_terminal_custody".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let dead = function.fresh_value();
    let block = function.blocks.get_mut(&function.entry_block).unwrap();
    block.ops.push(TirOp {
        origin: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![dead],
        attrs: AttrDict::from([("value".into(), AttrValue::Int(123))]),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };
    crate::tir::type_refine::refine_types(&mut function);
    for marker in [
        DROP_INSERTED_ATTR,
        super::super::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR,
    ] {
        let mut finalized = function.clone();
        finalized.attrs.insert(marker.into(), AttrValue::Bool(true));
        let before = crate::tir::serialize::serialize_tir_function(&finalized).unwrap();
        assert!(!prepare_final_operation_reachability(
            &mut finalized,
            &target
        ));
        assert_eq!(
            crate::tir::serialize::serialize_tir_function(&finalized).unwrap(),
            before,
            "{marker}: an idempotent release pass cannot repair a mutated finalized body"
        );
    }
    // Negative control: the same unused scalar is removed while the owner is
    // still mutable, so unchanged finalized bytes are not a no-op-pass oracle.
    assert!(prepare_final_operation_reachability(&mut function, &target));
    assert!(
        function
            .blocks
            .values()
            .all(|block| block.ops.iter().all(|op| !op.results.contains(&dead)))
    );
}
