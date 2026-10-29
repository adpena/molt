use super::super::*;

#[test]
fn generic_literal_name_attrs_preserve_boxed_receivers_and_source_site_id() {
    for kind in [
        molt_ir::OpKind::GetAttrGenericPtr,
        molt_ir::OpKind::GetAttrGenericObj,
    ] {
        for (receiver_type, raw_carrier) in [
            (TirType::DynBox, false),
            (TirType::I64, false),
            (TirType::I64, true),
        ] {
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
                origin: Some(kind),
                dialect: Dialect::Molt,
                opcode: OpCode::LoadAttr,
                operands: vec![ValueId(0)],
                results: vec![result_id],
                attrs: {
                    let mut m = AttrDict::new();

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

            // A semantic Python int parameter remains boxed without raw-carrier
            // proof. Exercise the actual full-width carrier separately, using
            // the same representation admission as the integer ABI controls.
            let lir = if raw_carrier {
                let repr = HashMap::from([
                    (ValueId(0), Repr::RawI64FullDeopt),
                    (result_id, Repr::DynBox),
                ]);
                lower_function_to_lir_with_inline_proof(
                    &func,
                    &repr,
                    &crate::representation_plan::value_range_for(&func),
                )
            } else {
                crate::tir::lower_to_lir::lower_function_to_lir(&func)
            };
            assert_eq!(
                lir.blocks[&lir.entry_block].args[0].repr,
                if raw_carrier {
                    crate::tir::lir::LirRepr::I64
                } else {
                    crate::tir::lir::LirRepr::DynBox
                },
                "{kind}: fixture must admit the intended physical receiver"
            );
            let output = lower_lir_to_wasm(&lir).test_view();

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
                box_int_bits(stable_ic_site_id(func_name, source_op_idx, kind.as_str()));
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
            assert_eq!(
                output
                    .runtime_calls
                    .iter()
                    .filter(|&&call| call == "int_from_i64")
                    .count(),
                usize::from(raw_carrier),
                "{kind}: full-width raw integers require one overflow-safe box; already-boxed receivers require none"
            );
            if raw_carrier {
                assert!(
                    output
                        .instructions
                        .iter()
                        .any(|inst| matches!(inst, Instruction::I64LtU)),
                    "{kind}: full-width boxing must retain its inline-range guard"
                );
            }
        }
    }
}

#[test]
fn generic_literal_name_writes_carry_source_site_id() {
    for kind in [
        molt_ir::OpKind::SetAttrGenericPtr,
        molt_ir::OpKind::SetAttrGenericObj,
    ] {
        let func_name = "generic_literal_name_write";
        let source_op_idx = 29usize;
        let mut func = TirFunction::new(
            func_name.into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::None,
            molt_ir::FunctionReturnAbi::Value,
        );
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            origin: Some(kind),
            dialect: Dialect::Molt,
            opcode: OpCode::StoreAttr,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![],
            attrs: {
                let mut m = AttrDict::new();

                m.insert("name".into(), AttrValue::Str("field".into()));
                m.insert(
                    "_source_op_idx".into(),
                    AttrValue::Int(source_op_idx as i64),
                );
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{kind} must stay in the LIR fast lane when source op identity is carried"
        );
        assert_eq!(
            output
                .runtime_calls
                .iter()
                .filter(|&&call| call == "set_attr_object_ic")
                .count(),
            1,
            "{kind} must perform one site write; got {:?}",
            output.runtime_calls
        );
        assert!(
            output.data_ptr_i32_literals.contains(&b"field".to_vec()),
            "{kind} must carry the literal name as a LIR data pointer"
        );
        let expected_site_bits =
            box_int_bits(stable_ic_site_id(func_name, source_op_idx, kind.as_str()));
        assert!(
            output.instructions.iter().any(
                |inst| matches!(inst, Instruction::I64Const(bits) if *bits == expected_site_bits)
            ),
            "{kind} must use source op identity for the IC site id"
        );
    }
}
