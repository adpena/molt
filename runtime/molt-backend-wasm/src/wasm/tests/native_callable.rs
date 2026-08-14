use super::support::*;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, LinkingSection,
    Module, SymbolTable, TypeSection, ValType,
};

struct RemoveNativeCallableTemp(PathBuf);

impl Drop for RemoveNativeCallableTemp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn native_callable_wasm_temp_dir() -> (PathBuf, RemoveNativeCallableTemp) {
    let path = std::env::temp_dir().join(format!(
        "molt-wasm-native-callable-link-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&path).expect("create WASM native callable link temp dir");
    (path.clone(), RemoveNativeCallableTemp(path))
}

fn real_execution_tool(tool: PathBuf, required_env: &str, purpose: &str) -> Option<PathBuf> {
    let available = Command::new(&tool)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if available {
        return Some(tool);
    }
    if std::env::var_os("CI").is_some()
        || std::env::var_os(required_env).is_some()
        || std::env::var_os("MOLT_REQUIRE_REAL_NATIVE_CALLABLE_EXECUTION_TESTS").is_some()
    {
        panic!(
            "real {purpose} is required but `{}` is unavailable",
            tool.display()
        );
    }
    eprintln!(
        "SKIP real {purpose}: `{}` is unavailable; set {required_env}=1 to make this a hard failure",
        tool.display()
    );
    None
}

fn wasm_ld_path() -> PathBuf {
    std::env::var_os("MOLT_WASM_LD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("wasm-ld"))
}

fn run_execution_command(command: &mut Command, purpose: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{purpose}: failed to start: {error}"));
    assert!(
        output.status.success(),
        "{purpose}: status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wasm_native_callable_provider_object(symbol: &str, sentinel: i64) -> Vec<u8> {
    let mut types = TypeSection::new();
    types.ty().function([ValType::I64], [ValType::I64]);
    let mut functions = FunctionSection::new();
    functions.function(0);
    let mut exports = ExportSection::new();
    exports.export(symbol, ExportKind::Func, 0);
    let mut body = Function::new([]);
    body.instruction(&Instruction::I64Const(sentinel));
    body.instruction(&Instruction::End);
    let mut code = CodeSection::new();
    code.function(&body);
    let mut symbols = SymbolTable::new();
    symbols.function(
        SymbolTable::WASM_SYM_EXPORTED | SymbolTable::WASM_SYM_NO_STRIP,
        0,
        Some(symbol),
    );
    let mut linking = LinkingSection::new();
    linking.symbol_table(&symbols);
    let mut module = Module::new();
    module.section(&types);
    module.section(&functions);
    module.section(&exports);
    module.section(&code);
    module.section(&linking);
    module.finish()
}

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
    ret.args = Some(vec!["literal".to_string()]);
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
    ret.args = Some(vec!["value".to_string()]);
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
fn relocatable_native_callable_links_provider_object_and_executes_in_node() {
    const SYMBOL: &str = "molt_nativepkg_ndimage_distance_transform_edt";
    const SENTINEL: i64 = 0x45A1_7E57_D15C_A11E;
    let Some(wasm_ld) = real_execution_tool(
        wasm_ld_path(),
        "MOLT_REQUIRE_REAL_WASM_LD_TESTS",
        "wasm-ld native callable final-link proof",
    ) else {
        return;
    };
    let Some(node) = real_execution_tool(
        PathBuf::from("node"),
        "MOLT_REQUIRE_REAL_NODE_TESTS",
        "Node native callable execution proof",
    ) else {
        return;
    };
    let mut execution_ir = wasm_native_callable_ir("molt.object_call_v1");
    execution_ir.functions[0].params.clear();
    let mut payload = wasm_test_op("const", Some("arg"), vec![]);
    payload.value = Some(1);
    execution_ir.functions[0].ops.insert(0, payload);
    let app_object = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: true,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(execution_ir);
    let provider_object = wasm_native_callable_provider_object(SYMBOL, SENTINEL);
    let (temp, _remove_temp) = native_callable_wasm_temp_dir();
    let app_path = temp.join("native_callable_app.o.wasm");
    let provider_path = temp.join("native_callable_provider.o.wasm");
    let linked_path = temp.join("native_callable_linked.wasm");
    let script_path = temp.join("verify_native_callable.cjs");
    fs::write(&app_path, app_object).expect("write relocatable native callable app");
    fs::write(&provider_path, provider_object).expect("write relocatable native callable provider");
    run_execution_command(
        Command::new(&wasm_ld)
            .arg("--no-entry")
            .arg("--import-memory")
            .arg("--import-table")
            .arg("--export=molt_main")
            .arg("-o")
            .arg(&linked_path)
            .arg(&app_path)
            .arg(&provider_path),
        "final-link relocatable app and native callable provider with wasm-ld",
    );
    let linked = fs::read(&linked_path).expect("read final-linked native callable WASM");
    wasmparser::Validator::new()
        .validate_all(&linked)
        .expect("final-linked native callable WASM must validate");
    fs::write(
        &script_path,
        format!(
            r#"const fs = require('fs');
const bytes = fs.readFileSync(process.argv[2]);
const env = {{
  memory: new WebAssembly.Memory({{initial: 256}}),
  __indirect_function_table: new WebAssembly.Table({{initial: 8192, element: 'anyfunc'}}),
}};
const wasmModule = new WebAssembly.Module(bytes);
const imports = {{env}};
for (const entry of WebAssembly.Module.imports(wasmModule)) {{
  if (entry.module === 'env') continue;
  imports[entry.module] ??= {{}};
  imports[entry.module][entry.name] = () => 0n;
}}
WebAssembly.instantiate(wasmModule, imports).then((instance) => {{
  const actual = instance.exports.molt_main();
  const expected = BigInt('{SENTINEL}');
  if (actual !== expected) throw new Error(`direct-symbol ABI returned ${{actual}}, expected ${{expected}}`);
}}).catch((error) => {{ console.error(error); process.exit(1); }});
"#
        ),
    )
    .expect("write Node native callable verifier");
    run_execution_command(
        Command::new(&node).arg(&script_path).arg(&linked_path),
        "execute final-linked native callable WASM in Node",
    );
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
