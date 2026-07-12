use super::super::*;

#[test]
fn typed_dict_and_tuple_index_select_specialized_runtime_calls() {
    let cases = [
        (
            "dict_index",
            TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox)),
            "dict_getitem",
        ),
        (
            "tuple_index",
            TirType::Tuple(vec![TirType::DynBox]),
            "tuple_getitem",
        ),
    ];

    for (name, container_type, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![container_type, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Index,
            operands: vec![ValueId(0), ValueId(1)],
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
            "{name} must use {runtime_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&"index"),
            "{name} must not fall back to the generic index helper"
        );
    }
}
