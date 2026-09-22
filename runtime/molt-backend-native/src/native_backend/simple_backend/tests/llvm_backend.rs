use super::*;

#[test]
fn llvm_backend_keeps_shared_stdlib_partition_external() {
    use object::{BinaryFormat, Object, ObjectSymbol};

    let caller = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "molt_main".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "call".to_string(),
                s_value: Some("molt_init_sys".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };
    let provider = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "molt_init_sys".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };
    let module_context =
        SimpleBackend::prepare_module_context(&mut vec![caller.clone(), provider.clone()]);
    let mut declaration = provider;
    declaration
        .externalize_with_signature()
        .expect("externalize shared stdlib provider");
    let ir = SimpleIR {
        functions: vec![caller, declaration],
        profile: None,
    };

    let mut backend = SimpleBackend::new();
    backend.set_module_context(module_context);
    let bytes = backend.compile_llvm(ir).bytes;
    let object = object::File::parse(bytes.as_slice()).expect("parse LLVM output object");
    let expected = if object.format() == BinaryFormat::MachO {
        "_molt_init_sys"
    } else {
        "molt_init_sys"
    };
    let symbols: Vec<_> = object
        .symbols()
        .filter(|symbol| symbol.name().expect("read LLVM object symbol name") == expected)
        .map(|symbol| {
            (
                symbol.is_global(),
                symbol.is_weak(),
                symbol.is_definition(),
                symbol.is_undefined(),
            )
        })
        .collect();
    assert!(
        symbols
            .iter()
            .any(|&(global, weak, _, undefined)| global && !weak && undefined),
        "shared stdlib symbol {expected} must be a strong undefined external: {symbols:?}"
    );
    assert!(
        !symbols.iter().any(|&(_, _, defined, _)| defined),
        "LLVM output must not define shared stdlib symbol {expected}: {symbols:?}"
    );
}
