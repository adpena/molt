use super::{
    HostState, ProcessManager, SocketManager, WebSocketManager, call_app_startup_entries,
    define_isolate_host_imports, resolve_execution_modules, select_manifest_path,
};
use sha2::Digest;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use wasmtime::{Engine, Func, Linker, Module, Store, Val, ValType};
use wasmtime_wasi::WasiCtxBuilder;

fn test_host_state() -> HostState {
    HostState {
        wasi: WasiCtxBuilder::new().build_p1(),
        memory: None,
        call_indirect: Arc::new(Mutex::new(HashMap::new())),
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

fn runtime_manifest_fixture(label: &str, module_bytes: &[u8], digest: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "molt-wasm-host-manifest-{}-{label}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("create manifest fixture root");
    fs::write(root.join("program.wasm"), module_bytes).expect("write module fixture");
    fs::write(
        root.join("manifest.json"),
        format!(
            r#"{{"version":2,"mode":"linked","modules":{{"linked":{{"path":"program.wasm","size":{},"sha256":"{digest}"}}}}}}"#,
            module_bytes.len()
        ),
    )
    .expect("write manifest fixture");
    root
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn runtime_manifest_resolves_and_verifies_linked_module() {
    let bytes = b"linked wasm fixture";
    let digest = sha256_hex(bytes);
    let root = runtime_manifest_fixture("valid", bytes, &digest);
    let manifest = root.join("manifest.json");
    let resolved = resolve_execution_modules(Some(manifest.to_string_lossy().into_owned()))
        .expect("resolve valid linked manifest");
    assert_eq!(resolved.manifest_path, manifest);
    assert_eq!(resolved.main_path, root.join("program.wasm"));
    assert!(resolved.runtime_path.is_none());
    assert!(resolved.linked);
    fs::remove_dir_all(root).expect("remove valid fixture");
}

#[test]
fn runtime_manifest_resolves_and_verifies_split_modules() {
    let app_bytes = b"app wasm fixture";
    let app_digest = sha256_hex(app_bytes);
    let root = runtime_manifest_fixture("split", app_bytes, &app_digest);
    let runtime = root.join("runtime.wasm");
    let runtime_bytes = b"runtime wasm fixture";
    fs::write(&runtime, runtime_bytes).expect("write runtime fixture");
    let runtime_digest = sha256_hex(runtime_bytes);
    fs::write(
        root.join("manifest.json"),
        format!(
            r#"{{"version":2,"mode":"split-runtime","modules":{{"app":{{"path":"program.wasm","size":{},"sha256":"{app_digest}"}},"runtime":{{"path":"runtime.wasm","size":{},"sha256":"{runtime_digest}"}}}}}}"#,
            app_bytes.len(),
            runtime_bytes.len(),
        ),
    )
    .expect("write split manifest");
    let resolved = resolve_execution_modules(Some(
        root.join("manifest.json").to_string_lossy().into_owned(),
    ))
    .expect("resolve valid split manifest");
    assert_eq!(resolved.main_path, root.join("program.wasm"));
    assert_eq!(resolved.runtime_path, Some(runtime));
    assert!(!resolved.linked);
    fs::remove_dir_all(root).expect("remove split fixture");
}

#[test]
fn runtime_manifest_rejects_digest_drift() {
    let root = runtime_manifest_fixture("digest-drift", b"linked wasm fixture", &"0".repeat(64));
    let manifest = root.join("manifest.json");
    let error = resolve_execution_modules(Some(manifest.to_string_lossy().into_owned()))
        .expect_err("digest drift must fail");
    assert!(error.to_string().contains("linked SHA-256 mismatch"));
    fs::remove_dir_all(root).expect("remove drift fixture");
}

#[test]
fn isolate_host_imports_are_registered_with_runtime_abi_shapes() {
    let engine = Engine::default();
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
fn app_startup_calls_only_molt_main_wrapper() {
    let engine = Engine::default();
    let mut store = Store::new(&engine, test_host_state());
    let mut linker = Linker::new(&engine);
    let order = Arc::new(Mutex::new(Vec::new()));
    let bootstrap_order = Arc::clone(&order);
    let main_order = Arc::clone(&order);
    let mark_bootstrap = Func::wrap(&mut store, move || {
        bootstrap_order.lock().unwrap().push("bootstrap");
    });
    let mark_main = Func::wrap(&mut store, move || {
        main_order.lock().unwrap().push("main");
    });
    linker
        .define(&mut store, "env", "mark_bootstrap", mark_bootstrap)
        .unwrap();
    linker
        .define(&mut store, "env", "mark_main", mark_main)
        .unwrap();
    let module = Module::new(
        &engine,
        r#"
            (module
              (import "env" "mark_bootstrap" (func $mark_bootstrap))
              (import "env" "mark_main" (func $mark_main))
              (func (export "molt_isolate_bootstrap") (result i64)
                call $mark_bootstrap
                i64.const 0)
              (func (export "molt_isolate_import") (param i64) (result i64)
                local.get 0)
              (func (export "molt_main")
                call $mark_main))
            "#,
    )
    .unwrap();
    let instance = linker.instantiate(&mut store, &module).unwrap();

    call_app_startup_entries(&mut store, &instance).unwrap();

    assert_eq!(&*order.lock().unwrap(), &["main"]);
}
