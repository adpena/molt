use super::*;

#[test]
fn trivial_const_return() {
    let func = make_const_return_func(42);
    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, vec![]);
    assert_eq!(output.result_types, vec![ValType::I64]);

    // Should contain i64.const 42 somewhere.
    let has_const = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64Const(42)));
    assert!(has_const, "expected i64.const 42 in output");

    // Should end with `end`.
    assert!(matches!(output.instructions.last(), Some(Instruction::End)));
}

#[test]
#[should_panic(expected = "WASM const policy const requires int scalar payload")]
fn lir_const_int_missing_payload_fails_closed() {
    let func = make_scalar_const_return_func(
        "bad_const_int",
        OpCode::ConstInt,
        TirType::I64,
        AttrDict::new(),
    );

    let _ = lower_tir_to_wasm(&func);
}

#[test]
#[should_panic(expected = "WASM const policy const_float requires float scalar payload")]
fn lir_const_float_missing_payload_fails_closed() {
    let func = make_scalar_const_return_func(
        "bad_const_float",
        OpCode::ConstFloat,
        TirType::F64,
        AttrDict::new(),
    );

    let _ = lower_tir_to_wasm(&func);
}

#[test]
#[should_panic(expected = "WASM const policy const_bool requires bool scalar payload")]
fn lir_const_bool_mismatched_payload_fails_closed() {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(1));
    let func =
        make_scalar_const_return_func("bad_const_bool", OpCode::ConstBool, TirType::Bool, attrs);

    let _ = lower_tir_to_wasm(&func);
}

#[test]
fn lir_literal_consts_materialize_without_generic_bail() {
    let cases = [
        {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("hello".into()));
            (
                "const_str_literal",
                OpCode::ConstStr,
                TirType::Str,
                attrs,
                "string_from_bytes",
            )
        },
        {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "s_value".into(),
                AttrValue::Str("9223372036854775808".into()),
            );
            (
                "const_bigint_literal",
                OpCode::ConstBigInt,
                TirType::DynBox,
                attrs,
                "bigint_from_str",
            )
        },
        {
            let mut attrs = AttrDict::new();
            attrs.insert("bytes".into(), AttrValue::Bytes(vec![0, 1, 2, 255]));
            (
                "const_bytes_literal",
                OpCode::ConstBytes,
                TirType::Bytes,
                attrs,
                "bytes_from_bytes",
            )
        },
    ];

    for (name, opcode, return_type, attrs, import_name) in cases {
        let output = lower_tir_to_wasm(&make_scalar_const_return_func(
            name,
            opcode,
            return_type,
            attrs,
        ))
        .test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must materialize in the LIR fast body instead of bailing"
        );
        assert_eq!(output.bail_to_generic_reason, None);
        assert!(
            output.runtime_calls.contains(&import_name),
            "{name} must call {import_name}; got {:?}",
            output.runtime_calls
        );
        assert!(
            output.locals.len() >= 3,
            "{name} must declare result plus ptr/len scratch locals for materialization"
        );
    }
}

#[test]
#[should_panic(expected = "generated WASM const policy requires a result for ConstStr")]
fn lir_literal_const_without_result_fails_closed() {
    let mut func = TirFunction::new("bad_const_str".into(), vec![], TirType::None);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str("orphan".into()));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let _ = lower_tir_to_wasm(&func);
}

#[test]
fn binding_alias_copy_retains_before_forwarding_bits() {
    let mut func = TirFunction::new(
        "binding_alias_copy".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let alias = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![ValueId(0)],
        results: vec![alias],
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "_original_kind".into(),
                AttrValue::Str("binding_alias".into()),
            );
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![alias],
    };

    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output.runtime_calls.contains(&"inc_ref_obj"),
        "binding_alias Copy must retain its forwarded source: {:?}",
        output.runtime_calls
    );
}
