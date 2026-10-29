use super::*;

#[test]
fn lune_oracle_checked_singletons_preserve_repr_alias_identity_and_versioned_truth() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![
                OpIR {
                    out: Some("singleton".into()),
                    ..OpIR::new(molt_ir::OpKind::ConstNotImplemented)
                },
                OpIR {
                    args: Some(vec!["singleton".into()]),
                    out: Some("identity".into()),
                    ..OpIR::new(molt_ir::OpKind::IdentityAlias)
                },
                OpIR {
                    args: Some(vec!["identity".into()]),
                    out: Some("binding".into()),
                    ..OpIR::new(molt_ir::OpKind::BindingAlias)
                },
                OpIR {
                    out: Some("ellipsis".into()),
                    ..OpIR::new(molt_ir::OpKind::ConstEllipsis)
                },
                OpIR {
                    args: Some(vec!["binding".into()]),
                    out: Some("representation".into()),
                    ..OpIR::new(molt_ir::OpKind::ReprFromObj)
                },
                OpIR {
                    args: Some(vec!["singleton".into()]),
                    out: Some("text".into()),
                    ..OpIR::new(molt_ir::OpKind::StrFromObj)
                },
                OpIR {
                    s_value: Some("molt_bool".into()),
                    args: Some(vec!["binding".into()]),
                    out: Some("truth".into()),
                    ..OpIR::new(molt_ir::OpKind::CallInternal)
                },
                OpIR {
                    args: Some(vec![
                        "representation".into(),
                        "text".into(),
                        "ellipsis".into(),
                        "truth".into(),
                    ]),
                    ..OpIR::new(molt_ir::OpKind::Print)
                },
                OpIR::new(molt_ir::OpKind::RetVoid),
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    // The version setter is private to its provider unless a guest uses it.
    // Exercise that emitted consumer instead of reaching into private storage.
    let version_params: Vec<String> = ["major", "minor", "micro", "level", "serial"]
        .into_iter()
        .map(str::to_string)
        .collect();
    ir.functions.push(FunctionIR {
        name: "set_singleton_target_version".into(),
        return_abi: molt_ir::FunctionReturnAbi::Void,
        params: version_params.clone(),
        ops: vec![
            OpIR {
                s_value: Some("molt_sys_set_version_info".into()),
                args: Some(version_params),
                ..OpIR::new(molt_ir::OpKind::CallInternal)
            },
            OpIR::new(molt_ir::OpKind::RetVoid),
        ],
        ..FunctionIR::default()
    });
    for minor in [12, 13, 14] {
        let mut source = LuauBackend::new()
            .compile_checked(&ir)
            .expect("neutral singleton and alias wires");
        source.push_str(&format!(
            "\nset_singleton_target_version(3, {minor}, 0, \"final\", 0)\n"
        ));
        source.push_str(
            r#"
assert(molt_str(molt_not_implemented) == "NotImplemented")
assert(molt_repr(molt_not_implemented) == "NotImplemented")
assert(molt_str(molt_ellipsis) == "Ellipsis")
assert(molt_repr(molt_ellipsis) == "Ellipsis")
assert(molt_not_implemented ~= molt_ellipsis)
assert(molt_equal(molt_not_implemented, molt_not_implemented))
assert(not molt_equal(molt_not_implemented, molt_ellipsis))
assert(molt_bool(molt_ellipsis))
"#,
        );
        if minor < 14 {
            source.push_str("assert(molt_bool(molt_not_implemented))\n");
        } else {
            source.push_str(
                r#"local ok, err = pcall(function() return molt_bool(molt_not_implemented) end)
assert(not ok and err.__type == "TypeError")
assert(err.__msg == "NotImplemented should not be used in a boolean context")
"#,
            );
        }
        source.push_str("print(\"singleton values preserved\")\n");
        execute_lune_oracle(
            &format!("singleton_values_3{minor}"),
            &source,
            LuneStdout::Contains(&[
                "NotImplemented NotImplemented Ellipsis True",
                "singleton values preserved",
            ]),
        );
    }
}

#[test]
fn lune_oracle_checked_integer_alias_uses_the_same_bounded_literal_admission() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![
                OpIR {
                    value: Some(7),
                    out: Some("value".into()),
                    ..OpIR::new(molt_ir::OpKind::LoadConst)
                },
                OpIR {
                    args: Some(vec!["value".into()]),
                    ..OpIR::new(molt_ir::OpKind::Print)
                },
                OpIR::new(molt_ir::OpKind::RetVoid),
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("bounded canonical integer alias");
    execute_lune_oracle("literal_alias", &source, LuneStdout::TrimmedExact("7"));
}
