#![cfg(feature = "rust-backend")]

use molt_backend_rust::rust::RustBackend;
use molt_backend_rust::{FunctionIR, OpIR, SimpleIR};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

#[test]
fn emitted_sys_default_and_full_release_metadata_execute() {
    mod expected {
        include!("../src/rust/python_version_policy_generated.rs");
    }
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![op("ret_void")],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let mut source = RustBackend::new().compile_checked(&ir).unwrap();
    source.push_str(&format!(
        r#"
#[test]
fn target_version_metadata() {{
    let state = MoltSysVersionInfo::default();
    assert_eq!((state.major, state.minor, state.micro, state.releaselevel.as_str(), state.serial), {:?});
    let mut args = vec![MoltValue::Int(3), MoltValue::Int(14), MoltValue::Int(7), MoltValue::Str("candidate".into()), MoltValue::Int(2)];
    molt_sys_set_version_info(&mut args);
    let published = molt_sys_version_state().lock().unwrap();
    assert_eq!(published.formatted_version(), "3.14.7rc2 (molt)");
    assert_eq!(published.hexversion(), 0x030e07c2);
}}
"#,
        expected::DEFAULT_TARGET_PYTHON_VERSION,
    ));
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("sys-version-metadata")
        .expect("create emitted-source proof artifacts");
    let input = artifacts.path().join("version.rs");
    let binary = artifacts.path().join(if cfg!(windows) {
        "version.exe"
    } else {
        "version"
    });
    std::fs::write(&input, source).unwrap();
    let compiled = artifacts
        .command(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .unwrap()
        .args(["--test", "--edition=2021", "-Awarnings"])
        .arg(artifacts.argument("", &input).unwrap())
        .arg("-o")
        .arg(artifacts.argument("", &binary).unwrap())
        .output()
        .expect("compile actual emitted source");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let executed = std::process::Command::new(&binary)
        .args(["--exact", "target_version_metadata"])
        .output()
        .expect("execute standalone emitted Rust without Python");
    assert!(
        executed.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&executed.stdout),
        String::from_utf8_lossy(&executed.stderr)
    );
    assert!(String::from_utf8_lossy(&executed.stdout).contains("1 passed"));
}

#[test]
fn rust_backend_rejects_class_slots_without_object_representation() {
    let mut alloc_obj = op("alloc_class");
    alloc_obj.args = Some(vec!["point_cls".to_string()]);
    alloc_obj.value = Some(24);
    alloc_obj.out = Some("point_obj".to_string());

    let mut none0 = op("const_none");
    none0.out = Some("slot0_init".to_string());

    let mut store0_init = op("store");
    store0_init.args = Some(vec!["point_obj".to_string(), "slot0_init".to_string()]);
    store0_init.value = Some(0);

    let mut none1 = op("const_none");
    none1.out = Some("slot1_init".to_string());

    let mut store1_init = op("store");
    store1_init.args = Some(vec!["point_obj".to_string(), "slot1_init".to_string()]);
    store1_init.value = Some(8);

    let mut init_name = op("const_str");
    init_name.s_value = Some("__init__".to_string());
    init_name.out = Some("init_name".to_string());

    let mut get_init = op("get_attr_name");
    get_init.args = Some(vec!["point_cls".to_string(), "init_name".to_string()]);
    get_init.out = Some("init_func".to_string());

    let mut bind_init = op("bound_method_new");
    bind_init.args = Some(vec!["init_func".to_string(), "point_obj".to_string()]);
    bind_init.out = Some("bound_init".to_string());

    let mut call_args = op("callargs_new");
    call_args.out = Some("call_args".to_string());

    let mut call_init = op("call_bind");
    call_init.args = Some(vec!["bound_init".to_string(), "call_args".to_string()]);
    call_init.out = Some("init_result".to_string());

    let mut store0 = op("store");
    store0.args = Some(vec!["point_obj".to_string(), "x".to_string()]);
    store0.value = Some(0);

    let mut store1 = op("store");
    store1.args = Some(vec!["point_obj".to_string(), "y".to_string()]);
    store1.value = Some(8);

    let mut load0 = op("load");
    load0.args = Some(vec!["point_obj".to_string()]);
    load0.value = Some(0);
    load0.out = Some("loaded_x".to_string());

    let mut load1 = op("load");
    load1.args = Some(vec!["point_obj".to_string()]);
    load1.value = Some(8);
    load1.out = Some("loaded_y".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["loaded_y".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_class_slots".to_string(),
            params: vec!["point_cls".to_string(), "x".to_string(), "y".to_string()],
            ops: vec![
                alloc_obj,
                none0,
                store0_init,
                none1,
                store1_init,
                init_name,
                get_init,
                bind_init,
                call_args,
                call_init,
                store0,
                store1,
                load0,
                load1,
                ret,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let err = backend
        .compile_checked(&ir)
        .expect_err("class slot IR requires a real Rust object/type representation");

    assert!(err.contains("rejected before source generation"), "{err}");
    assert!(
        err.contains("molt_test_class_slots:op#0 `alloc_class`"),
        "{err}"
    );
    assert!(
        err.contains("Python aliasing, cycles, None storage, hashing, and object protocols"),
        "{err}"
    );
}

#[test]
fn rust_backend_rejects_module_attrs_without_python_object_protocols() {
    let mut module_new = op("module_new");
    module_new.out = Some("module_obj".to_string());

    let mut attr_name = op("const_str");
    attr_name.s_value = Some("Point".to_string());
    attr_name.out = Some("attr_name".to_string());

    let mut attr_value = op("const_str");
    attr_value.s_value = Some("Point".to_string());
    attr_value.out = Some("point_value".to_string());

    let mut module_set = op("module_set_attr");
    module_set.args = Some(vec![
        "module_obj".to_string(),
        "attr_name".to_string(),
        "point_value".to_string(),
    ]);

    let mut module_get = op("module_get_attr");
    module_get.args = Some(vec!["module_obj".to_string(), "attr_name".to_string()]);
    module_get.out = Some("loaded_cls".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["loaded_cls".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_module_attrs".to_string(),
            params: Vec::new(),
            ops: vec![
                module_new, attr_name, attr_value, module_set, module_get, ret,
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let error = backend
        .compile_checked(&ir)
        .expect_err("module source helpers do not implement Python object protocols");
    assert!(
        error.contains("rejected before source generation"),
        "{error}"
    );
    assert!(
        error.contains("molt_test_module_attrs:op#0 `module_new`"),
        "{error}"
    );
    assert!(
        error.contains("Python aliasing, cycles, None storage, hashing, and object protocols"),
        "{error}"
    );
}

#[test]
fn rust_backend_rejects_runtime_value_surface_without_catchable_exceptions() {
    for kind in ["str_from_obj", "repr_from_obj", "ascii_from_obj"] {
        let mut call = op(kind);
        call.args = Some(vec!["word".to_string()]);
        call.out = Some("converted".to_string());
        let mut ret = op("ret");
        ret.args = Some(vec!["converted".to_string()]);
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "runtime_value_surface".to_string(),
                params: vec!["word".to_string()],
                ops: vec![call, ret],
                ..FunctionIR::default()
            }],
            profile: None,
        };
        let error = RustBackend::new()
            .compile_checked(&ir)
            .expect_err("well-shaped runtime calls still require catchable Python exceptions");
        assert!(
            error.contains("rejected before source generation"),
            "{error}"
        );
        assert!(
            error.contains(&format!("runtime_value_surface:op#0 `{kind}`")),
            "{error}"
        );
        assert!(
            error.contains("structured catchable Python exceptions"),
            "{error}"
        );
    }
}

#[test]
fn rust_backend_rejects_valid_branch_without_python_truthiness() {
    let mut cond = op("const_bool");
    cond.value = Some(1);
    cond.out = Some("cond".to_string());

    let mut branch = op("br_if");
    branch.args = Some(vec!["cond".to_string()]);
    branch.value = Some(1);

    let mut label = op("label");
    label.value = Some(1);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_test_branch_gap".to_string(),
            params: Vec::new(),
            ops: vec![cond, branch, label, op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let error = backend
        .compile_checked(&ir)
        .expect_err("a valid graph cannot claim unavailable Python truthiness");
    assert!(
        error.contains("rejected before source generation"),
        "{error}"
    );
    assert!(
        error.contains("molt_test_branch_gap:op#1 `br_if`"),
        "{error}"
    );
    assert!(error.contains("Python truthiness"), "{error}");
}

#[test]
fn rust_backend_rejects_sys_version_call_without_catchable_exceptions() {
    let mut call = op("call");
    call.s_value = Some("molt_sys_version".to_string());
    call.out = Some("version".to_string());
    let mut ret = op("ret");
    ret.args = Some(vec!["version".to_string()]);
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "sys_version_probe".to_string(),
            ops: vec![call, ret],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let error = RustBackend::new()
        .compile_checked(&ir)
        .expect_err("a prelude helper does not supply catchable Python call semantics");
    assert!(
        error.contains("rejected before source generation"),
        "{error}"
    );
    assert!(error.contains("sys_version_probe:op#0 `call`"), "{error}");
    assert!(
        error.contains("structured catchable Python exceptions"),
        "{error}"
    );
}

#[test]
fn rust_backend_keeps_admitted_float_programs_available() {
    let mut value = op("const_float");
    value.f_value = Some(1.5);
    value.out = Some("answer".to_string());
    let mut ret = op("ret");
    ret.args = Some(vec!["answer".to_string()]);
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "float_probe".to_string(),
            ops: vec![value, ret],
            ..FunctionIR::default()
        }],
        profile: None,
    };
    let source = RustBackend::new()
        .compile_checked(&ir)
        .expect("the exact fixed-width float subset remains admitted");
    assert!(source.contains("MoltValue::Float(f64::from_bits(4609434218613702656u64))"));
    assert!(source.contains("return answer.clone();"));
    assert!(!source.contains("compile_error!") && !source.contains("MOLT_STUB"));
}

#[test]
fn rust_backend_rejects_sys_import_without_python_import_protocol() {
    let mut import_sys = op("module_import");
    import_sys.s_value = Some("sys".to_string());
    import_sys.out = Some("sys_module".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["sys_module".to_string()]);

    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_target_python_sys_import_probe".to_string(),
            params: Vec::new(),
            ops: vec![import_sys, ret],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let mut backend = RustBackend::new();
    let error = backend
        .compile_checked(&ir)
        .expect_err("sys import requires the complete Python import protocol");

    assert!(
        error.contains("rejected before source generation"),
        "{error}"
    );
    assert!(error.contains("module_import"), "{error}");
}
