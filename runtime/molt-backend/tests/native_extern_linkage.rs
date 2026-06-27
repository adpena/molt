use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};
use object::{Object, ObjectSymbol};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn object_symbol_matches<'data, S: ObjectSymbol<'data>>(symbol: &S, logical_name: &str) -> bool {
    symbol
        .name()
        .ok()
        .is_some_and(|name| name == logical_name || name.strip_prefix('_') == Some(logical_name))
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
            },
            FunctionIR {
                name: "molt_init_sys".to_string(),
                params: Vec::new(),
                ops: Vec::new(),
                param_types: None,
                source_file: None,
                is_extern: true,
            },
        ],
        profile: None,
    };

    // Standalone codegen object for symbol inspection — never linked into a
    // final binary, so it must not emit the per-app `molt_app_resolve_intrinsic`
    // resolver (which would require the linked runtime staticlib's
    // intrinsic-symbol set). This is the same opt-out production uses for every
    // non-primary object; integration tests cannot rely on the `cfg(test)`
    // carve-out in `runtime_intrinsic_symbols_required` because they link
    // `molt-backend` as a non-test library.
    let mut backend = SimpleBackend::new();
    backend.emit_app_intrinsic_resolver = false;
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
    };
    let mut helper = FunctionIR {
        name: "stdlib_value_helper".to_string(),
        params: Vec::new(),
        ops: vec![helper_missing, helper_ret],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    molt_backend::externalize_function_with_signature(&mut helper);
    let module_context = SimpleBackend::build_module_context(&[caller.clone(), helper.clone()]);

    let ir = SimpleIR {
        functions: vec![caller, helper],
        profile: None,
    };

    let mut backend = SimpleBackend::new();
    backend.emit_app_intrinsic_resolver = false;
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
