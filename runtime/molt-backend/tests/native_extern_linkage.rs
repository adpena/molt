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
                ops: Vec::new(),
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
fn externalized_value_returning_stdlib_call_emits_undefined_import_object() {
    let mut call_helper = op("call");
    call_helper.s_value = Some("stdlib_value_helper".to_string());
    call_helper.out = Some("result".to_string());
    call_helper.args = Some(Vec::new());

    let mut ret_result = op("ret");
    ret_result.var = Some("result".to_string());

    let mut helper_missing = op("missing");
    helper_missing.out = Some("value".to_string());
    let mut helper_ret = op("ret");
    helper_ret.args = Some(vec!["value".to_string()]);

    let caller = FunctionIR {
        name: "molt_main".to_string(),
        params: Vec::new(),
        ops: vec![call_helper, ret_result],
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };
    let mut helper = FunctionIR {
        name: "stdlib_value_helper".to_string(),
        params: Vec::new(),
        ops: vec![helper_missing, helper_ret],
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };
    molt_backend::externalize_function_with_signature(&mut helper);
    let module_context = SimpleBackend::build_module_context(&[caller.clone(), helper.clone()]);

    let ir = SimpleIR {
        functions: vec![caller, helper],
        profile: None,
    };

    let mut backend = SimpleBackend::new();
    backend.emit_app_callable_resolver = false;
    backend.set_module_context(module_context);
    let output = backend.compile(ir);

    assert!(!output.bytes.is_empty());
    let file = object::File::parse(&*output.bytes).expect("parse object");
    let symbols: Vec<String> = file
        .symbols()
        .filter_map(|symbol| symbol.name().ok().map(str::to_string))
        .collect();
    assert!(
        file.symbols().any(|symbol| {
            object_symbol_matches(&symbol, "stdlib_value_helper") && symbol.is_undefined()
        }),
        "stdlib_value_helper must remain an undefined import resolved by the shared stdlib object; symbols: {symbols:?}"
    );
    assert!(
        file.symbols().any(|symbol| {
            object_symbol_matches(&symbol, "molt_main") && !symbol.is_undefined()
        }),
        "molt_main must be defined by the application object; symbols: {symbols:?}"
    );
}
