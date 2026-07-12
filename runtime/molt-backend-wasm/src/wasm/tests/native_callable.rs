use super::support::*;

#[test]
fn production_lir_wasm_fast_path_is_reserved_for_global_builtin_lane() {
    assert!(is_production_lir_wasm_fast_path_name(
        "molt_test____molt_globals_builtin__"
    ));
    assert!(!is_production_lir_wasm_fast_path_name(
        "molt_test_regular_helper"
    ));
    assert!(!is_production_lir_wasm_fast_path_name(
        "molt_test_user_callable"
    ));
}

#[test]
fn lir_fast_literal_const_materialization_emits_valid_wasm() {
    let literal_bytes = b"hello wasm literal";
    let mut literal = wasm_test_op("const_str", Some("literal"), vec![]);
    literal.s_value = Some(String::from_utf8(literal_bytes.to_vec()).expect("ascii literal"));
    let mut ret = wasm_test_op("ret", None, vec!["literal"]);
    ret.var = Some("literal".to_string());
    let func = wasm_test_function(
        "m____molt_globals_builtin__literal_const",
        vec![],
        None,
        vec![literal, ret],
    );
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("LIR-fast literal const materialization must emit valid WASM");

    let imports = wasm_function_import_indices(&wasm);
    let string_from_bytes = *imports
        .get("string_from_bytes")
        .unwrap_or_else(|| panic!("string_from_bytes import missing; imports={imports:?}"));
    assert!(
        wasm_direct_call_indices(&wasm).contains(&string_from_bytes),
        "LIR-fast materialization must emit a direct call to string_from_bytes"
    );
    assert!(
        wasm_data_segment_payloads(&wasm)
            .iter()
            .any(|payload| payload.as_slice() == literal_bytes),
        "LIR-fast materialization must write literal bytes into a data segment"
    );
}

#[test]
fn generic_attr_ic_uses_transported_source_op_idx() {
    let source_op_idx = 17;
    let mut load_attr = wasm_test_op("get_attr_generic_obj", Some("value"), vec!["obj"]);
    load_attr.s_value = Some("field".to_string());
    load_attr.source_op_idx = Some(source_op_idx);
    let mut ret = wasm_test_op("ret", None, vec!["value"]);
    ret.var = Some("value".to_string());
    let func = wasm_test_function("generic_attr_ic", vec!["obj"], None, vec![load_attr, ret]);
    let ir = SimpleIR {
        functions: vec![func],
        profile: None,
    };
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile(ir);

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("generic attr IC lowering must emit valid WASM");

    let expected_site_bits = molt_codegen_abi::box_int_bits(molt_codegen_abi::stable_ic_site_id(
        "generic_attr_ic",
        source_op_idx as usize,
        "get_attr_generic_obj",
    ));
    assert!(
        wasm_i64_consts(&wasm).contains(&expected_site_bits),
        "generic WASM attr IC must use transported source_op_idx for site id"
    );

    let imports = wasm_function_import_indices(&wasm);
    let get_attr_object_ic = *imports
        .get("get_attr_object_ic")
        .unwrap_or_else(|| panic!("get_attr_object_ic import missing; imports={imports:?}"));
    assert!(
        wasm_direct_call_indices(&wasm).contains(&get_attr_object_ic),
        "generic WASM attr IC must call get_attr_object_ic"
    );
}

#[test]
fn native_callable_direct_symbol_object_call_imports_and_directly_calls_symbol() {
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(wasm_native_callable_ir("molt.object_call_v1"));

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("native callable direct symbol dispatch must emit valid WASM");

    let native_symbol = "molt_nativepkg_ndimage_distance_transform_edt";
    let import_modules = wasm_function_import_modules(&wasm);
    assert_eq!(
        import_modules.get(native_symbol).map(String::as_str),
        Some("molt_native"),
        "native callable symbols must not be imported through the Molt runtime namespace"
    );

    let import_type_indices = wasm_function_import_type_indices(&wasm);
    assert_eq!(
        import_type_indices.get(native_symbol).copied(),
        Some(2),
        "unary object-call native callable ABI must use boxed (i64) -> i64 type index"
    );

    let import_indices = wasm_function_import_indices(&wasm);
    let native_import_index = *import_indices
        .get(native_symbol)
        .unwrap_or_else(|| panic!("{native_symbol} import missing; imports={import_indices:?}"));
    let call_indices = wasm_direct_call_indices_for_export(&wasm, "molt_main");
    assert!(
        call_indices.contains(&native_import_index),
        "native callable invoke_ffi must become a direct WASM call to {native_symbol}; calls={call_indices:?}"
    );
    if let Some(invoke_ffi_ic_index) = import_indices.get("invoke_ffi_ic") {
        assert!(
            !call_indices.contains(invoke_ffi_ic_index),
            "native callable invoke_ffi must not fall back to invoke_ffi_ic; calls={call_indices:?}"
        );
    }
}

#[test]
fn native_callable_direct_symbol_object_callargs_imports_and_directly_calls_symbol() {
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(wasm_native_callable_ir("molt.object_callargs_v1"));

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("native callable callargs dispatch must emit valid WASM");

    let native_symbol = "molt_nativepkg_ndimage_distance_transform_edt";
    let import_modules = wasm_function_import_modules(&wasm);
    assert_eq!(
        import_modules.get(native_symbol).map(String::as_str),
        Some("molt_native"),
        "callargs native callable symbols must not be imported through the Molt runtime namespace"
    );

    let import_type_indices = wasm_function_import_type_indices(&wasm);
    assert_eq!(
        import_type_indices.get(native_symbol).copied(),
        Some(2),
        "object-callargs native callable ABI must use boxed (i64) -> i64 type index"
    );

    let import_indices = wasm_function_import_indices(&wasm);
    let native_import_index = *import_indices
        .get(native_symbol)
        .unwrap_or_else(|| panic!("{native_symbol} import missing; imports={import_indices:?}"));
    let call_indices = wasm_direct_call_indices_for_export(&wasm, "molt_main");
    assert!(
        call_indices.contains(&native_import_index),
        "native callable callargs invoke_ffi must become a direct WASM call to {native_symbol}; calls={call_indices:?}"
    );
    if let Some(invoke_ffi_ic_index) = import_indices.get("invoke_ffi_ic") {
        assert!(
            !call_indices.contains(invoke_ffi_ic_index),
            "native callable callargs invoke_ffi must not fall back to invoke_ffi_ic; calls={call_indices:?}"
        );
    }
}

#[test]
fn native_callable_module_attr_object_call_uses_runtime_ffi_without_native_import() {
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(wasm_module_attr_native_callable_ir(
        "molt.object_call_v1",
        vec!["func", "arg"],
    ));

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("native callable module_attr dispatch must emit valid WASM");

    let native_symbol = "molt_nativepkg_ndimage_distance_transform_edt";
    let import_modules = wasm_function_import_modules(&wasm);
    assert!(
        !import_modules.contains_key(native_symbol),
        "module_attr native callable dispatch must not invent a direct native import"
    );

    let import_indices = wasm_function_import_indices(&wasm);
    let invoke_ffi_ic_index = *import_indices
        .get("invoke_ffi_ic")
        .unwrap_or_else(|| panic!("invoke_ffi_ic import missing; imports={import_indices:?}"));
    let call_indices = wasm_direct_call_indices_for_export(&wasm, "molt_main");
    assert!(
        call_indices.contains(&invoke_ffi_ic_index),
        "module_attr invoke_ffi must dispatch through runtime FFI; calls={call_indices:?}"
    );
}

#[test]
fn native_callable_module_attr_object_callargs_uses_runtime_ffi_without_native_import() {
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(wasm_module_attr_native_callable_ir(
        "molt.object_callargs_v1",
        vec!["func", "callargs"],
    ));

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("native callable module_attr callargs dispatch must emit valid WASM");

    let native_symbol = "molt_nativepkg_ndimage_distance_transform_edt";
    let import_modules = wasm_function_import_modules(&wasm);
    assert!(
        !import_modules.contains_key(native_symbol),
        "module_attr callargs dispatch must not invent a direct native import"
    );

    let import_indices = wasm_function_import_indices(&wasm);
    let invoke_ffi_ic_index = *import_indices
        .get("invoke_ffi_ic")
        .unwrap_or_else(|| panic!("invoke_ffi_ic import missing; imports={import_indices:?}"));
    let call_indices = wasm_direct_call_indices_for_export(&wasm, "molt_main");
    assert!(
        call_indices.contains(&invoke_ffi_ic_index),
        "module_attr callargs invoke_ffi must dispatch through runtime FFI; calls={call_indices:?}"
    );
}

#[test]
fn native_callable_forward_f32_imports_and_directly_calls_typed_payload_symbol() {
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(wasm_native_callable_ir("molt.forward_f32_v1"));

    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("native callable forward_f32 dispatch must emit valid WASM");

    let native_symbol = "molt_nativepkg_ndimage_distance_transform_edt";
    let import_modules = wasm_function_import_modules(&wasm);
    assert_eq!(
        import_modules.get(native_symbol).map(String::as_str),
        Some("molt_native"),
        "forward_f32 native callable symbols must be imported through the native callable namespace"
    );

    let import_type_indices = wasm_function_import_type_indices(&wasm);
    assert_eq!(
        import_type_indices.get(native_symbol).copied(),
        Some(19),
        "forward_f32 must use typed native WASM ABI (i32 input_ptr, i64 byte_len, i32 output_ptr) -> i32"
    );

    let import_indices = wasm_function_import_indices(&wasm);
    let native_import_index = *import_indices
        .get(native_symbol)
        .unwrap_or_else(|| panic!("{native_symbol} import missing; imports={import_indices:?}"));
    let call_indices = wasm_direct_call_indices_for_export(&wasm, "molt_main");
    assert!(
        call_indices.contains(&native_import_index),
        "forward_f32 invoke_ffi must become a direct WASM call to {native_symbol}; calls={call_indices:?}"
    );
    for runtime_import in [
        "bytes_as_ptr",
        "scratch_alloc",
        "scratch_free",
        "bytes_from_bytes",
    ] {
        let runtime_import_index = *import_indices.get(runtime_import).unwrap_or_else(|| {
            panic!("{runtime_import} import missing; imports={import_indices:?}")
        });
        assert!(
            call_indices.contains(&runtime_import_index),
            "forward_f32 typed lowering must directly call {runtime_import}; calls={call_indices:?}"
        );
    }
    if let Some(invoke_ffi_ic_index) = import_indices.get("invoke_ffi_ic") {
        assert!(
            !call_indices.contains(invoke_ffi_ic_index),
            "forward_f32 invoke_ffi must not fall back to invoke_ffi_ic; calls={call_indices:?}"
        );
    }
}

#[test]
fn native_callable_pyinit_imports_wasm32_pointer_and_extends_to_value_lane() {
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(wasm_native_callable_ir_with_args(
        "molt.pyinit_module_v1",
        vec![],
    ));

    wasmparser::Validator::new().validate_all(&wasm).expect(
        "PyInit native callable dispatch must extend wasm32 PyObject* into the i64 value lane",
    );

    let native_symbol = "molt_nativepkg_ndimage_distance_transform_edt";
    let import_modules = wasm_function_import_modules(&wasm);
    assert_eq!(
        import_modules.get(native_symbol).map(String::as_str),
        Some("molt_native"),
        "PyInit native callable symbols must be imported through the native callable namespace"
    );

    let import_type_indices = wasm_function_import_type_indices(&wasm);
    let native_type_index = *import_type_indices.get(native_symbol).unwrap_or_else(|| {
        panic!("{native_symbol} type index missing; imports={import_type_indices:?}")
    });
    let type_signatures = wasm_type_section_value_signatures(&wasm);
    assert_eq!(
        type_signatures
            .get(native_type_index as usize)
            .unwrap_or_else(|| panic!("missing type signature for index {native_type_index}")),
        &(Vec::<String>::new(), vec!["I32".to_string()])
    );

    let import_indices = wasm_function_import_indices(&wasm);
    let native_import_index = *import_indices
        .get(native_symbol)
        .unwrap_or_else(|| panic!("{native_symbol} import missing; imports={import_indices:?}"));
    let call_indices = wasm_direct_call_indices_for_export(&wasm, "molt_main");
    assert!(
        call_indices.contains(&native_import_index),
        "PyInit invoke_ffi must become a direct WASM call to {native_symbol}; calls={call_indices:?}"
    );
}

#[test]
#[should_panic(expected = "with ABI `molt.forward_f32_v1` has 2 operand(s), expected 1")]
fn native_callable_forward_f32_rejects_non_unary_payload_abi() {
    let _ = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(wasm_native_callable_ir_with_args(
        "molt.forward_f32_v1",
        vec!["arg0", "arg1"],
    ));
}
