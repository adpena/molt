use super::super::*;

#[test]
fn dynbox_iterator_helpers_stay_lir_fast_runtime_calls() {
    let cases = [
        ("get_iter_dynbox", OpCode::GetIter, "iter"),
        ("iter_next_dynbox", OpCode::IterNext, "iter_next"),
    ];

    for (name, opcode, runtime_call) in cases {
        let mut func = TirFunction::new(name.into(), vec![TirType::DynBox], TirType::DynBox);
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}
