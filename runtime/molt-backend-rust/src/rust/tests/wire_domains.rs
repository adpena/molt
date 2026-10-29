use super::*;

fn checked_program(ops: Vec<OpIR>) -> SimpleIR {
    SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops,
            ..FunctionIR::default()
        }],
        profile: None,
    }
}

#[test]
fn checked_unary_transport_and_phase_markers_emit_real_source() {
    let mut ops = vec![OpIR {
        f_value: Some(-0.0),
        out: Some("input".into()),
        ..OpIR::new(molt_ir::OpKind::ConstFloat)
    }];
    for (kind, source, output) in [
        (molt_ir::OpKind::Pos, "input", "positive"),
        (molt_ir::OpKind::UnaryPos, "positive", "alias_positive"),
        (molt_ir::OpKind::TypeGuard, "alias_positive", "guarded"),
    ] {
        ops.push(OpIR {
            args: Some(vec![source.into()]),
            out: Some(output.into()),
            ..OpIR::new(kind)
        });
    }
    for kind in [
        molt_ir::OpKind::LoopIndexEnd,
        molt_ir::OpKind::DropInserted,
        molt_ir::OpKind::ExceptionRegionDropsInserted,
    ] {
        ops.push(OpIR::new(kind));
    }
    // A value function lets an independent emitted consumer inspect sign bits.
    let mut ir = checked_program(vec![OpIR::new(molt_ir::OpKind::RetVoid)]);
    ops.push(OpIR {
        args: Some(vec!["guarded".into()]),
        ..OpIR::new(molt_ir::OpKind::Ret)
    });
    ir.functions.push(FunctionIR {
        name: "transport".into(),
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops,
        ..FunctionIR::default()
    });
    let source = RustBackend::new()
        .compile_checked(&ir)
        .expect("admitted scalar transport");
    let mut source = source.replacen("fn main() {", "fn main() { check_transport();", 1);
    source.push_str(
        r#"
fn check_transport() {
    let MoltValue::Float(value) = transport(&mut vec![]) else { panic!("expected float") };
    assert_eq!(value.to_bits(), (-0.0f64).to_bits());
    let text = PythonString::from_code_points(&[0xd800, 0xdc00, 0x1f600]);
    assert_eq!(text.to_utf8_backslashreplace(), "\\ud800\\udc00😀");
    println!("admitted transport preserved");
}
"#,
    );
    assert_eq!(
        compile_and_run_emitted(&source, "wire_domain_transport").trim(),
        "admitted transport preserved"
    );
}

#[test]
fn scalar_and_copy_shape_failures_are_rejected_by_shared_admission() {
    for op in [
        OpIR {
            out: Some("bad".into()),
            ..OpIR::new(molt_ir::OpKind::ConstFloat)
        },
        OpIR {
            out: Some("bad".into()),
            ..OpIR::new(molt_ir::OpKind::ConstBool)
        },
        OpIR {
            out: Some("bad".into()),
            ..OpIR::new(molt_ir::OpKind::Copy)
        },
        OpIR {
            out: Some("bad".into()),
            ..OpIR::new(molt_ir::OpKind::BindingAlias)
        },
        OpIR {
            var: Some("none".into()),
            ..OpIR::new(molt_ir::OpKind::StoreVar)
        },
        OpIR {
            out: Some("unexpected".into()),
            ..OpIR::new(molt_ir::OpKind::Nop)
        },
        OpIR {
            out: Some("unexpected".into()),
            ..OpIR::new(molt_ir::OpKind::DropInserted)
        },
        OpIR {
            var: Some("slot".into()),
            ..OpIR::new(molt_ir::OpKind::StoreVar)
        },
        OpIR::new(molt_ir::OpKind::WarnStderr),
    ] {
        let error = RustBackend::new()
            .compile_checked(&checked_program(vec![op]))
            .expect_err("malformed input");
        assert!(error.contains("SimpleIR validation failed"), "{error}");
        assert!(!error.contains("unsupported Rust backend op"), "{error}");
    }
}

#[test]
fn shared_text_helpers_do_not_collide_with_user_function_names() {
    let mut ir = checked_program(vec![OpIR::new(molt_ir::OpKind::RetVoid)]);
    for name in [
        "PythonString",
        "PythonCodePoints",
        "decode_python_code_point",
    ] {
        ir.functions.push(FunctionIR {
            name: name.into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            ops: vec![
                OpIR {
                    s_value: Some(name.into()),
                    out: Some("text".into()),
                    ..OpIR::new(molt_ir::OpKind::ConstStr)
                },
                OpIR {
                    args: Some(vec!["text".into()]),
                    ..OpIR::new(molt_ir::OpKind::Ret)
                },
            ],
            ..FunctionIR::default()
        });
    }
    let mut source = RustBackend::new()
        .compile_checked(&ir)
        .expect("user-owned function namespace");
    source = source.replacen("fn main() {", "fn main() { check_function_names();", 1);
    source.push_str(r#"
fn check_function_names() {
    for (value, expected) in [(PythonString(&mut vec![]), "PythonString"), (PythonCodePoints(&mut vec![]), "PythonCodePoints"), (decode_python_code_point(&mut vec![]), "decode_python_code_point")] {
        let MoltValue::Str(text) = value else { panic!("expected text") };
        assert_eq!(text.to_utf8().unwrap(), expected);
    }
    println!("function namespace preserved");
}
"#);
    assert_eq!(
        compile_and_run_emitted(&source, "text_namespace").trim(),
        "function namespace preserved"
    );
}
