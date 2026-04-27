use cranelift_object::object::{Object, ObjectSymbol};
use molt_backend::{FunctionIR, OpIR, SimpleBackend, SimpleIR};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
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

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
    let file = cranelift_object::object::File::parse(&*output.bytes).expect("parse object");
    assert!(
        !file
            .symbols()
            .any(|symbol| symbol.name().ok() == Some("molt_init_sys") && !symbol.is_undefined()),
        "molt_init_sys must not be defined/exported by the object"
    );
}
