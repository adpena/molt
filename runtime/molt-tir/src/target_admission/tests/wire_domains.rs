use super::*;

#[test]
fn runtime_aliases_and_typed_siblings_cannot_escape_existing_capabilities() {
    for kind in [
        "load",
        "guarded_load",
        "store",
        "string_eq",
        "index_set",
        "del_attr",
        "build_tuple",
        "build_set",
        "build_slice",
        "object_new_bound",
        "call_builtin",
        "call_method_ic",
        "call_super_method_ic",
        "yield",
        "yield_from",
        "state_set",
        "state_block_start",
        "state_block_end",
        "is_pending",
        "task_wait",
        "exception_pending",
        "function_defaults_version",
        "get_iter",
        "for_iter_start",
        "for_iter_end",
        "async_for_start",
        "async_for_end",
        "free",
        "del_boundary",
        "delete_var",
        "gpu_thread_id",
        "gpu_barrier",
    ] {
        let ir = function_ir(vec![OpIR {
            kind: kind.into(),
            ..OpIR::default()
        }]);
        let error = validate_runtime_target_contract(&ir, &TargetInfo::rust_release_fast())
            .expect_err(kind);
        assert!(
            error.contains("before source generation"),
            "{kind}: {error}"
        );
        for target in [
            TargetInfo::native_release_fast(),
            TargetInfo::wasm_release_fast(),
            TargetInfo::llvm_release_fast(),
        ] {
            validate_runtime_target_contract(&ir, &target).expect(kind);
        }
    }
}

#[test]
fn canonical_integer_literal_aliases_share_exact_admission() {
    for kind in ["const", "const_int", "load_const"] {
        let op = OpIR {
            kind: kind.into(),
            value: Some(-7),
            out: Some("value".into()),
            ..OpIR::default()
        };
        assert_eq!(exact_integer_literal_value(&op, 7), Some(-7));
        assert_eq!(exact_integer_literal_value(&op, 6), None);
        let ir = function_ir(vec![op]);
        assert!(
            validate_numeric_target_contract(&ir, &TargetInfo::rust_release_fast()).is_err(),
            "{kind}"
        );
        validate_numeric_target_contract(&ir, &TargetInfo::native_release_fast()).expect(kind);
    }
}
