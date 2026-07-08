use super::{
    HostState, ProcessManager, SocketManager, WebSocketManager, call_app_startup_entries,
    define_isolate_host_imports, linked_path_candidates, wasm_path_candidates,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
fn wasm_path_candidates_prefer_explicit_then_canonical_dist() {
    let cwd = Path::new("/repo");
    let candidates = wasm_path_candidates(
        Some(PathBuf::from("/tmp/app.wasm")),
        Some(PathBuf::from("/env/app.wasm")),
        cwd,
    );
    assert_eq!(
        candidates,
        vec![
            PathBuf::from("/tmp/app.wasm"),
            PathBuf::from("/env/app.wasm"),
            PathBuf::from("/repo/dist/output.wasm"),
        ]
    );
}

#[test]
fn linked_path_candidates_prefer_env_then_sibling_then_canonical_dist() {
    let cwd = Path::new("/repo");
    let wasm_path = Path::new("/artifacts/output.wasm");
    let candidates = linked_path_candidates(
        wasm_path,
        Some(PathBuf::from("/env/output_linked.wasm")),
        cwd,
    );
    assert_eq!(
        candidates,
        vec![
            PathBuf::from("/env/output_linked.wasm"),
            PathBuf::from("/artifacts/output_linked.wasm"),
            PathBuf::from("/repo/dist/output_linked.wasm"),
        ]
    );
}

#[test]
fn linked_path_candidates_deduplicate_canonical_sibling() {
    let cwd = Path::new("/repo");
    let wasm_path = Path::new("/repo/dist/output.wasm");
    let candidates = linked_path_candidates(wasm_path, None, cwd);
    assert_eq!(
        candidates,
        vec![PathBuf::from("/repo/dist/output_linked.wasm")]
    );
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
