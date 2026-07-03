use super::*;

#[test]
fn heap_alloc_stays_lir_fast_through_immediate_runtime_call() {
    let mut func = TirFunction::new("heap_alloc".into(), vec![], TirType::DynBox);
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Alloc,
        operands: vec![],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(32));
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
        "ordinary heap Alloc must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"alloc"),
        "heap Alloc must call alloc; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Const(32))),
        "heap Alloc must pass its size attr as an immediate"
    );
}

#[test]
fn arena_eligible_alloc_stays_explicit_generic_fallback() {
    let mut func = TirFunction::new("arena_alloc".into(), vec![], TirType::DynBox);
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Alloc,
        operands: vec![],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(32));
            m.insert("arena_eligible".into(), AttrValue::Bool(true));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.bails_to_generic_path,
        "arena-eligible Alloc must not be heap-lowered until LIR owns arena locals"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
    assert!(
        !output.runtime_calls.contains(&"alloc"),
        "arena-eligible Alloc must not silently fall back to heap alloc"
    );
}

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

#[test]
fn closure_offset_ops_stay_lir_fast_runtime_calls() {
    let cases = [
        ("closure_load", OpCode::ClosureLoad, 1, true, "closure_load"),
        (
            "closure_store",
            OpCode::ClosureStore,
            2,
            false,
            "closure_store",
        ),
    ];

    for (name, opcode, operand_count, has_result, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            if has_result {
                TirType::DynBox
            } else {
                TirType::None
            },
        );
        let result_id = has_result.then(|| {
            let id = func.fresh_value();
            func.value_types.insert(id, TirType::DynBox);
            id
        });
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
            results: result_id.into_iter().collect(),
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(16));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: result_id.into_iter().collect(),
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&"handle_resolve"),
            "{name} must resolve closure object bits before offset access; got {:?}",
            output.runtime_calls
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            output
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::I64Const(16))),
            "{name} must pass the closure offset attr as an immediate"
        );
    }
}

#[test]
fn aggregate_builders_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            "build_list",
            OpCode::BuildList,
            3,
            vec![
                ("list_builder_new", 1),
                ("list_builder_append", 3),
                ("list_builder_finish", 1),
            ],
        ),
        (
            "build_tuple",
            OpCode::BuildTuple,
            2,
            vec![
                ("list_builder_new", 1),
                ("list_builder_append", 2),
                ("tuple_builder_finish", 1),
            ],
        ),
        (
            "build_dict",
            OpCode::BuildDict,
            4,
            vec![("dict_new", 1), ("dict_set", 2)],
        ),
        (
            "build_set",
            OpCode::BuildSet,
            3,
            vec![("set_new", 1), ("set_add", 3)],
        ),
    ];

    for (name, opcode, operand_count, expected_calls) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
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
        for (runtime_call, expected_count) in expected_calls {
            let actual_count = output
                .runtime_calls
                .iter()
                .filter(|&&call| call == runtime_call)
                .count();
            assert_eq!(
                actual_count, expected_count,
                "{name} runtime call {runtime_call} count mismatch: {:?}",
                output.runtime_calls
            );
        }
    }
}
