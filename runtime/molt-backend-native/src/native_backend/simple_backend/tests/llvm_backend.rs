use super::*;

#[cfg(feature = "llvm")]
#[test]
fn llvm_backend_keeps_shared_stdlib_partition_external() {
    let _guard = acquire_backend_env_lock();
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-llvm-stdlib-extern-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let stdlib_obj = tmp_dir.join("stdlib.o");
    std::fs::write(&stdlib_obj, b"placeholder").expect("write stdlib marker");

    let prev_backend = std::env::var("MOLT_BACKEND").ok();
    let prev_stdlib_obj = std::env::var("MOLT_STDLIB_OBJ").ok();
    let prev_entry_module = std::env::var("MOLT_ENTRY_MODULE").ok();
    let prev_stdlib_symbols = std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok();
    unsafe {
        std::env::set_var("MOLT_BACKEND", "llvm");
        std::env::set_var("MOLT_STDLIB_OBJ", &stdlib_obj);
        std::env::set_var("MOLT_ENTRY_MODULE", "app");
        std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]");
    }

    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
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
            },
            FunctionIR {
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    };

    let bytes = SimpleBackend::new().compile(ir).bytes;
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

    match prev_backend {
        Some(value) => unsafe { std::env::set_var("MOLT_BACKEND", value) },
        None => unsafe { std::env::remove_var("MOLT_BACKEND") },
    }
    match prev_stdlib_obj {
        Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_OBJ", value) },
        None => unsafe { std::env::remove_var("MOLT_STDLIB_OBJ") },
    }
    match prev_entry_module {
        Some(value) => unsafe { std::env::set_var("MOLT_ENTRY_MODULE", value) },
        None => unsafe { std::env::remove_var("MOLT_ENTRY_MODULE") },
    }
    match prev_stdlib_symbols {
        Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", value) },
        None => unsafe { std::env::remove_var("MOLT_STDLIB_MODULE_SYMBOLS") },
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[cfg(not(feature = "llvm"))]
#[test]
#[should_panic(
    expected = "MOLT_BACKEND=llvm requested but molt-backend was built without the llvm feature"
)]
fn llvm_backend_request_without_feature_fails_closed() {
    assert_requested_llvm_backend_available(true);
}

#[cfg(not(feature = "llvm"))]
#[test]
fn llvm_missing_feature_guard_allows_non_llvm_backend_selection() {
    assert_requested_llvm_backend_available(false);
}
