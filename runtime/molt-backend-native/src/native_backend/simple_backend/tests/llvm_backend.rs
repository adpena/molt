use super::*;

#[test]
fn llvm_backend_keeps_shared_stdlib_partition_external() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-llvm-stdlib-extern-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let caller = FunctionIR {
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
        execution_context: Default::default(),
    };
    let provider = FunctionIR {
        name: "molt_init_sys".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };
    let module_context = SimpleBackend::build_module_context(&[caller.clone(), provider.clone()]);
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
    let output = tmp_dir.join("out.o");
    std::fs::write(&output, &bytes).expect("write llvm object");
    let nm = std::process::Command::new("nm")
        .args(["-g", output.to_str().expect("utf8 object path")])
        .output()
        .expect("run nm");
    assert!(
        nm.status.success(),
        "nm failed: {}",
        String::from_utf8_lossy(&nm.stderr)
    );
    let symbols = String::from_utf8_lossy(&nm.stdout);
    assert!(
        symbols
            .lines()
            .any(|line| line.contains(" U _molt_init_sys")
                || line == "                 U molt_init_sys"),
        "shared stdlib symbol must be an undefined external, got:\n{symbols}"
    );
    assert!(
        !symbols
            .lines()
            .any(|line| line.contains(" T _molt_init_sys") || line.contains(" T molt_init_sys")),
        "LLVM output object must not define shared stdlib symbol, got:\n{symbols}"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
