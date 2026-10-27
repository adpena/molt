use super::{
    ExecutionRequest, GuestModuleEntrypoint, GuestTermination, HostState, LoadedGuestKind,
    LoadedGuestOptions, ParsedHostArgs, ProcessManager, ResolvedExecution, SocketManager,
    WebSocketManager, build_engine, define_isolate_host_imports, execute_loaded_guest,
    parse_host_args, resolve_execution, resolve_execution_modules, select_manifest_path,
    validate_execution_imports, validate_guest_module_entrypoint,
};
use molt_wasm_facts::WASM_EXECUTION_MANIFEST_VERSION;
use molt_wasm_host::sha256_hex;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use wasmtime::{Engine, Func, Linker, Module, Store, Val, ValType};
use wasmtime_wasi::{I32Exit, WasiCtxBuilder, p1};

pub(super) fn test_host_state() -> HostState {
    test_host_state_with_wasi(WasiCtxBuilder::new().build_p1())
}

fn test_host_state_with_wasi(wasi: p1::WasiP1Ctx) -> HostState {
    HostState {
        wasi,
        memory: None,
        call_indirect: Arc::default(),
        isolate_bootstrap_export: None,
        isolate_import_export: None,
        db_worker: None,
        db_pending: HashMap::new(),
        db_cancel_index: Vec::new(),
        db_cancel_positions: HashMap::new(),
        db_cancel_cursor: 0,
        last_cancel_check: None,
        socket_manager: SocketManager::new(),
        ws_manager: WebSocketManager::new(),
        process_manager: ProcessManager::new(),
    }
}

fn call_test_app_startup(
    store: &mut Store<HostState>,
    application: &wasmtime::Instance,
    runtime: &wasmtime::Instance,
) -> wasmtime::Result<GuestTermination> {
    super::entrypoint::call_molt_application_entrypoint(store, application, runtime)
}

fn execute_test_guest(
    engine: &Engine,
    output: &Module,
    runtime: Option<&Module>,
    kind: LoadedGuestKind<'_>,
    guest_args: &[String],
    wasm_table_base: Option<u64>,
) -> wasmtime::Result<GuestTermination> {
    execute_loaded_guest(
        engine,
        output,
        runtime,
        LoadedGuestOptions {
            kind,
            vfs_envs: &[],
            guest_args,
            wasm_table_base,
        },
    )
}

#[test]
fn manifest_path_has_one_explicit_env_default_precedence() {
    let cwd = Path::new("/repo");
    assert_eq!(
        select_manifest_path(
            Some(PathBuf::from("/tmp/app.wasm")),
            Some(PathBuf::from("/env/manifest.json")),
            cwd,
        ),
        PathBuf::from("/tmp/app.wasm")
    );
    assert_eq!(
        select_manifest_path(None, Some(PathBuf::from("/env/manifest.json")), cwd),
        PathBuf::from("/env/manifest.json")
    );
    assert_eq!(
        select_manifest_path(None, None, cwd),
        PathBuf::from("/repo/dist/manifest.json")
    );
}

/// A manifest fixture in a directory this process alone owns; the directory
/// is removed when the returned handle drops.
fn runtime_manifest_fixture(label: &str, module_bytes: &[u8], digest: &str) -> tempfile::TempDir {
    let root = tempfile::Builder::new()
        .prefix(&format!("molt-wasm-host-manifest-{label}-"))
        .tempdir()
        .expect("create manifest fixture root");
    fs::write(root.path().join("program.wasm"), module_bytes).expect("write module fixture");
    fs::write(
        root.path().join("manifest.json"),
        format!(
            r#"{{"version":{WASM_EXECUTION_MANIFEST_VERSION},"mode":"linked","modules":{{"linked":{{"path":"program.wasm","size":{},"sha256":"{digest}"}}}}}}"#,
            module_bytes.len()
        ),
    )
    .expect("write manifest fixture");
    root
}

#[test]
fn runtime_manifest_resolves_and_verifies_linked_module() {
    let bytes = b"linked wasm fixture";
    let digest = sha256_hex(bytes);
    let root = runtime_manifest_fixture("valid", bytes, &digest);
    let manifest = root.path().join("manifest.json");
    let resolved = resolve_execution_modules(Some(manifest.to_string_lossy().into_owned()))
        .expect("resolve valid linked manifest");
    assert_eq!(resolved.manifest_path, manifest);
    assert_eq!(resolved.main.path(), root.path().join("program.wasm"));
    assert_eq!(resolved.main.bytes(), bytes);
    assert!(resolved.runtime.is_none());
    assert!(resolved.linked);
}

#[test]
fn runtime_manifest_resolves_and_verifies_split_modules() {
    let app_bytes = b"app wasm fixture";
    let app_digest = sha256_hex(app_bytes);
    let root = runtime_manifest_fixture("split", app_bytes, &app_digest);
    let runtime = root.path().join("runtime.wasm");
    let runtime_bytes = b"runtime wasm fixture";
    fs::write(&runtime, runtime_bytes).expect("write runtime fixture");
    let runtime_digest = sha256_hex(runtime_bytes);
    fs::write(
        root.path().join("manifest.json"),
        format!(
            r#"{{"version":{WASM_EXECUTION_MANIFEST_VERSION},"mode":"split-runtime","modules":{{"app":{{"path":"program.wasm","size":{},"sha256":"{app_digest}"}},"runtime":{{"path":"runtime.wasm","size":{},"sha256":"{runtime_digest}"}}}}}}"#,
            app_bytes.len(),
            runtime_bytes.len(),
        ),
    )
    .expect("write split manifest");
    let resolved = resolve_execution_modules(Some(
        root.path()
            .join("manifest.json")
            .to_string_lossy()
            .into_owned(),
    ))
    .expect("resolve valid split manifest");
    assert_eq!(resolved.main.path(), root.path().join("program.wasm"));
    assert_eq!(resolved.main.bytes(), app_bytes);
    let runtime_source = resolved.runtime.as_ref().expect("resolved runtime");
    assert_eq!(runtime_source.path(), runtime);
    assert_eq!(runtime_source.bytes(), runtime_bytes);
    assert!(!resolved.linked);
}

#[test]
fn runtime_manifest_rejects_digest_drift() {
    let root = runtime_manifest_fixture("digest-drift", b"linked wasm fixture", &"0".repeat(64));
    let manifest = root.path().join("manifest.json");
    let error = resolve_execution_modules(Some(manifest.to_string_lossy().into_owned()))
        .expect_err("digest drift must fail");
    assert!(error.to_string().contains("linked SHA-256 mismatch"));
}

#[test]
fn admitted_manifest_bytes_are_the_bytes_compiled_after_path_replacement() {
    let original = br#"(module (func (export "answer") (result i32) i32.const 17))"#;
    let replacement = br#"(module (func (export "answer") (result i32) i32.const 99))"#;
    let root = runtime_manifest_fixture("immutable-source", original, &sha256_hex(original));
    let resolved = resolve_execution_modules(Some(
        root.path()
            .join("manifest.json")
            .to_string_lossy()
            .into_owned(),
    ))
    .expect("admit original module bytes");
    fs::write(resolved.main.path(), replacement).expect("replace admitted path");
    assert_eq!(resolved.main.bytes(), original);
    let engine = build_engine().expect("production host engine");
    let module = super::load_or_compile_module(&engine, &resolved.main, super::ModuleRole::Main)
        .expect("compile owned original bytes");
    let mut store = Store::new(&engine, ());
    let instance =
        wasmtime::Instance::new(&mut store, &module, &[]).expect("instantiate original program");
    assert_eq!(
        instance
            .get_typed_func::<(), i32>(&mut store, "answer")
            .unwrap()
            .call(&mut store, ())
            .unwrap(),
        17,
    );
}

#[test]
fn runtime_manifest_rejects_version_and_nonportable_asset_paths() {
    let bytes = b"module fixture";
    let root = runtime_manifest_fixture("manifest-admission", bytes, &sha256_hex(bytes));
    let path = root.path().join("manifest.json");
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // Remove the module: an old receiver must be rejected before even reading
    // its bytes, rather than reaching a misleading missing-artifact failure.
    fs::remove_file(root.path().join("program.wasm")).unwrap();
    for version in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(0)),
        Some(serde_json::json!(1)),
        Some(serde_json::json!(2)),
        Some(serde_json::json!(4)),
        Some(serde_json::json!(99)),
        Some(serde_json::json!(true)),
        Some(serde_json::json!(false)),
        Some(serde_json::json!("3")),
        Some(serde_json::json!(3.0)),
    ] {
        let mut manifest = original.clone();
        match version {
            Some(version) => manifest["version"] = version,
            None => {
                manifest.as_object_mut().unwrap().remove("version");
            }
        }
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let error =
            resolve_execution_modules(Some(path.to_string_lossy().into_owned())).unwrap_err();
        assert!(
            format!("{error:#}").contains("Molt deployment manifest version must be 3"),
            "{error:#}",
        );
    }
    fs::write(root.path().join("program.wasm"), bytes).unwrap();
    for asset in [
        "../program.wasm",
        "/program.wasm",
        "dir/program.wasm",
        "dir\\program.wasm",
        "C:program.wasm",
        "program.wasm:stream",
    ] {
        let mut manifest = original.clone();
        manifest["modules"]["linked"]["path"] = asset.into();
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let error =
            resolve_execution_modules(Some(path.to_string_lossy().into_owned())).unwrap_err();
        assert!(error.to_string().contains("adjacent file"), "{error:#}");
    }
}

#[test]
fn explicit_wasi_command_resolves_without_a_runtime_manifest() {
    let root = tempfile::Builder::new()
        .prefix("molt-wasm-host-command-")
        .tempdir()
        .expect("create command fixture root");
    let module = root.path().join("command.wasm");
    let bytes = br#"(module (func (export "_start")))"#;
    fs::write(&module, bytes).expect("write command fixture");

    let resolved = resolve_execution(ExecutionRequest::WasiCommand {
        module: module.to_string_lossy().into_owned(),
    })
    .expect("resolve explicit command module");
    match resolved {
        ResolvedExecution::WasiCommand { module: source } => {
            assert_eq!(source.path(), module);
            assert_eq!(source.bytes(), bytes);
            let engine = build_engine().expect("production host engine");
            let command = Module::new(&engine, source.bytes()).unwrap();
            assert_eq!(
                execute_test_guest(
                    &engine,
                    &command,
                    None,
                    LoadedGuestKind::WasiCommand,
                    &[],
                    None
                )
                .unwrap(),
                GuestTermination::Returned,
            );
        }
        ResolvedExecution::MoltApplication(_) | ResolvedExecution::WasiCommandManifest(_) => {
            panic!("raw command resolved through a manifest")
        }
    }
}

#[test]
fn explicit_wasi_command_cannot_import_molt_without_current_manifest() {
    let engine = build_engine().expect("production host engine");
    for (namespace, name) in [
        ("env", "molt_socket_getaddrinfo_host"),
        ("molt_runtime", "molt_int_from_i64"),
        ("molt_native", "native_entry"),
    ] {
        let module = Module::new(
            &engine,
            format!(
                r#"(module
                (import "{namespace}" "{name}" (func))
                (func $unreachable_start unreachable) (start $unreachable_start)
                (func (export "_start")))"#
            ),
        )
        .unwrap();
        let error = execute_test_guest(
            &engine,
            &module,
            None,
            LoadedGuestKind::WasiCommand,
            &[],
            None,
        )
        .unwrap_err();
        assert!(
            format!("{error:#}")
                .contains("Molt host imports require a current deployment manifest"),
            "{namespace}.{name}: {error:#}",
        );
        assert!(
            !error
                .chain()
                .any(|cause| cause.to_string().contains("unreachable")),
            "command started before import admission: {error:#}",
        );
    }
}

#[test]
fn command_indirect_dispatch_cannot_admit_manifest_bound_services() {
    let engine = build_engine().expect("production host engine");
    for (namespace, name) in [
        ("env", "molt_socket_getaddrinfo_host"),
        ("molt_runtime", "molt_int_from_i64"),
        ("molt_native", "native_entry"),
    ] {
        let module = Module::new(
            &engine,
            format!(
                r#"(module
                (import "env" "molt_call_indirect0"
                  (func $dispatch (param i64) (result i64)))
                (import "{namespace}" "{name}" (func))
                (table (export "__indirect_function_table") 2 funcref)
                (func $target (result i64) i64.const 42)
                (elem (i32.const 1) $target)
                (func $unreachable_start unreachable) (start $unreachable_start)
                (func (export "_start") i64.const 1 call $dispatch drop))"#
            ),
        )
        .expect("valid indirect dispatch and service import fixture");
        let error = execute_test_guest(
            &engine,
            &module,
            None,
            LoadedGuestKind::WasiCommand,
            &[],
            None,
        )
        .unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(
            diagnostic.contains("Molt host imports require a current deployment manifest"),
            "{diagnostic}",
        );
        assert!(
            diagnostic.contains(&format!("{namespace}.{name}")),
            "{diagnostic}",
        );
        assert!(
            !error
                .chain()
                .any(|cause| cause.to_string().contains("unreachable")),
            "command started before service import admission: {error:#}",
        );
    }
}

#[test]
fn command_selector_separates_host_options_from_guest_argv() {
    let parsed = parse_host_args(
        [
            "--bundle",
            "bundle.pack",
            "--vfs-tmp-quota",
            "8",
            "--wasi-command",
            "tests.wasm",
            "--",
            "--exact",
            "shutdown_custody",
        ]
        .map(str::to_string),
    )
    .expect("parse command arguments");
    let ParsedHostArgs::Run(options) = parsed else {
        panic!("command arguments parsed as help");
    };
    match options.execution {
        ExecutionRequest::WasiCommand { module } => assert_eq!(module, "tests.wasm"),
        ExecutionRequest::MoltApplication { .. } | ExecutionRequest::WasiCommandManifest { .. } => {
            panic!("raw command parsed through a manifest")
        }
    }
    assert_eq!(options.bundle_path.as_deref(), Some("bundle.pack"));
    assert_eq!(options.vfs_tmp_quota, Some(8));
    assert_eq!(
        options.guest_args,
        ["--exact".to_string(), "shutdown_custody".to_string()]
    );
}

#[test]
fn application_selector_keeps_manifest_out_of_guest_argv() {
    let parsed = parse_host_args(["manifest.json", "route", "query"].map(str::to_string))
        .expect("parse application arguments");
    let ParsedHostArgs::Run(options) = parsed else {
        panic!("application arguments parsed as help");
    };
    match options.execution {
        ExecutionRequest::MoltApplication { manifest } => {
            assert_eq!(manifest.as_deref(), Some("manifest.json"));
        }
        ExecutionRequest::WasiCommand { .. } | ExecutionRequest::WasiCommandManifest { .. } => {
            panic!("application parsed as command")
        }
    }
    assert_eq!(
        options.guest_args,
        ["route".to_string(), "query".to_string()]
    );
}

#[test]
fn host_rejects_removed_executable_snapshot_options() {
    let error = parse_host_args(
        [
            "--snapshot-restore",
            "snapshot.bin",
            "--wasi-command",
            "tests.wasm",
        ]
        .map(str::to_string),
    )
    .expect_err("removed executable snapshot option must fail");
    assert!(
        error
            .to_string()
            .contains("does not support executable snapshots")
    );
}

#[test]
fn precompile_selector_accepts_only_execution_inputs() {
    for args in [
        vec!["--precompile"],
        vec!["--precompile", "manifest.json"],
        vec!["--precompile", "--wasi-command", "module.wasm"],
    ] {
        let parsed = parse_host_args(args.into_iter().map(str::to_owned)).unwrap();
        assert!(matches!(parsed, ParsedHostArgs::Precompile(_)));
    }
    for args in [
        vec!["--precompile", "--precompile"],
        vec!["--precompile", "manifest.json", "guest"],
        vec![
            "--precompile",
            "--wasi-command",
            "module.wasm",
            "--",
            "guest",
        ],
        vec!["--bundle", "bundle.pack", "--precompile", "manifest.json"],
        vec!["--precompile", "--vfs-tmp-quota", "8", "manifest.json"],
    ] {
        let error = parse_host_args(args.into_iter().map(str::to_owned)).unwrap_err();
        assert!(error.to_string().contains("--precompile"), "{error:#}");
    }
    // After an execution selector, host-looking options remain guest argv.
    let parsed = parse_host_args(["manifest.json", "--precompile"].map(str::to_owned)).unwrap();
    let ParsedHostArgs::Run(options) = parsed else {
        panic!("expected run");
    };
    assert_eq!(options.guest_args, ["--precompile"]);
}

#[test]
fn execution_kind_rejects_runtime_import_drift() {
    let engine = build_engine().expect("production host engine");
    let standalone = Module::new(
        &engine,
        r#"(module (import "wasi_snapshot_preview1" "proc_exit" (func (param i32))))"#,
    )
    .expect("valid standalone import fixture");
    let split = Module::new(
        &engine,
        r#"(module (import "molt_runtime" "anchor" (func)))"#,
    )
    .expect("valid runtime import fixture");
    assert!(
        validate_execution_imports(LoadedGuestKind::WasiCommand, &split, None)
            .unwrap_err()
            .to_string()
            .contains("Molt host imports require a current deployment manifest")
    );
    assert!(
        validate_execution_imports(
            LoadedGuestKind::MoltApplication { linked: true },
            &split,
            None
        )
        .unwrap_err()
        .to_string()
        .contains("linked wasm still imports molt_runtime")
    );
    assert!(
        validate_execution_imports(
            LoadedGuestKind::MoltApplication { linked: false },
            &standalone,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("split-runtime app does not import molt_runtime")
    );
    validate_execution_imports(LoadedGuestKind::WasiCommand, &standalone, None)
        .expect("standalone command imports");
    validate_execution_imports(
        LoadedGuestKind::MoltApplication { linked: true },
        &standalone,
        None,
    )
    .expect("linked application imports");
    validate_execution_imports(
        LoadedGuestKind::MoltApplication { linked: false },
        &split,
        None,
    )
    .expect("split application imports");
}

#[test]
fn native_wasmtime_context_preserves_typed_exit_and_trap_causes() {
    let exit = wasmtime::Error::from(I32Exit(23)).context("run WASI command");
    assert_eq!(exit.downcast_ref::<I32Exit>().map(|exit| exit.0), Some(23));
    assert_eq!(
        exit.chain().next().map(ToString::to_string).as_deref(),
        Some("run WASI command")
    );

    let trap = wasmtime::Error::from(wasmtime::Trap::UnreachableCodeReached)
        .context("call guest entrypoint");
    assert_eq!(
        trap.downcast_ref::<wasmtime::Trap>(),
        Some(&wasmtime::Trap::UnreachableCodeReached)
    );
    assert_eq!(
        trap.chain().next().map(ToString::to_string).as_deref(),
        Some("call guest entrypoint")
    );
}

#[test]
fn isolate_host_imports_are_registered_with_runtime_abi_shapes() {
    let engine = build_engine().expect("production host engine");
    let mut store = Store::new(&engine, test_host_state());
    let mut linker = Linker::new(&engine);

    define_isolate_host_imports(&mut linker, &mut store, &engine).unwrap();

    let bootstrap = linker
        .get(&mut store, "env", "molt_isolate_bootstrap")
        .expect("molt_isolate_bootstrap env linker item")
        .into_func()
        .expect("molt_isolate_bootstrap env import");
    let bootstrap_ty = bootstrap.ty(&store);
    let mut bootstrap_params = bootstrap_ty.params();
    assert!(bootstrap_params.next().is_none());
    let mut bootstrap_results = bootstrap_ty.results();
    assert!(matches!(bootstrap_results.next(), Some(ValType::I64)));
    assert!(bootstrap_results.next().is_none());

    let isolate_import = linker
        .get(&mut store, "env", "molt_isolate_import")
        .expect("molt_isolate_import env linker item")
        .into_func()
        .expect("molt_isolate_import env import");
    let import_ty = isolate_import.ty(&store);
    let mut import_params = import_ty.params();
    assert!(matches!(import_params.next(), Some(ValType::I64)));
    assert!(import_params.next().is_none());
    let mut import_results = import_ty.results();
    assert!(matches!(import_results.next(), Some(ValType::I64)));
    assert!(import_results.next().is_none());

    let exported_bootstrap = Func::wrap(&mut store, || -> i64 { 41 });
    let exported_import = Func::wrap(&mut store, |name_bits: i64| -> i64 { name_bits + 1 });
    store.data_mut().isolate_bootstrap_export = Some(exported_bootstrap);
    store.data_mut().isolate_import_export = Some(exported_import);

    let mut bootstrap_results = [Val::I64(0)];
    bootstrap
        .call(&mut store, &[], &mut bootstrap_results)
        .expect("bootstrap bridge call");
    assert!(matches!(bootstrap_results[0], Val::I64(41)));

    let mut import_results = [Val::I64(0)];
    isolate_import
        .call(&mut store, &[Val::I64(41)], &mut import_results)
        .expect("import bridge call");
    assert!(matches!(import_results[0], Val::I64(42)));
}

#[test]
fn module_entrypoint_abis_reject_before_wasm_start_sections_can_run() {
    let engine = build_engine().expect("production host engine");
    let command = Module::new(
        &engine,
        r#"
        (module
          (func $initialize unreachable)
          (start $initialize)
          (func (export "_start") (result i32) i32.const 0))
    "#,
    )
    .unwrap();
    let error = execute_test_guest(
        &engine,
        &command,
        None,
        LoadedGuestKind::WasiCommand,
        &[],
        None,
    )
    .expect_err("malformed command ABI must fail before its core start section");
    assert!(error.to_string().contains("malformed _start"));
    assert!(
        !error
            .chain()
            .any(|cause| cause.to_string().contains("unreachable"))
    );

    for (malformed_export, definitions) in [
        (
            "molt_main",
            r#"(func (export "molt_main"))
               (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
               (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)
               (func (export "molt_main_exit_status") (result i64) i64.const 0)
               (func (export "molt_exception_pending") (result i64) i64.const 0)"#,
        ),
        (
            "molt_isolate_bootstrap",
            r#"(func (export "molt_main") (result i64) i64.const 0)
               (func (export "molt_isolate_bootstrap"))
               (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)
               (func (export "molt_main_exit_status") (result i64) i64.const 0)
               (func (export "molt_exception_pending") (result i64) i64.const 0)"#,
        ),
        (
            "molt_isolate_import",
            r#"(func (export "molt_main") (result i64) i64.const 0)
               (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
               (func (export "molt_isolate_import") (param i32) (result i64) i64.const 0)
               (func (export "molt_main_exit_status") (result i64) i64.const 0)
               (func (export "molt_exception_pending") (result i64) i64.const 0)"#,
        ),
        (
            "molt_exception_pending",
            r#"(func (export "molt_main") (result i64) i64.const 0)
               (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
               (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)
               (func (export "molt_main_exit_status") (result i64) i64.const 0)
               (func (export "molt_exception_pending") (result i32) i32.const 0)"#,
        ),
    ] {
        let application = Module::new(
            &engine,
            format!(
                r#"(module
                      (func $initialize unreachable)
                      (start $initialize)
                      {definitions})"#
            ),
        )
        .unwrap();
        let error = execute_test_guest(
            &engine,
            &application,
            None,
            LoadedGuestKind::MoltApplication { linked: true },
            &[],
            None,
        )
        .expect_err("malformed application ABI must fail before its core start section");
        assert!(error.to_string().contains(malformed_export));
        assert!(
            !error
                .chain()
                .any(|cause| cause.to_string().contains("unreachable")),
            "{malformed_export} admission instantiated the application"
        );
    }

    let application = Module::new(
        &engine,
        r#"(module
              (import "molt_runtime" "anchor" (func))
              (func (export "molt_main") (result i64) i64.const 0)
              (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
              (func (export "molt_isolate_import") (param i64) (result i64) local.get 0))"#,
    )
    .unwrap();
    let runtime = Module::new(
        &engine,
        r#"(module
              (func $initialize unreachable)
              (start $initialize)
              (func (export "molt_anchor"))
              (func (export "molt_main_exit_status") (result i64) i64.const 0)
              (func (export "molt_exception_pending") (result i32) i32.const 0))"#,
    )
    .unwrap();
    let error = execute_test_guest(
        &engine,
        &application,
        Some(&runtime),
        LoadedGuestKind::MoltApplication { linked: false },
        &[],
        None,
    )
    .expect_err("malformed split runtime ABI must fail before its core start section");
    assert!(error.to_string().contains("molt_exception_pending"));
    assert!(
        !error
            .chain()
            .any(|cause| cause.to_string().contains("unreachable"))
    );
}

#[test]
fn module_entrypoint_abis_reject_missing_parameters_and_result_drift() {
    let engine = build_engine().expect("production host engine");
    for definition in [
        "",
        r#"(func (export "_start") (param i32))"#,
        r#"(func (export "_start") (result i32) i32.const 0)"#,
    ] {
        let command = Module::new(&engine, format!("(module {definition})")).unwrap();
        assert!(
            validate_guest_module_entrypoint(GuestModuleEntrypoint::WasiCommand {
                command: &command,
            })
            .is_err()
        );
    }

    for definition in [
        "",
        r#"(func (export "molt_main"))"#,
        r#"(func (export "molt_main") (param i64) (result i64) i64.const 0)"#,
        r#"(func (export "molt_main") (result i32) i32.const 0)"#,
    ] {
        let application = Module::new(
            &engine,
            format!(
                r#"(module
                    {definition}
                    (func (export "molt_main_exit_status") (result i64) i64.const 0)
                    (func (export "molt_exception_pending") (result i64) i64.const 0))"#
            ),
        )
        .unwrap();
        assert!(
            validate_guest_module_entrypoint(GuestModuleEntrypoint::MoltApplication {
                application: &application,
                runtime: &application,
            })
            .is_err()
        );
    }
}

#[test]
fn command_execution_calls_only_typed_start() {
    let engine = build_engine().expect("production host engine");
    let module = Module::new(
        &engine,
        r#"
        (module
          (func (export "molt_main") (result i64) unreachable)
          (func (export "_start")))
    "#,
    )
    .unwrap();
    assert_eq!(
        execute_test_guest(
            &engine,
            &module,
            None,
            LoadedGuestKind::WasiCommand,
            &[],
            None,
        )
        .unwrap(),
        GuestTermination::Returned
    );
}

#[test]
fn command_entrypoint_preserves_wasm_traps() {
    let engine = build_engine().expect("production host engine");
    let trap_module =
        Module::new(&engine, r#"(module (func (export "_start") unreachable))"#).unwrap();
    let trap = execute_test_guest(
        &engine,
        &trap_module,
        None,
        LoadedGuestKind::WasiCommand,
        &[],
        None,
    )
    .unwrap_err();
    assert!(
        trap.chain()
            .any(|cause| cause.to_string().contains("unreachable"))
    );
}

#[derive(Clone, Copy)]
enum WasiProcExitSite {
    CoreStart,
    ExportedStart,
}

impl WasiProcExitSite {
    fn label(self) -> &'static str {
        match self {
            Self::CoreStart => "core start",
            Self::ExportedStart => "exported _start",
        }
    }

    fn error_context(self) -> &'static str {
        match self {
            Self::CoreStart => "instantiate WASI command",
            Self::ExportedStart => "call _start WASI command entrypoint",
        }
    }
}

fn wasi_proc_exit_command(engine: &Engine, site: WasiProcExitSite, status: i32) -> Module {
    let entrypoints = match site {
        WasiProcExitSite::CoreStart => format!(
            r#"(func $initialize i32.const {status} call $exit)
               (start $initialize)
               (func (export "_start") unreachable)"#
        ),
        WasiProcExitSite::ExportedStart => {
            format!(r#"(func (export "_start") i32.const {status} call $exit)"#)
        }
    };
    Module::new(
        engine,
        format!(
            r#"(module
                  (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
                  (memory (export "memory") 1)
                  {entrypoints})"#
        ),
    )
    .unwrap()
}

#[test]
fn command_core_start_wasi_exit_uses_command_termination_semantics() {
    let engine = build_engine().expect("production host engine");
    for status in [0, 17, 125] {
        let module = wasi_proc_exit_command(&engine, WasiProcExitSite::CoreStart, status);
        assert_eq!(
            execute_test_guest(
                &engine,
                &module,
                None,
                LoadedGuestKind::WasiCommand,
                &[],
                None,
            )
            .unwrap(),
            GuestTermination::WasiExit(status),
            "core-start proc_exit status {status}"
        );
    }
}

#[test]
fn command_exported_start_wasi_exit_uses_command_termination_semantics() {
    let engine = build_engine().expect("production host engine");
    for status in [0, 17, 125] {
        let module = wasi_proc_exit_command(&engine, WasiProcExitSite::ExportedStart, status);
        assert_eq!(
            execute_test_guest(
                &engine,
                &module,
                None,
                LoadedGuestKind::WasiCommand,
                &[],
                None,
            )
            .unwrap(),
            GuestTermination::WasiExit(status),
            "exported _start proc_exit status {status}"
        );
    }
}

#[test]
fn command_wasi_exit_rejects_status_outside_preview1_range() {
    let engine = build_engine().expect("production host engine");
    for site in [WasiProcExitSite::CoreStart, WasiProcExitSite::ExportedStart] {
        let error = execute_test_guest(
            &engine,
            &wasi_proc_exit_command(&engine, site, 126),
            None,
            LoadedGuestKind::WasiCommand,
            &[],
            None,
        )
        .expect_err("invalid WASIp1 exit status must remain a host error");
        assert!(
            error.downcast_ref::<I32Exit>().is_none(),
            "{} invalid status was misclassified as command termination",
            site.label()
        );
        assert!(
            error.to_string().contains(site.error_context()),
            "{} invalid status lost execution-phase context: {error:#}",
            site.label()
        );
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string().contains("outside of [0..126)")),
            "{} invalid status lost the upstream WASIp1 range error: {error:#}",
            site.label()
        );
    }
}

#[test]
fn command_core_start_non_exit_trap_keeps_instantiation_context() {
    let engine = build_engine().expect("production host engine");
    let module = Module::new(
        &engine,
        r#"(module
              (func $initialize unreachable)
              (start $initialize)
              (func (export "_start")))"#,
    )
    .unwrap();
    let error = execute_test_guest(
        &engine,
        &module,
        None,
        LoadedGuestKind::WasiCommand,
        &[],
        None,
    )
    .expect_err("non-exit core start trap must remain a host error");
    assert!(error.to_string().contains("instantiate WASI command"));
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string().contains("unreachable"))
    );
}

#[test]
fn wasi_argv_is_explicit_even_without_a_guest_tail() {
    let engine = build_engine().expect("production host engine");
    let module = Module::new(
        &engine,
        r#"
        (module
          (import "wasi_snapshot_preview1" "args_sizes_get"
            (func $args_sizes_get (param i32 i32) (result i32)))
          (memory (export "memory") 1)
          (func (export "_start")
            i32.const 0
            i32.const 4
            call $args_sizes_get
            drop
            i32.const 0
            i32.load
            i32.const 1
            i32.ne
            if unreachable end))
    "#,
    )
    .unwrap();
    assert_eq!(
        execute_test_guest(
            &engine,
            &module,
            None,
            LoadedGuestKind::WasiCommand,
            &[],
            None,
        )
        .unwrap(),
        GuestTermination::Returned
    );
}

#[test]
fn command_mode_never_configures_molt_table_base() {
    let engine = build_engine().expect("production host engine");
    let module = Module::new(
        &engine,
        r#"
        (module
          (table 1 funcref)
          (func $target)
          (elem (i32.const 0) $target)
          (func (export "molt_set_wasm_table_base") (param i64) unreachable)
          (func (export "_start")))
    "#,
    )
    .unwrap();
    assert_eq!(
        execute_test_guest(
            &engine,
            &module,
            None,
            LoadedGuestKind::WasiCommand,
            &[],
            Some(37),
        )
        .unwrap(),
        GuestTermination::Returned
    );
}

#[test]
fn standalone_command_memory_and_table_imports_need_no_runtime_module() {
    let engine = build_engine().expect("production host engine");
    let module = Module::new(
        &engine,
        r#"
        (module
          (import "env" "memory" (memory 1))
          (import "env" "__indirect_function_table" (table 1 funcref))
          (func (export "_start")))
    "#,
    )
    .unwrap();
    assert_eq!(
        execute_test_guest(
            &engine,
            &module,
            None,
            LoadedGuestKind::WasiCommand,
            &[],
            None,
        )
        .unwrap(),
        GuestTermination::Returned
    );
}

#[test]
fn loaded_guest_primitive_executes_linked_application_contract() {
    let engine = build_engine().expect("production host engine");
    let application = Module::new(
        &engine,
        r#"
        (module
          (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
          (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)
          (func (export "molt_main_exit_status") (result i64) i64.const 0)
          (func (export "molt_exception_pending") (result i64) i64.const 0)
          (func (export "molt_runtime_execution_enter") (result i64) i64.const 41)
          (func (export "molt_runtime_execution_leave") (param i64))
          (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
          (func (export "molt_main") (result i64) i64.const 0))
    "#,
    )
    .unwrap();
    assert_eq!(
        execute_test_guest(
            &engine,
            &application,
            None,
            LoadedGuestKind::MoltApplication { linked: true },
            &[],
            None,
        )
        .unwrap(),
        GuestTermination::Returned
    );
}

#[test]
fn loaded_guest_primitive_executes_split_application_contract() {
    let engine = build_engine().expect("production host engine");
    let runtime = Module::new(
        &engine,
        r#"
        (module
          (func (export "molt_anchor"))
          (func (export "molt_runtime_execution_enter") (result i64) i64.const 41)
          (func (export "molt_runtime_execution_leave") (param i64))
          (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
          (func (export "molt_main_exit_status") (result i64) i64.const 7)
          (func (export "molt_exception_pending") (result i64) i64.const 0))
    "#,
    )
    .unwrap();
    let application = Module::new(
        &engine,
        r#"
        (module
          (import "molt_runtime" "anchor" (func $anchor))
          (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
          (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)
          (func (export "molt_main_exit_status") (result i64) unreachable)
          (func (export "molt_main") (result i64) call $anchor i64.const 0))
    "#,
    )
    .unwrap();
    assert_eq!(
        execute_test_guest(
            &engine,
            &application,
            Some(&runtime),
            LoadedGuestKind::MoltApplication { linked: false },
            &[],
            None,
        )
        .unwrap(),
        GuestTermination::ApplicationExit(7)
    );
}

fn execute_split_import_fixture(
    engine: &Engine,
    runtime_fields: &str,
    application_fields: &str,
    main_body: &str,
    trap_core_starts: bool,
) -> wasmtime::Result<GuestTermination> {
    let core_start = if trap_core_starts {
        "(func $initialize unreachable) (start $initialize)"
    } else {
        ""
    };
    let runtime = Module::new(
        engine,
        format!(
            r#"(module
                {runtime_fields}
                {core_start}
                (func (export "molt_runtime_execution_enter") (result i64) i64.const 41)
                (func (export "molt_runtime_execution_leave") (param i64))
                (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
                (func (export "molt_main_exit_status") (result i64) i64.const 0)
                (func (export "molt_exception_pending") (result i64) i64.const 0))"#
        ),
    )
    .expect("valid runtime fixture");
    // Every member of this fixture family is a real split-runtime application,
    // including cases whose injected imports belong only to env/WASI. Reuse
    // the canonical exception query already exported by the runtime fixture so
    // protocol admission cannot mask the particular import error under test.
    let application = Module::new(
        engine,
        format!(
            r#"(module
                (import "molt_runtime" "exception_pending"
                    (func $fixture_exception_pending (result i64)))
                {application_fields}
                {core_start}
                (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
                (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)
                (func (export "molt_main") (result i64)
                    call $fixture_exception_pending drop
                    {main_body}))"#
        ),
    )
    .expect("valid application fixture");
    validate_execution_imports(
        LoadedGuestKind::MoltApplication { linked: false },
        &application,
        Some(&runtime),
    )
    .expect("split fixture must pass protocol admission before its injected import failure");
    execute_test_guest(
        engine,
        &application,
        Some(&runtime),
        LoadedGuestKind::MoltApplication { linked: false },
        &[],
        None,
    )
}

#[test]
fn split_runtime_imports_are_admitted_before_either_core_start() {
    let engine = build_engine().expect("production host engine");
    for (runtime_fields, application_fields, diagnostic) in [
        (
            "",
            r#"(import "molt_runtime" "anchor" (func))"#,
            "missing runtime export molt_anchor",
        ),
        (
            r#"(memory (export "molt_anchor") 1)"#,
            r#"(import "molt_runtime" "anchor" (func))"#,
            "malformed runtime export molt_anchor",
        ),
        (
            r#"(global (export "molt_anchor") i64 (i64.const 0))"#,
            r#"(import "molt_runtime" "anchor" (func))"#,
            "malformed runtime export molt_anchor",
        ),
        (
            r#"(table (export "molt_anchor") 1 funcref)"#,
            r#"(import "molt_runtime" "anchor" (func))"#,
            "malformed runtime export molt_anchor",
        ),
        (
            r#"(func (export "molt_anchor") (param i64))"#,
            r#"(import "molt_runtime" "anchor" (func (param i32)))"#,
            "runtime export molt_anchor signature mismatch",
        ),
        (
            r#"(func (export "molt_anchor") (result i32) i32.const 0)"#,
            r#"(import "molt_runtime" "anchor" (func (result i64)))"#,
            "runtime export molt_anchor signature mismatch",
        ),
        (
            r#"(memory (export "molt_anchor") 1)"#,
            r#"(import "molt_runtime" "anchor" (memory 1))"#,
            "malformed molt_runtime::anchor import",
        ),
        (
            r#"(func (export "molt_anchor"))"#,
            r#"(import "molt_runtime" "anchor" (func))
               (import "molt_runtime" "anchor" (func (param i64)))"#,
            "runtime export molt_anchor signature mismatch",
        ),
    ] {
        let error = execute_split_import_fixture(
            &engine,
            runtime_fields,
            application_fields,
            "i64.const 0",
            true,
        )
        .expect_err("invalid runtime mapping must fail before either core start");
        assert!(error.to_string().contains(diagnostic), "{error:#}");
        assert!(
            error.downcast_ref::<wasmtime::Trap>().is_none(),
            "runtime mapping admission executed a guest core start: {error:#}"
        );
    }
}

#[test]
fn split_runtime_import_plan_preserves_subtypes_and_duplicate_imports() {
    let engine = build_engine().expect("production host engine");
    for (runtime_fields, application_fields) in [
        (
            r#"(func (export "molt_answer") (result i64) i64.const 42)"#,
            r#"(import "molt_runtime" "answer" (func $first (result i64)))
               (import "molt_runtime" "answer" (func $second (result i64)))"#,
        ),
        (
            r#"(type $base (sub (func (result i64))))
               (type $derived (sub $base (func (result i64))))
               (func (export "molt_answer") (type $derived) i64.const 42)"#,
            r#"(type $base (sub (func (result i64))))
               (type $derived (sub $base (func (result i64))))
               (import "molt_runtime" "answer" (func $first (type $base)))
               (import "molt_runtime" "answer" (func $second (type $derived)))"#,
        ),
    ] {
        assert_eq!(
            execute_split_import_fixture(
                &engine,
                runtime_fields,
                application_fields,
                "call $first i64.const 42 i64.ne if unreachable end
                 call $second i64.const 42 i64.ne if unreachable end i64.const 0",
                false,
            )
            .unwrap(),
            GuestTermination::Returned
        );
    }
}

#[test]
fn split_application_host_imports_are_admitted_before_runtime_core_start() {
    let engine = build_engine().expect("production host engine");
    for (application_fields, diagnostic) in [
        (
            r#"(import "env" "missing_host" (func))"#,
            "missing application host import env::missing_host",
        ),
        (
            r#"(import "wasi_snapshot_preview1" "missing_host" (func))"#,
            "missing application host import wasi_snapshot_preview1::missing_host",
        ),
        (
            r#"(import "env" "molt_getpid_host" (func (param i64) (result i64)))"#,
            "application host import env::molt_getpid_host type mismatch",
        ),
        (
            r#"(import "env" "molt_getpid_host" (func (result i32)))"#,
            "application host import env::molt_getpid_host type mismatch",
        ),
        (
            r#"(import "env" "molt_getpid_host" (global i64))"#,
            "application host import env::molt_getpid_host type mismatch",
        ),
        (
            r#"(import "wasi_snapshot_preview1" "proc_exit" (func (param i64)))"#,
            "application host import wasi_snapshot_preview1::proc_exit type mismatch",
        ),
        (
            r#"(import "env" "memory" (memory 1 4))
               (import "env" "memory" (memory 2 4))"#,
            "application host import env::memory type mismatch",
        ),
        (
            r#"(import "env" "__indirect_function_table" (table 1 4 funcref))
               (import "env" "__indirect_function_table" (table 2 4 funcref))"#,
            "application host import env::__indirect_function_table type mismatch",
        ),
        (
            r#"(import "env" "__indirect_function_table" (table 1 funcref))
               (import "env" "__indirect_function_table" (table 1 externref))"#,
            "application host import env::__indirect_function_table type mismatch",
        ),
    ] {
        let error =
            execute_split_import_fixture(&engine, "", application_fields, "i64.const 0", true)
                .expect_err("invalid application host import must fail before runtime core start");
        assert!(error.to_string().contains(diagnostic), "{error:#}");
        assert!(
            error.downcast_ref::<wasmtime::Trap>().is_none(),
            "{error:#}"
        );
    }
}

#[test]
fn split_runtime_host_imports_use_native_preinstantiation_admission() {
    let engine = build_engine().expect("production host engine");
    let error = execute_split_import_fixture(
        &engine,
        r#"(import "env" "molt_getpid_host" (func (result i32)))"#,
        "",
        "i64.const 0",
        true,
    )
    .expect_err("invalid runtime host import must fail before its core start");
    assert!(
        error.to_string().contains("admit runtime host imports"),
        "{error:#}"
    );
    assert!(
        format!("{error:#}").contains("molt_getpid_host"),
        "{error:#}"
    );
    assert!(
        error.downcast_ref::<wasmtime::Trap>().is_none(),
        "{error:#}"
    );
}

#[test]
fn split_runtime_admission_accepts_shared_host_definitions_and_preserves_start_traps() {
    let engine = build_engine().expect("production host engine");
    let runtime_fields = r#"
        (import "env" "memory" (memory 2 4))
        (import "env" "__indirect_function_table" (table 2 4 funcref))
        (func (export "molt_anchor") (result i64) i64.const 42)"#;
    let application_fields = r#"
        (import "env" "memory" (memory 1 5))
        (import "env" "__indirect_function_table" (table 1 5 funcref))
        (import "env" "molt_getpid_host" (func $getpid (result i64)))
        (import "wasi_snapshot_preview1" "proc_exit" (func (param i32)))
        (import "molt_runtime" "anchor" (func $anchor (result i64)))"#;
    let main_body = "call $getpid drop
        call $anchor i64.const 42 i64.ne if unreachable end
        memory.size i32.const 2 i32.ne if unreachable end
        table.size 0 i32.const 2 i32.ne if unreachable end i64.const 0";
    assert_eq!(
        execute_split_import_fixture(
            &engine,
            runtime_fields,
            application_fields,
            main_body,
            false,
        )
        .unwrap(),
        GuestTermination::Returned
    );
    let error =
        execute_split_import_fixture(&engine, runtime_fields, application_fields, main_body, true)
            .expect_err("valid import admission must preserve the runtime core-start trap");
    assert!(
        error.to_string().contains("instantiate runtime"),
        "{error:#}"
    );
    assert!(
        error.downcast_ref::<wasmtime::Trap>().is_some(),
        "{error:#}"
    );
}

#[test]
fn app_startup_calls_only_molt_main_wrapper() {
    let engine = build_engine().expect("production host engine");
    let mut store = Store::new(&engine, test_host_state());
    let mut linker = Linker::new(&engine);
    let order = Arc::new(Mutex::new(Vec::new()));
    let bootstrap_order = Arc::clone(&order);
    let main_order = Arc::clone(&order);
    let start_order = Arc::clone(&order);
    let report_order = Arc::clone(&order);
    let mark_bootstrap = Func::wrap(&mut store, move || {
        bootstrap_order.lock().unwrap().push("bootstrap");
    });
    let mark_main = Func::wrap(&mut store, move || {
        main_order.lock().unwrap().push("main");
    });
    let mark_start = Func::wrap(&mut store, move || {
        start_order.lock().unwrap().push("start");
    });
    linker
        .define(&mut store, "env", "mark_bootstrap", mark_bootstrap)
        .unwrap();
    linker
        .define(&mut store, "env", "mark_main", mark_main)
        .unwrap();
    linker
        .define(&mut store, "env", "mark_start", mark_start)
        .unwrap();
    let mark_report = Func::wrap(&mut store, move || {
        report_order.lock().unwrap().push("report");
    });
    linker
        .define(&mut store, "env", "mark_report", mark_report)
        .unwrap();
    let module = Module::new(
        &engine,
        r#"
            (module
              (import "env" "mark_bootstrap" (func $mark_bootstrap))
              (import "env" "mark_main" (func $mark_main))
              (import "env" "mark_start" (func $mark_start))
              (import "env" "mark_report" (func $mark_report))
              (func (export "molt_isolate_bootstrap") (result i64)
                call $mark_bootstrap
                i64.const 0)
              (func (export "molt_isolate_import") (param i64) (result i64)
                local.get 0)
              (func (export "molt_main_exit_status") (result i64) call $mark_report i64.const 0)
              (func (export "molt_exception_pending") (result i64)
                i64.const 0)
              (func (export "molt_main") (result i64)
                call $mark_main
                i64.const 0)
              (func (export "_start")
                call $mark_start))
            "#,
    )
    .unwrap();
    let instance = linker.instantiate(&mut store, &module).unwrap();

    call_test_app_startup(&mut store, &instance, &instance).unwrap();

    assert_eq!(&*order.lock().unwrap(), &["main", "report"]);
}

#[test]
fn app_startup_reports_and_consumes_pending_runtime_failure() {
    let engine = build_engine().expect("production host engine");
    let mut store = Store::new(&engine, test_host_state());
    let module = Module::new(
        &engine,
        r#"(module
        (global $pending (mut i64) (i64.const 0))
        (global $reports (mut i64) (i64.const 0))
        (func (export "molt_main") (result i64)
            i64.const 1 global.set $pending i64.const 0)
        (func (export "molt_exception_pending") (result i64) global.get $pending)
        (func (export "reports") (result i64) global.get $reports)
        (func (export "molt_main_exit_status") (result i64)
            global.get $pending i64.const 1 i64.ne if unreachable end
            global.get $reports i64.const 1 i64.add global.set $reports
            i64.const 0 global.set $pending i64.const 7))"#,
    )
    .unwrap();
    let instance = Linker::new(&engine)
        .instantiate(&mut store, &module)
        .unwrap();
    assert_eq!(
        call_test_app_startup(&mut store, &instance, &instance).unwrap(),
        GuestTermination::ApplicationExit(7)
    );
    for (name, expected) in [("molt_exception_pending", 0), ("reports", 1)] {
        let observed = instance
            .get_typed_func::<(), i64>(&mut store, name)
            .unwrap();
        assert_eq!(observed.call(&mut store, ()).unwrap(), expected);
    }
}

#[test]
fn app_startup_resolves_status_before_executing_main() {
    for status_export in [
        "",
        r#"(func (export "molt_main_exit_status") (result i64) i64.const 0)
          (func (export "molt_exception_pending") (result i32) i32.const 0)"#,
        r#"(func (export "molt_main_exit_status") (result i64) i64.const 0)
          (func (export "molt_exception_pending") (result i64) i64.const 2)"#,
    ] {
        let engine = build_engine().expect("production host engine");
        let mut store = Store::new(&engine, test_host_state());
        let module = Module::new(
            &engine,
            format!(
                r#"
            (module
              (global $calls (mut i64) (i64.const 0))
              (func (export "molt_main") (result i64)
                i64.const 1
                global.set $calls
                i64.const 0)
              (func (export "calls") (result i64) global.get $calls)
              {status_export})
        "#
            ),
        )
        .unwrap();
        let instance = Linker::new(&engine)
            .instantiate(&mut store, &module)
            .unwrap();
        let error = call_test_app_startup(&mut store, &instance, &instance)
            .expect_err("missing or malformed status export must fail closed");
        assert!(error.to_string().contains("molt_exception_pending"));
        let calls = instance
            .get_typed_func::<(), i64>(&mut store, "calls")
            .unwrap();
        assert_eq!(calls.call(&mut store, ()).unwrap(), 0);
    }
}

#[test]
fn split_startup_reads_runtime_instance_status_not_application_shadow() {
    let engine = build_engine().expect("production host engine");
    let mut store = Store::new(&engine, test_host_state());
    let runtime = Module::new(
        &engine,
        r#"
        (module (func (export "molt_main_exit_status") (result i64) i64.const 0)
          (func (export "molt_exception_pending") (result i64) i64.const 1))
    "#,
    )
    .unwrap();
    let app = Module::new(
        &engine,
        r#"
        (module
          (global $calls (mut i64) (i64.const 0))
          (func (export "molt_main") (result i64)
            global.get $calls
            i64.const 1
            i64.add
            global.set $calls
            i64.const 0)
          (func (export "calls") (result i64) global.get $calls)
          (func (export "molt_main_exit_status") (result i64) i64.const 0)
          (func (export "molt_exception_pending") (result i64) i64.const 0))
    "#,
    )
    .unwrap();
    let linker = Linker::new(&engine);
    let runtime = linker.instantiate(&mut store, &runtime).unwrap();
    let app = linker.instantiate(&mut store, &app).unwrap();
    assert!(
        call_test_app_startup(&mut store, &app, &runtime)
            .unwrap_err()
            .to_string()
            .contains("MOLT_APP_BOOTSTRAP_FAILED")
    );
    let calls = app.get_typed_func::<(), i64>(&mut store, "calls").unwrap();
    assert_eq!(
        calls.call(&mut store, ()).unwrap(),
        0,
        "preexisting runtime failure must prevent split application startup"
    );
}

#[test]
fn linked_startup_rejects_preexisting_failure_before_main() {
    let engine = build_engine().expect("production host engine");
    let mut store = Store::new(&engine, test_host_state());
    let module = Module::new(
        &engine,
        r#"
        (module
          (global $calls (mut i64) (i64.const 0))
          (func (export "molt_main") (result i64)
            global.get $calls
            i64.const 1
            i64.add
            global.set $calls
            i64.const 0)
          (func (export "calls") (result i64) global.get $calls)
          (func (export "molt_main_exit_status") (result i64) i64.const 0)
          (func (export "molt_exception_pending") (result i64) i64.const 1))
    "#,
    )
    .unwrap();
    let instance = Linker::new(&engine)
        .instantiate(&mut store, &module)
        .unwrap();
    assert!(
        call_test_app_startup(&mut store, &instance, &instance)
            .unwrap_err()
            .to_string()
            .contains("MOLT_APP_BOOTSTRAP_FAILED")
    );
    let calls = instance
        .get_typed_func::<(), i64>(&mut store, "calls")
        .unwrap();
    assert_eq!(
        calls.call(&mut store, ()).unwrap(),
        0,
        "preexisting runtime failure must prevent linked application startup"
    );
}

#[test]
fn app_startup_preserves_wasm_trap_failure() {
    let engine = build_engine().expect("production host engine");
    let mut store = Store::new(&engine, test_host_state());
    let module = Module::new(
        &engine,
        r#"
        (module
          (func (export "molt_main") (result i64) unreachable)
          (func (export "molt_main_exit_status") (result i64) i64.const 0)
          (func (export "molt_exception_pending") (result i64) i64.const 0))
    "#,
    )
    .unwrap();
    let instance = Linker::new(&engine)
        .instantiate(&mut store, &module)
        .unwrap();
    let error = call_test_app_startup(&mut store, &instance, &instance).unwrap_err();
    assert!(
        error
            .chain()
            .any(|cause| cause.to_string().contains("unreachable"))
    );
}

#[test]
fn isolate_export_registration_validates_both_abis_before_publication() {
    for definitions in [
        r#"(func (export "molt_isolate_bootstrap"))
           (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)"#,
        r#"(func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
           (func (export "molt_isolate_import") (param i32) (result i64) i64.const 0)"#,
    ] {
        let engine = build_engine().expect("production host engine");
        let mut store = Store::new(&engine, test_host_state());
        let module = Module::new(&engine, format!("(module {definitions})")).unwrap();
        let instance = Linker::new(&engine)
            .instantiate(&mut store, &module)
            .unwrap();
        let error = super::register_isolate_exports(&mut store, &instance).unwrap_err();
        assert!(error.to_string().contains("missing or malformed"));
        assert!(store.data().isolate_bootstrap_export.is_none());
        assert!(store.data().isolate_import_export.is_none());
    }
}

#[test]
fn manifest_command_selector_preserves_command_argv_and_precompile_identity() {
    let ParsedHostArgs::Run(options) = parse_host_args(
        [
            "--wasi-command-manifest",
            "manifest.json",
            "--",
            "--exact",
            "case",
        ]
        .map(str::to_string),
    )
    .unwrap() else {
        panic!("expected command run");
    };
    assert!(matches!(options.execution,
        ExecutionRequest::WasiCommandManifest { manifest } if manifest == "manifest.json"));
    assert_eq!(options.guest_args, ["--exact", "case"]);
    assert!(matches!(parse_host_args(
        ["--precompile", "--wasi-command-manifest", "manifest.json"].map(str::to_string)
    ).unwrap(), ParsedHostArgs::Precompile(ExecutionRequest::WasiCommandManifest { manifest })
        if manifest == "manifest.json"));
    for args in [
        vec!["--wasi-command-manifest"],
        vec![
            "--precompile",
            "--wasi-command-manifest",
            "manifest.json",
            "guest",
        ],
    ] {
        assert!(parse_host_args(args.into_iter().map(str::to_string)).is_err());
    }
}

#[test]
fn manifest_command_captures_source_and_cannot_admit_an_unrelated_module() {
    let original =
        br#"(module (func (export "_start")) (func (export "answer") (result i32) i32.const 17))"#;
    let root = runtime_manifest_fixture("command-capture", original, &sha256_hex(original));
    let path = root.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["entry"] = serde_json::json!({"module":"linked","function":"_start"});
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let ResolvedExecution::WasiCommandManifest(resolved) =
        resolve_execution(ExecutionRequest::WasiCommandManifest {
            manifest: path.to_string_lossy().into_owned(),
        })
        .unwrap()
    else {
        panic!("manifest did not resolve as a command");
    };
    fs::write(root.path().join("program.wasm"), b"changed after admission").unwrap();
    let engine = build_engine().unwrap();
    let command = resolved.load(&engine).unwrap();
    assert_eq!(
        molt_wasm_host::sha256_digest_hex(command.source_sha256()),
        sha256_hex(original)
    );
    let mut store = Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, command.module(), &[]).unwrap();
    assert_eq!(
        instance
            .get_typed_func::<(), i32>(&mut store, "answer")
            .unwrap()
            .call(&mut store, ())
            .unwrap(),
        17
    );
    assert_eq!(
        execute_test_guest(
            &engine,
            command.module(),
            None,
            LoadedGuestKind::WasiCommandManifest(&command),
            &[],
            None
        )
        .unwrap(),
        GuestTermination::Returned
    );
    let unrelated = Module::new(
        &engine,
        r#"(module (func $s unreachable) (start $s) (func (export "_start")))"#,
    )
    .unwrap();
    let error = execute_test_guest(
        &engine,
        &unrelated,
        None,
        LoadedGuestKind::WasiCommandManifest(&command),
        &[],
        None,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("requires its captured module"),
        "{error:#}"
    );
    assert!(!format!("{error:#}").contains("unreachable"), "{error:#}");
}

#[test]
fn split_reporter_admission_rejects_runtime_defect_despite_valid_app_shadow() {
    let engine = build_engine().unwrap();
    for reporter in [
        "",
        r#"(global (export "molt_main_exit_status") i64 (i64.const 0))"#,
        r#"(func (export "molt_main_exit_status") (param i64) (result i64) i64.const 0)"#,
        r#"(func (export "molt_main_exit_status") (result i32) i32.const 0)"#,
    ] {
        let runtime = Module::new(
            &engine,
            format!(
                r#"(module
            (func $start unreachable) (start $start)
            (func (export "molt_anchor"))
            (func (export "molt_exception_pending") (result i64) i64.const 0)
            (func (export "molt_runtime_execution_enter") (result i64) i64.const 41)
            (func (export "molt_runtime_execution_leave") (param i64))
            (func (export "molt_runtime_shutdown") (result i64) i64.const 1)
            {reporter})"#
            ),
        )
        .unwrap();
        let app = Module::new(
            &engine,
            r#"(module
            (import "molt_runtime" "anchor" (func))
            (func $start unreachable) (start $start)
            (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
            (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)
            (func (export "molt_main_exit_status") (result i64) i64.const 0)
            (func (export "molt_main") (result i64) unreachable))"#,
        )
        .unwrap();
        let error = execute_test_guest(
            &engine,
            &app,
            Some(&runtime),
            LoadedGuestKind::MoltApplication { linked: false },
            &[],
            None,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("molt_main_exit_status"),
            "{error:#}"
        );
        assert!(
            error.downcast_ref::<wasmtime::Trap>().is_none(),
            "{error:#}"
        );
    }
}
