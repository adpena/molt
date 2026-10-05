use super::*;

#[test]
fn checked_singletons_preserve_repr_alias_identity_and_versioned_truth() {
    let mut ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![
                OpIR {
                    kind: "const_not_implemented".into(),
                    out: Some("singleton".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "identity_alias".into(),
                    args: Some(vec!["singleton".into()]),
                    out: Some("identity".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "binding_alias".into(),
                    args: Some(vec!["identity".into()]),
                    out: Some("binding".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_ellipsis".into(),
                    out: Some("ellipsis".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "repr_from_obj".into(),
                    args: Some(vec!["binding".into()]),
                    out: Some("representation".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "str_from_obj".into(),
                    args: Some(vec!["singleton".into()]),
                    out: Some("text".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_internal".into(),
                    s_value: Some("molt_bool".into()),
                    args: Some(vec!["binding".into()]),
                    out: Some("truth".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "print".into(),
                    args: Some(vec![
                        "representation".into(),
                        "text".into(),
                        "ellipsis".into(),
                        "truth".into(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    // The version setter is private to its provider unless a guest uses it.
    // Exercise that emitted consumer instead of reaching into private storage.
    let version_params: Vec<String> = ["major", "minor", "micro", "level", "serial", "version"]
        .into_iter()
        .map(str::to_string)
        .collect();
    ir.functions.push(FunctionIR {
        name: "set_singleton_target_version".into(),
        return_abi: molt_ir::FunctionReturnAbi::Void,
        params: version_params.clone(),
        ops: vec![
            OpIR {
                kind: "call_internal".into(),
                s_value: Some("molt_sys_set_version_info".into()),
                args: Some(version_params),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            },
        ],
        ..FunctionIR::default()
    });
    let mut source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("neutral singleton and alias wires");
    source.push_str(
        r#"
assert(molt_str(molt_not_implemented) == "NotImplemented")
assert(molt_repr(molt_not_implemented) == "NotImplemented")
assert(molt_str(molt_ellipsis) == "Ellipsis")
assert(molt_repr(molt_ellipsis) == "Ellipsis")
assert(molt_not_implemented ~= molt_ellipsis)
assert(molt_equal(molt_not_implemented, molt_not_implemented))
assert(not molt_equal(molt_not_implemented, molt_ellipsis))
for _, minor in {12, 13} do
    set_singleton_target_version(3, minor, 0, "final", 0, nil)
    assert(molt_bool(molt_not_implemented))
    assert(molt_bool(molt_ellipsis))
end
set_singleton_target_version(3, 14, 0, "final", 0, nil)
local ok, err = pcall(function() return molt_bool(molt_not_implemented) end)
assert(not ok and err.__type == "TypeError")
assert(err.__msg == "NotImplemented should not be used in a boolean context")
assert(molt_bool(molt_ellipsis))
print("singleton values preserved")
"#,
    );
    let output = execute_lune_oracle("singleton_values", &source);
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 singleton witness");
    assert!(
        stdout.contains("NotImplemented NotImplemented Ellipsis True"),
        "{stdout}"
    );
    assert!(stdout.contains("singleton values preserved"), "{stdout}");
}

#[test]
fn checked_integer_alias_uses_the_same_bounded_literal_admission() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![
                OpIR {
                    kind: "load_const".into(),
                    value: Some(7),
                    out: Some("value".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "print".into(),
                    args: Some(vec!["value".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = LuauBackend::new()
        .compile_checked(&ir)
        .expect("bounded canonical integer alias");
    let output = execute_lune_oracle("literal_alias", &source);
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "7");
}
