use super::super::*;

#[test]
fn object_new_bound_stays_lir_fast_and_selects_sized_helper_from_payload_attr() {
    let cases = [
        (
            "object_new_bound_unsized",
            None,
            "object_new_bound",
            "object_new_bound_sized",
        ),
        (
            "object_new_bound_sized",
            Some(24),
            "object_new_bound_sized",
            "object_new_bound",
        ),
    ];

    for (name, payload_size, expected_call, absent_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox],
            TirType::UserClass("Point".into()),
        );
        let result_id = func.fresh_value();
        func.value_types
            .insert(result_id, TirType::UserClass("Point".into()));
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![ValueId(0)],
            results: vec![result_id],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("_type_hint".into(), AttrValue::Str("Point".into()));
                if let Some(size) = payload_size {
                    m.insert("value".into(), AttrValue::Int(size));
                }
                m
            },
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
            output.runtime_calls.contains(&expected_call),
            "{name} must call {expected_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&absent_call),
            "{name} must not also call {absent_call}; got {:?}",
            output.runtime_calls
        );
    }
}
