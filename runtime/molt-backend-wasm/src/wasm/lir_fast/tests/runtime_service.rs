use super::*;

#[test]
fn exception_pending_stays_lir_fast_with_bool_result_adapter() {
    let mut func = TirFunction::new("exception_pending".into(), vec![], TirType::Bool);
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::Bool);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ExceptionPending,
        operands: vec![],
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
        "exception_pending must stay in the LIR fast lane"
    );
    assert_eq!(output.result_types, vec![ValType::I32]);
    assert!(
        output.runtime_calls.contains(&"exception_pending"),
        "exception_pending must call the typed runtime import; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Ne)),
        "exception_pending raw i64 flag must be adapted to a Bool1 result"
    );
}

#[test]
fn fixed_runtime_service_and_module_ops_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            "function_defaults_version",
            OpCode::FunctionDefaultsVersion,
            1,
            true,
            "function_defaults_version",
        ),
        ("module_import", OpCode::Import, 1, true, "module_import"),
        (
            "module_cache_get",
            OpCode::ModuleCacheGet,
            1,
            true,
            "module_cache_get",
        ),
        (
            "module_cache_set",
            OpCode::ModuleCacheSet,
            2,
            false,
            "module_cache_set",
        ),
        (
            "module_cache_del",
            OpCode::ModuleCacheDel,
            1,
            false,
            "module_cache_del",
        ),
        (
            "module_get_attr",
            OpCode::ModuleGetAttr,
            2,
            true,
            "module_get_attr",
        ),
        (
            "module_import_from",
            OpCode::ModuleImportFrom,
            2,
            true,
            "module_import_from",
        ),
        (
            "module_get_global",
            OpCode::ModuleGetGlobal,
            2,
            true,
            "module_get_global",
        ),
        (
            "module_get_name",
            OpCode::ModuleGetName,
            2,
            true,
            "module_get_name",
        ),
        (
            "module_set_attr",
            OpCode::ModuleSetAttr,
            3,
            false,
            "module_set_attr",
        ),
        (
            "module_del_global",
            OpCode::ModuleDelGlobal,
            2,
            false,
            "module_del_global",
        ),
        (
            "module_del_global_if_present",
            OpCode::ModuleDelGlobalIfPresent,
            2,
            false,
            "module_del_global_if_present",
        ),
    ];

    for (name, opcode, operand_count, has_result, runtime_call) in cases {
        let func = make_fixed_runtime_service_func(name, opcode, operand_count, has_result);
        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
        assert_eq!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Drop)),
            !has_result,
            "{name} must drop the runtime sentinel exactly when TIR has no result"
        );
    }
}

#[test]
fn preserved_copy_runtime_service_imports_stay_lir_fast_runtime_calls() {
    let cases = [
        ("module_new", "module_new", 1, true, "module_new"),
        (
            "module_import_star",
            "module_import_star",
            2,
            true,
            "module_import_star",
        ),
        (
            "bridge_unavailable",
            "bridge_unavailable",
            1,
            true,
            "bridge_unavailable",
        ),
        ("context_null", "context_null", 1, true, "context_null"),
        ("context_enter", "context_enter", 1, true, "context_enter"),
        ("context_exit", "context_exit", 2, true, "context_exit"),
        (
            "context_unwind",
            "context_unwind",
            1,
            true,
            "context_unwind",
        ),
        ("context_depth", "context_depth", 0, true, "context_depth"),
        (
            "context_unwind_to",
            "context_unwind_to",
            2,
            true,
            "context_unwind_to",
        ),
        (
            "context_closing",
            "context_closing",
            1,
            true,
            "context_closing",
        ),
    ];

    for (name, original_kind, operand_count, has_result, runtime_call) in cases {
        let func =
            make_copy_original_kind_runtime_func(name, original_kind, operand_count, has_result);
        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} preserved Copy runtime service must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn unsupported_preserved_copy_runtime_service_bails_instead_of_aliasing_operand() {
    let func = make_copy_original_kind_runtime_func(
        "exception_new_builtin_empty",
        "exception_new_builtin_empty",
        0,
        true,
    );
    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.bails_to_generic_path,
        "unsupported preserved Copy runtime service must fail closed to generic emission"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
    assert!(
        !output
            .runtime_calls
            .contains(&"exception_new_builtin_empty"),
        "unsupported preserved Copy runtime service must not fake a partial LIR runtime call"
    );
}
