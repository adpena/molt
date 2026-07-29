use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};
use object::{Object, ObjectSection, ObjectSymbol};

#[path = "support/generated_object_abi.rs"]
mod generated_object_abi;

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn provider_function(name: &str, params: &[&str], returns_value: bool) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        params: params.iter().map(|param| (*param).to_string()).collect(),
        ops: if returns_value {
            let mut missing = op("missing");
            missing.out = Some("value".to_string());
            let mut ret = op("ret");
            ret.args = Some(vec!["value".to_string()]);
            vec![missing, ret]
        } else {
            vec![op("ret_void")]
        },
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    }
}

fn externalized_function(name: &str, params: &[&str], returns_value: bool) -> FunctionIR {
    let mut function = provider_function(name, params, returns_value);
    molt_backend::externalize_function_with_signature(&mut function);
    function
}

fn mixed_void_and_value_extern_ir() -> SimpleIR {
    mixed_void_and_value_extern_fixture().0
}

fn mixed_void_and_value_extern_fixture() -> (SimpleIR, molt_backend::NativeBackendModuleContext) {
    let mut call_void = op("call");
    call_void.s_value = Some("stdlib_void_helper".to_string());
    call_void.args = Some(Vec::new());

    let mut call_value = op("call");
    call_value.s_value = Some("stdlib_value_helper".to_string());
    call_value.out = Some("result".to_string());
    call_value.args = Some(Vec::new());

    let mut ret_result = op("ret");
    ret_result.args = Some(vec!["result".to_string()]);

    let main = FunctionIR {
        name: "molt_main".to_string(),
        params: Vec::new(),
        ops: vec![call_void, call_value, ret_result],
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };
    let void_provider = provider_function("stdlib_void_helper", &[], false);
    let value_provider = provider_function("stdlib_value_helper", &[], true);
    let module_context = SimpleBackend::build_module_context(&[
        main.clone(),
        void_provider.clone(),
        value_provider.clone(),
    ]);
    (
        SimpleIR {
            functions: vec![
                main,
                externalized_function("stdlib_void_helper", &[], false),
                externalized_function("stdlib_value_helper", &[], true),
            ],
            profile: None,
        },
        module_context,
    )
}

fn assert_mixed_extern_linkage(bytes: &[u8], backend: &str) {
    let file = object::File::parse(bytes).expect("parse native object");
    let symbols: Vec<String> = file
        .symbols()
        .filter_map(|symbol| symbol.name().ok().map(str::to_string))
        .collect();
    for name in ["stdlib_void_helper", "stdlib_value_helper"] {
        assert!(
            file.symbols()
                .any(|symbol| object_symbol_matches(&symbol, name) && symbol.is_undefined()),
            "{backend} must emit `{name}` as an undefined declaration with no body; symbols: {symbols:?}"
        );
    }
    assert!(
        file.symbols().any(|symbol| {
            object_symbol_matches(&symbol, "molt_main") && !symbol.is_undefined()
        }),
        "{backend} must define molt_main; symbols: {symbols:?}"
    );
}

fn assert_extern_helper_linkage(bytes: &[u8], backend: &str) {
    let file = object::File::parse(bytes).expect("parse native object");
    assert!(
        file.symbols().any(|symbol| {
            object_symbol_matches(&symbol, "extern_helper") && symbol.is_undefined()
        }),
        "{backend} must retain extern_helper as an undefined declaration"
    );
    assert!(
        file.symbols().any(|symbol| {
            object_symbol_matches(&symbol, "molt_main") && !symbol.is_undefined()
        }),
        "{backend} must define molt_main"
    );
}

fn extern_call_mismatch_ir(
    declaration_params: &[&str],
    declaration_returns_value: bool,
    caller_args: &[&str],
    caller_expects_result: bool,
) -> SimpleIR {
    extern_call_mismatch_fixture(
        declaration_params,
        declaration_returns_value,
        caller_args,
        caller_expects_result,
    )
    .0
}

fn extern_call_mismatch_fixture(
    declaration_params: &[&str],
    declaration_returns_value: bool,
    caller_args: &[&str],
    caller_expects_result: bool,
) -> (SimpleIR, molt_backend::NativeBackendModuleContext) {
    let mut call = op("call");
    call.s_value = Some("extern_helper".to_string());
    call.args = Some(caller_args.iter().map(|arg| (*arg).to_string()).collect());
    call.out = caller_expects_result.then(|| "result".to_string());
    let caller = FunctionIR {
        name: "molt_main".to_string(),
        params: caller_args.iter().map(|arg| (*arg).to_string()).collect(),
        ops: vec![call, op("ret_void")],
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };
    let provider = provider_function(
        "extern_helper",
        declaration_params,
        declaration_returns_value,
    );
    let module_context = SimpleBackend::build_module_context(&[caller.clone(), provider]);
    (
        SimpleIR {
            functions: vec![
                caller,
                externalized_function(
                    "extern_helper",
                    declaration_params,
                    declaration_returns_value,
                ),
            ],
            profile: None,
        },
        module_context,
    )
}

#[test]
fn native_object_retains_exact_generated_object_abi_import() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: Vec::new(),
            ops: vec![op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    let output = backend.compile(ir);
    generated_object_abi::assert_exact_import_and_dead_strip_link(&output.bytes, "Cranelift");
    let file = object::File::parse(&*output.bytes).expect("parse native object");
    let anchor = file
        .symbols()
        .find(|symbol| object_symbol_matches(symbol, "__molt_generated_object_abi_anchor"))
        .expect("generated-object ABI anchor");
    let anchor_section = file
        .section_by_index(anchor.section_index().expect("anchor section"))
        .expect("read anchor section");
    let pointer_bytes = match file.architecture() {
        object::Architecture::X86_64 | object::Architecture::Aarch64 => 8,
        _ => 4,
    };
    assert_eq!(
        anchor_section.size(),
        pointer_bytes,
        "link admission costs exactly one pointer-sized data word"
    );
    let witness_relocations = anchor_section
        .relocations()
        .filter(|(_, relocation)| {
            let object::RelocationTarget::Symbol(index) = relocation.target() else {
                return false;
            };
            file.symbol_by_index(index).ok().is_some_and(|symbol| {
                object_symbol_matches(&symbol, molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL)
            })
        })
        .count();
    assert_eq!(
        witness_relocations, 1,
        "link admission costs exactly one retained relocation"
    );

    #[cfg(windows)]
    {
        let directives = file
            .section_by_name(".drectve")
            .and_then(|section| section.data().ok())
            .expect("COFF object must carry linker retention directives");
        let directives = String::from_utf8_lossy(directives);
        assert!(
            directives.contains(molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL),
            "COFF /OPT:REF root must name exact ABI symbol: {directives}"
        );
    }
}

fn object_symbol_matches<'data, S: ObjectSymbol<'data>>(symbol: &S, logical_name: &str) -> bool {
    symbol
        .name()
        .ok()
        .is_some_and(|name| name == logical_name || name.strip_prefix('_') == Some(logical_name))
}

#[test]
fn cross_format_objects_retain_generated_object_abi_anchor() {
    for (target, expected_format) in [
        ("x86_64-unknown-linux-gnu", object::BinaryFormat::Elf),
        ("aarch64-apple-darwin", object::BinaryFormat::MachO),
    ] {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: Vec::new(),
                ops: vec![op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            }],
            profile: None,
        };
        let mut backend = SimpleBackend::new_with_target(Some(target));
        backend.emit_app_callable_resolver = false;
        let output = backend.compile(ir);
        let file = object::File::parse(&*output.bytes).expect("parse cross-format object");
        assert_eq!(file.format(), expected_format);
        let anchor = file
            .symbols()
            .find(|symbol| object_symbol_matches(symbol, "__molt_generated_object_abi_anchor"))
            .expect("retained generated-object ABI anchor symbol");
        let section = file
            .section_by_index(anchor.section_index().expect("anchor section"))
            .expect("read anchor section");
        match (expected_format, section.flags()) {
            (object::BinaryFormat::Elf, object::SectionFlags::Elf { sh_flags }) => assert_ne!(
                sh_flags & u64::from(object::elf::SHF_GNU_RETAIN),
                0,
                "ELF anchor must survive --gc-sections"
            ),
            (object::BinaryFormat::MachO, object::SectionFlags::MachO { .. }) => assert_ne!(
                match anchor.flags() {
                    object::SymbolFlags::MachO { n_desc } => {
                        n_desc & object::macho::N_NO_DEAD_STRIP
                    }
                    flags => panic!("unexpected Mach-O anchor symbol flags: {flags:?}"),
                },
                0,
                "Mach-O anchor must survive -dead_strip"
            ),
            (_, flags) => panic!("unexpected {target} anchor flags: {flags:?}"),
        }
    }
}

#[test]
fn extern_calls_compile_without_exporting_undefined_stdlib_symbols() {
    let mut init_sys = op("call");
    init_sys.s_value = Some("molt_init_sys".to_string());

    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                params: Vec::new(),
                ops: vec![init_sys, op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                name: "molt_init_sys".to_string(),
                params: Vec::new(),
                ops: vec![op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: true,
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    // Standalone codegen object for symbol inspection — never linked into a
    // final binary, so it must not emit the per-app `molt_app_resolve_callable`
    // resolver (which would require the linked runtime staticlib's
    // callable-symbol set). This is the same opt-out production uses for every
    // non-primary object; integration tests cannot rely on the `cfg(test)`
    // carve-out in `runtime_callable_symbols_required` because they link
    // `molt-backend` as a non-test library.
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    let output = backend.compile(ir);

    assert!(!output.bytes.is_empty());
    let file = object::File::parse(&*output.bytes).expect("parse object");
    assert!(
        !file
            .symbols()
            .any(|symbol| object_symbol_matches(&symbol, "molt_init_sys") && !symbol.is_undefined()),
        "molt_init_sys must not be defined/exported by the object"
    );
}

#[test]
fn cranelift_declares_mixed_void_and_value_externs_without_bodies() {
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    let output = backend.compile(mixed_void_and_value_extern_ir());
    assert!(!output.bytes.is_empty());
    assert_mixed_extern_linkage(&output.bytes, "Cranelift");
}

#[test]
#[should_panic(
    expected = "native extern call ABI mismatch: caller `molt_main` supplies 0 parameter(s) to declaration `extern_helper`, which requires 1"
)]
fn cranelift_rejects_exact_extern_arity_mismatch() {
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    let _ = backend.compile(extern_call_mismatch_ir(&["arg"], true, &[], true));
}

#[test]
fn cranelift_void_extern_call_may_bind_boxed_none() {
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    let output = backend.compile(extern_call_mismatch_ir(&[], false, &[], true));
    assert!(!output.bytes.is_empty());
    assert_extern_helper_linkage(&output.bytes, "Cranelift void-to-None");
}

#[test]
fn native_module_context_rejects_corrupt_logical_and_machine_linkage_contracts() {
    let (ir, module_context) = mixed_void_and_value_extern_fixture();
    let encoded = serde_json::to_value(module_context).expect("serialize native module context");

    let mut return_mismatch = encoded.clone();
    return_mismatch["function_linkage_abis"]["stdlib_value_helper"]["source_signature"]["returns_value"] =
        serde_json::json!(false);
    let context: molt_backend::NativeBackendModuleContext =
        serde_json::from_value(return_mismatch).expect("decode return-mismatched context");
    let error = context
        .validate_function_linkage_abis(&ir.functions)
        .expect_err("return mismatch must fail closed");
    assert!(error.contains("return carrier disagrees"), "{error}");

    let mut closure_mismatch = encoded.clone();
    closure_mismatch["function_linkage_abis"]["stdlib_void_helper"]["source_signature"]["has_closure"] =
        serde_json::json!(true);
    let context: molt_backend::NativeBackendModuleContext =
        serde_json::from_value(closure_mismatch).expect("decode closure-mismatched context");
    let error = context
        .validate_function_linkage_abis(&ir.functions)
        .expect_err("closure mismatch must fail closed");
    assert!(error.contains("signature"), "{error}");

    let mut execution_context_mismatch = encoded;
    execution_context_mismatch["function_linkage_abis"]["stdlib_void_helper"]["source_signature"]
        ["execution_context"] = serde_json::json!("local");
    let context: molt_backend::NativeBackendModuleContext =
        serde_json::from_value(execution_context_mismatch)
            .expect("decode execution-context-mismatched context");
    let error = context
        .validate_function_linkage_abis(&ir.functions)
        .expect_err("execution-context mismatch must fail closed");
    assert!(error.contains("signature"), "{error}");
}

#[cfg(feature = "llvm")]
#[test]
fn llvm_declares_mixed_void_and_value_externs_without_bodies() {
    let (ir, module_context) = mixed_void_and_value_extern_fixture();
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    backend.set_module_context(module_context);
    let output = backend.compile_llvm(ir);
    assert!(!output.bytes.is_empty());
    assert_mixed_extern_linkage(&output.bytes, "LLVM");
}

#[cfg(feature = "llvm")]
#[test]
#[should_panic(
    expected = "native extern call ABI mismatch: caller `molt_main` supplies 0 parameter(s) to declaration `extern_helper`, which requires 1"
)]
fn llvm_rejects_exact_extern_arity_mismatch() {
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    let _ = backend.compile_llvm(extern_call_mismatch_ir(&["arg"], true, &[], true));
}

#[cfg(feature = "llvm")]
#[test]
fn llvm_void_extern_call_may_bind_boxed_none() {
    let (ir, module_context) = extern_call_mismatch_fixture(&[], false, &[], true);
    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    backend.set_module_context(module_context);
    let output = backend.compile_llvm(ir);
    assert!(!output.bytes.is_empty());
    assert_extern_helper_linkage(&output.bytes, "LLVM void-to-None");
}
