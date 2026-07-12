use super::super::*;

#[test]
fn typed_index_store_without_manifest_selector_rows_use_generic_runtime_calls() {
    let index_cases = [
        (
            "list_index_generic",
            TirType::List(Box::new(TirType::DynBox)),
        ),
        ("set_index_generic", TirType::Set(Box::new(TirType::DynBox))),
        ("str_index_generic", TirType::Str),
    ];

    for (name, container_type) in index_cases {
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
            output.runtime_calls.contains(&"index"),
            "{name} must use generic index; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output
                .runtime_calls
                .iter()
                .any(|call| matches!(*call, "dict_getitem" | "tuple_getitem" | "list_int_getitem")),
            "{name} must not use an unsupported specialized index helper"
        );
    }

    let store_cases = [
        (
            "list_store_generic",
            TirType::List(Box::new(TirType::DynBox)),
        ),
        ("set_store_generic", TirType::Set(Box::new(TirType::DynBox))),
        ("tuple_store_generic", TirType::Tuple(vec![TirType::DynBox])),
        ("str_store_generic", TirType::Str),
    ];

    for (name, container_type) in store_cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![container_type, TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::StoreIndex,
            operands: vec![ValueId(0), ValueId(1), ValueId(2)],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&"store_index"),
            "{name} must use generic store_index; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output
                .runtime_calls
                .iter()
                .any(|call| matches!(*call, "dict_setitem" | "list_int_setitem")),
            "{name} must not use an unsupported specialized store helper"
        );
    }
}
