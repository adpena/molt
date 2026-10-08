use super::*;

fn value_function(name: &str, mut ops: Vec<OpIR>, result: &str) -> FunctionIR {
    ops.push(OpIR {
        kind: "ret".into(),
        args: Some(vec![result.into()]),
        ..OpIR::default()
    });
    FunctionIR {
        name: name.into(),
        ops,
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ..FunctionIR::default()
    }
}

#[test]
fn checked_value_literals_preserve_surrogates_float_bits_and_owned_copies() {
    let mut functions = vec![
        value_function(
            "sentinel",
            vec![OpIR {
                kind: "const_not_implemented".into(),
                out: Some("value".into()),
                ..OpIR::default()
            }],
            "value",
        ),
        value_function(
            "text",
            vec![
                OpIR {
                    kind: "const_str".into(),
                    bytes: Some(vec![b'a', 0, 0xed, 0xa0, 0x80, 0xed, 0xbf, 0xbf]),
                    out: Some("original".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "binding_alias".into(),
                    args: Some(vec!["original".into()]),
                    out: Some("owned".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy".into(),
                    args: Some(vec!["owned".into()]),
                    out: Some("result".into()),
                    ..OpIR::default()
                },
            ],
            "result",
        ),
        FunctionIR {
            name: "molt_main".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            }],
            ..FunctionIR::default()
        },
    ];
    for (name, value) in [
        ("neg_zero", -0.0),
        ("pos_inf", f64::INFINITY),
        ("neg_inf", f64::NEG_INFINITY),
        ("nan_value", f64::from_bits(0x7ff8000000001234)),
    ] {
        functions.push(value_function(
            name,
            vec![OpIR {
                kind: "const_float".into(),
                f_value: Some(value),
                out: Some("value".into()),
                ..OpIR::default()
            }],
            "value",
        ));
    }
    let source = RustBackend::new()
        .compile_checked(&SimpleIR {
            functions,
            profile: None,
        })
        .expect("implemented value family must be admitted");
    let mut source = source.replacen("fn main() {", "fn main() { check_values();", 1);
    source.push_str(r#"
fn check_values() {
    let sentinel = sentinel(&mut vec![]);
    assert!(matches!(sentinel, MoltValue::NotImplemented));
    assert_eq!(molt_str(&sentinel), "NotImplemented");
    assert!(sentinel == sentinel.clone());
    assert!(sentinel != MoltValue::Ellipsis && sentinel != MoltValue::None);
    let MoltValue::Str(text) = text(&mut vec![]) else { panic!("expected text") };
    assert_eq!(text.code_points().collect::<Vec<_>>(), [97, 0, 0xd800, 0xdfff]);
    assert_eq!(text.as_surrogatepass_bytes(), &[97, 0, 237, 160, 128, 237, 191, 191]);
    for (value, bits) in [(neg_zero(&mut vec![]), 0x8000000000000000), (pos_inf(&mut vec![]), 0x7ff0000000000000), (neg_inf(&mut vec![]), 0xfff0000000000000), (nan_value(&mut vec![]), 0x7ff8000000001234)] {
        let MoltValue::Float(value) = value else { panic!("expected float") };
        assert_eq!(value.to_bits(), bits);
    }
    println!("checked values preserved");
}
"#);
    assert_eq!(
        compile_and_run_emitted(&source, "checked_value_primitives").trim(),
        "checked values preserved"
    );
}

#[test]
fn emitted_text_consumers_share_lossless_code_point_operations() {
    let body = r##"
fn main() {
    let s = MoltValue::Str(PythonString::from_code_points(&[0xd800, 0, 0x1f600]));
    assert_eq!(molt_str(&s).code_points().collect::<Vec<_>>(), [0xd800, 0, 0x1f600]);
    assert_eq!(molt_len(&s), MoltValue::Int(3));
    assert_eq!(molt_ord(&molt_get_item(&s, &MoltValue::Int(0))), MoltValue::Int(0xd800));
    assert_eq!(molt_ord(&molt_get_item(&s, &MoltValue::Int(-1))), MoltValue::Int(0x1f600));
    assert_eq!(molt_chr(&MoltValue::Int(0xd800)), molt_get_item(&s, &MoltValue::Int(0)));
    assert_eq!(molt_iter_list(&s), molt_unpack_sequence(&s, 3));
    assert_eq!(molt_str(&molt_repr(&s)), "'\\ud800\\x00😀'");
    assert_eq!(molt_str(&molt_ascii_from_obj(&s)), "'\\ud800\\x00\\U0001f600'");
    assert_eq!(molt_add(s.clone(), s.clone()), MoltValue::Str(PythonString::from_code_points(&[0xd800, 0, 0x1f600, 0xd800, 0, 0x1f600])));
    assert!(molt_bool(&MoltValue::Float(f64::NAN)));
    let sentinel = MoltValue::NotImplemented;
    assert!(molt_bool(&sentinel));
    molt_sys_version_state().lock().unwrap().minor = 14;
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| molt_bool(&sentinel))).is_err());
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| molt_print(&[s.clone()]))).is_err());
    println!("text family preserved");
}
"##;
    let mut backend = RustBackend::new();
    backend.emit_header();
    backend.emit_prelude_conditional(body);
    backend.output.push_str(body);
    assert_eq!(
        compile_and_run_emitted(&backend.output, "text_consumer_family").trim(),
        "text family preserved"
    );
}

/// `missing` needs no runtime capability, so every target admits it and the
/// Rust backend must lower it rather than reach its unsupported-op catch-all.
#[test]
fn admitted_missing_sentinel_lowers_to_a_distinct_truthy_singleton() {
    let functions = vec![
        value_function(
            "sentinel",
            vec![
                OpIR {
                    kind: "missing".into(),
                    out: Some("value".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy".into(),
                    args: Some(vec!["value".into()]),
                    out: Some("result".into()),
                    ..OpIR::default()
                },
            ],
            "result",
        ),
        FunctionIR {
            name: "molt_main".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            }],
            ..FunctionIR::default()
        },
    ];
    let source = RustBackend::new()
        .compile_checked(&SimpleIR {
            functions,
            profile: None,
        })
        .expect("the admitted missing sentinel must lower");
    let mut source = source.replacen("fn main() {", "fn main() { check_missing();", 1);
    source.push_str(r#"
fn check_missing() {
    let sentinel = sentinel(&mut vec![]);
    assert!(matches!(sentinel, MoltValue::Missing));
    assert!(sentinel == sentinel.clone());
    for other in [MoltValue::None, MoltValue::Ellipsis, MoltValue::NotImplemented, MoltValue::Bool(false)] {
        assert!(sentinel != other && other != sentinel);
    }
    assert!(molt_bool(&sentinel));
    assert_eq!(molt_str(&sentinel), "<object object>");
    println!("missing sentinel preserved");
}
"#);
    assert_eq!(
        compile_and_run_emitted(&source, "missing_sentinel").trim(),
        "missing sentinel preserved"
    );
}
