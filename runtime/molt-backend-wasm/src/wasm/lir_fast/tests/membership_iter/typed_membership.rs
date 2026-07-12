use super::super::*;

#[test]
fn typed_membership_selects_specialized_contains_runtime_calls() {
    let cases = [
        (
            "dict_contains",
            TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox)),
            "dict_contains",
        ),
        (
            "list_contains",
            TirType::List(Box::new(TirType::DynBox)),
            "list_contains",
        ),
        (
            "set_contains",
            TirType::Set(Box::new(TirType::DynBox)),
            "set_contains",
        ),
        ("str_contains", TirType::Str, "str_contains"),
    ];

    for (name, container_type, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![container_type, TirType::DynBox],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::Bool);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::In,
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
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&"contains"),
            "{name} must not fall back to the generic contains helper"
        );
    }
}
