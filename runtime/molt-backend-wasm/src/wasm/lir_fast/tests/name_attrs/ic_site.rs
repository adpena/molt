use super::super::*;

#[test]
fn generic_literal_name_attrs_preserve_boxed_receivers_and_source_site_id() {
    for kind in ["get_attr_generic_ptr", "get_attr_generic_obj"] {
        for receiver_type in [TirType::DynBox, TirType::I64] {
            let func_name = "generic_obj_literal_name_attr";
            let source_op_idx = 23usize;
            let mut func = TirFunction::new(
                func_name.into(),
                vec![receiver_type.clone()],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let result_id = func.fresh_value();
            func.value_types.insert(result_id, TirType::DynBox);
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::LoadAttr,
                operands: vec![ValueId(0)],
                results: vec![result_id],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("_original_kind".into(), AttrValue::Str(kind.into()));
                    m.insert("name".into(), AttrValue::Str("field".into()));
                    m.insert(
                        "_source_op_idx".into(),
                        AttrValue::Int(source_op_idx as i64),
                    );
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
                "{kind} must stay in the LIR fast lane when source op identity is carried"
            );
            assert_eq!(
                output
                    .runtime_calls
                    .iter()
                    .filter(|&&call| call == "get_attr_object_ic")
                    .count(),
                1,
                "{kind} must perform one boxed lookup"
            );
            assert!(
                output.data_ptr_i32_literals.contains(&b"field".to_vec()),
                "{kind} must carry the literal name as a LIR data pointer; got {:?}",
                output.data_ptr_i32_literals
            );
            let expected_site_bits =
                box_int_bits(stable_ic_site_id(func_name, source_op_idx, kind));
            let has_site = output.instructions.iter().any(
                |inst| matches!(inst, Instruction::I64Const(bits) if *bits == expected_site_bits),
            );
            assert!(
                has_site,
                "{kind} must use source op identity for the IC site id"
            );
            for pointer_only in ["handle_resolve", "get_attr_ptr"] {
                assert!(
                    !output.runtime_calls.contains(&pointer_only),
                    "{kind} must not resolve a boxed receiver as a pointer"
                );
            }
            if receiver_type == TirType::I64 {
                assert!(
                    output.runtime_calls.contains(&"int_from_i64"),
                    "full-width raw integers must be boxed before lookup"
                );
            }
        }
    }
}
