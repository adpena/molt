use molt_ir::FunctionIR;

#[test]
fn compiler_bootstrap_fixture_has_object_return_linkage_on_every_exit() {
    // Python compares this serialized fixture against the real wrapper builder.
    // Consume it through the native backend's exact FunctionIR ABI authority.
    let function: FunctionIR = serde_json::from_str(include_str!(
        "../../../tests/fixtures/bootstrap_return_abi.json"
    ))
    .unwrap();
    assert_eq!(function.name, "molt_isolate_bootstrap");
    assert!(function.function_signature().unwrap().returns_value);
    assert_eq!(
        function
            .ops
            .iter()
            .filter(|op| op.kind == molt_ir::OpKind::Ret)
            .count(),
        2
    );
    assert!(
        !function
            .ops
            .iter()
            .any(|op| op.kind == molt_ir::OpKind::RetVoid)
    );
    for returned in function
        .ops
        .iter()
        .filter(|op| op.kind == molt_ir::OpKind::Ret)
    {
        let result = returned.args.as_ref().unwrap().first().unwrap();
        assert!(
            function
                .ops
                .iter()
                .any(|op| op.kind == molt_ir::OpKind::ConstNone && op.out.as_ref() == Some(result))
        );
    }
}
