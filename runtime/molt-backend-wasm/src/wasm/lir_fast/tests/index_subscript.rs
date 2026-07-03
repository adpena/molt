use super::*;

#[test]
fn dynbox_index_store_and_delete_stay_lir_fast_runtime_calls() {
    let mut index_func = TirFunction::new(
        "index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let index_result = index_func.fresh_value();
    index_func.value_types.insert(index_result, TirType::DynBox);
    let entry = index_func.blocks.get_mut(&index_func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![index_result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![index_result],
    };

    let output = lower_tir_to_wasm(&index_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"index"),
        "DynBox index must dispatch through the boxed index helper; got {:?}",
        output.runtime_calls
    );

    let mut store_func = TirFunction::new(
        "store_index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let entry = store_func.blocks.get_mut(&store_func.entry_block).unwrap();
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

    let output = lower_tir_to_wasm(&store_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox store_index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"store_index"),
        "DynBox store_index must dispatch through the boxed store helper; got {:?}",
        output.runtime_calls
    );

    let mut del_func = TirFunction::new(
        "del_index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let entry = del_func.blocks.get_mut(&del_func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelIndex,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };

    let output = lower_tir_to_wasm(&del_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox del_index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"del_index"),
        "DynBox del_index must dispatch through the boxed delete helper; got {:?}",
        output.runtime_calls
    );
}

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

#[test]
fn build_slice_stays_lir_fast_and_pads_missing_bounds_with_none() {
    let mut func = TirFunction::new(
        "build_slice_missing_step".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildSlice,
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
        "BuildSlice must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"slice_new"),
        "BuildSlice must call the slice_new ABI helper; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Const(bits) if *bits == box_none_bits())),
        "BuildSlice must pad a missing step operand with the ABI None bits"
    );
}

#[test]
fn ord_at_stays_lir_fast_with_boxed_result_carrier() {
    let mut func = TirFunction::new(
        "ord_at_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::OrdAt,
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
        "OrdAt must stay in the LIR fast lane after boxed carrier authority is fixed"
    );
    assert!(
        output.runtime_calls.contains(&"ord_at"),
        "OrdAt must call the ord_at ABI helper; got {:?}",
        output.runtime_calls
    );
}

#[test]
fn raw_index_result_refuses_boxed_runtime_bits() {
    let mut func = TirFunction::new(
        "raw_index_result".into(),
        vec![TirType::DynBox, TirType::I64],
        TirType::I64,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::I64);
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

    let repr = HashMap::from([
        (ValueId(0), Repr::DynBox),
        (ValueId(1), Repr::RawI64Safe),
        (result_id, Repr::RawI64Safe),
    ]);
    let vr = crate::representation_plan::value_range_for(&func);
    let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
    let output = lower_lir_to_wasm(&lir).test_view();

    assert!(
        output.bails_to_generic_path,
        "raw index result must not store boxed runtime bits into an I64 carrier"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
}
