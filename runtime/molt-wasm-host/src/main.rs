use anyhow::{Context, Result, bail};
use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
use molt_runtime::vfs::snapshot::SnapshotHeader;
use num_format::{Grouping, SystemLocale};
use rmpv::Value as MsgpackValue;
use rmpv::encode::write_value;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use socket2::{Domain, Protocol, SockAddr, SockAddrStorage, Socket, Type, socklen_t};
use std::collections::{HashMap, VecDeque};
use std::env;
use std::ffi::{CStr, CString};
use std::fs;
use std::io::{BufReader, Read, Write};
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, connect};
use url::Url;
use wasmtime::{
    Cache, Caller, Config, Engine, Extern, ExternType, Func, FuncType, Instance, Linker, Memory,
    MemoryType, Module, OptLevel, Ref, Store, Table, TableType, Val, ValType,
};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder, p1};

mod process_host;
mod socket_host;
use process_host::{ProcessManager, define_process_host};
use socket_host::define_socket_host;

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, FromRawSocket, IntoRawSocket, RawSocket};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE},
    Networking::WinSock as winsock,
    System::Threading::GetCurrentProcess,
};

#[derive(Clone, Copy, Debug)]
struct Limits {
    min: u32,
    max: Option<u32>,
}

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const INT_MASK: u64 = (1 << 47) - 1;
const MAX_DB_FRAME_SIZE: usize = 64 * 1024 * 1024;
const CANCEL_POLL_MS: u64 = 10;
const CANCEL_POLL_BATCH: usize = 256;
const IO_EVENT_READ: u32 = 1;
const IO_EVENT_WRITE: u32 = 1 << 1;
const IO_EVENT_ERROR: u32 = 1 << 2;
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    molt_obj_model::MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_name_bits: u64) -> u64 {
    molt_obj_model::MoltObject::none().bits()
}

#[cfg(unix)]
const HOST_AF_INET: i32 = libc::AF_INET;
#[cfg(windows)]
const HOST_AF_INET: i32 = winsock::AF_INET as i32;
#[cfg(unix)]
const HOST_AF_INET6: i32 = libc::AF_INET6;
#[cfg(windows)]
const HOST_AF_INET6: i32 = winsock::AF_INET6 as i32;
#[cfg(unix)]
const HOST_AF_UNIX: i32 = libc::AF_UNIX;
#[cfg(windows)]
const HOST_AF_UNIX: i32 = winsock::AF_UNIX as i32;

#[cfg(unix)]
const HOST_SHUT_RD: i32 = libc::SHUT_RD;
#[cfg(windows)]
const HOST_SHUT_RD: i32 = winsock::SD_RECEIVE;
#[cfg(unix)]
const HOST_SHUT_WR: i32 = libc::SHUT_WR;
#[cfg(windows)]
const HOST_SHUT_WR: i32 = winsock::SD_SEND;

#[cfg(unix)]
const HOST_POLLIN: i16 = libc::POLLIN as i16;
#[cfg(windows)]
const HOST_POLLIN: i16 = winsock::POLLIN;
#[cfg(unix)]
const HOST_POLLOUT: i16 = libc::POLLOUT as i16;
#[cfg(windows)]
const HOST_POLLOUT: i16 = winsock::POLLOUT;
#[cfg(unix)]
const HOST_POLLERR: i16 = libc::POLLERR as i16;
#[cfg(windows)]
const HOST_POLLERR: i16 = winsock::POLLERR;
#[cfg(unix)]
const HOST_POLLHUP: i16 = libc::POLLHUP as i16;
#[cfg(windows)]
const HOST_POLLHUP: i16 = winsock::POLLHUP;
#[cfg(unix)]
const HOST_POLLNVAL: i16 = libc::POLLNVAL as i16;
#[cfg(windows)]
const HOST_POLLNVAL: i16 = winsock::POLLNVAL;

fn debug_log<F: FnOnce() -> String>(message: F) {
    if env::var("MOLT_WASM_HOST_DEBUG").is_ok() {
        eprintln!("[molt-wasm-host] {}", message());
    }
}

fn precompiled_enabled() -> bool {
    matches!(env::var("MOLT_WASM_PRECOMPILED").as_deref(), Ok("1"))
}

fn precompiled_write_enabled() -> bool {
    matches!(env::var("MOLT_WASM_PRECOMPILED_WRITE").as_deref(), Ok("1"))
}

fn resolve_precompiled_path(wasm_path: &Path, override_env: &str) -> Option<PathBuf> {
    if !precompiled_enabled() {
        return None;
    }
    if let Ok(path) = env::var(override_env)
        && !path.is_empty()
    {
        return Some(PathBuf::from(path));
    }
    Some(wasm_path.with_extension("cwasm"))
}

fn load_or_compile_module(
    engine: &Engine,
    wasm_path: &Path,
    label: &str,
    override_env: &str,
) -> Result<Module> {
    if let Some(precompiled) = resolve_precompiled_path(wasm_path, override_env)
        && precompiled.exists()
    {
        debug_log(|| format!("loading {label} precompiled: {precompiled:?}"));
        match unsafe { Module::deserialize_file(engine, &precompiled) } {
            Ok(module) => return Ok(module),
            Err(err) => {
                debug_log(|| format!("precompiled load failed ({label}): {err}"));
            }
        }
    }
    let read_start = Instant::now();
    let wasm_bytes = fs::read(wasm_path).with_context(|| format!("read {label} {wasm_path:?}"))?;
    debug_log(|| format!("read {label} wasm in {:?}", read_start.elapsed()));
    let compile_start = Instant::now();
    let module = Module::new(engine, wasm_bytes)
        .map_err(|err| err.context(format!("compile {label} {wasm_path:?}")))?;
    debug_log(|| format!("compiled {label} module in {:?}", compile_start.elapsed()));
    if precompiled_write_enabled()
        && let Some(precompiled) = resolve_precompiled_path(wasm_path, override_env)
    {
        match module.serialize() {
            Ok(bytes) => {
                let _ = fs::write(&precompiled, bytes);
                debug_log(|| format!("wrote {label} precompiled: {precompiled:?}"));
            }
            Err(err) => {
                debug_log(|| format!("serialize {label} failed: {err}"));
            }
        }
    }
    Ok(module)
}

fn build_engine() -> Result<Engine> {
    let mut config = Config::new();
    let cache_toggle = env::var("MOLT_WASM_CACHE").ok();
    let max_stack = env::var("MOLT_WASM_MAX_STACK")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .filter(|val| *val > 0)
        .unwrap_or(8 * 1024 * 1024);
    // Wasmtime 43+ requires async_stack_size >= max_wasm_stack unconditionally.
    // Bump async_stack_size to accommodate, adding headroom for host-side frames.
    config.async_stack_size(max_stack + (128 * 1024));
    config.max_wasm_stack(max_stack);
    debug_log(|| format!("wasmtime max_wasm_stack set to {max_stack}"));
    if cache_toggle.as_deref() != Some("0") {
        let cache_path = env::var("MOLT_WASM_CACHE_CONFIG").ok();
        if cache_toggle.as_deref() == Some("1") || cache_path.is_some() {
            let cache = match cache_path.as_deref() {
                Some(path) => {
                    debug_log(|| format!("wasmtime cache config: {path}"));
                    Cache::from_file(Some(Path::new(path)))?
                }
                None => {
                    debug_log(|| "wasmtime cache config: default".to_string());
                    Cache::from_file(None)?
                }
            };
            config.cache(Some(cache));
            debug_log(|| "wasmtime cache enabled".to_string());
        }
    }
    if matches!(env::var("MOLT_WASM_COMPILE_SERIAL").as_deref(), Ok("1")) {
        config.parallel_compilation(false);
        debug_log(|| "wasmtime parallel compilation disabled".to_string());
    }
    if matches!(env::var("MOLT_WASM_COMPILE_FAST").as_deref(), Ok("1")) {
        config.cranelift_opt_level(OptLevel::None);
        debug_log(|| "wasmtime opt level set to none".to_string());
    }
    // Deterministic mode: canonicalize NaN payloads and disable parallel compilation
    // to ensure reproducible WASM execution across runs and hosts.
    if matches!(env::var("MOLT_DETERMINISTIC").as_deref(), Ok("1")) {
        config.cranelift_nan_canonicalization(true);
        config.parallel_compilation(false);
        debug_log(|| {
            "deterministic mode: NaN canonicalization and serial compilation enabled".to_string()
        });
    }
    config.wasm_function_references(true);
    config.wasm_gc(true);
    Ok(Engine::new(&config)?)
}

struct HostState {
    wasi: WasiP1Ctx,
    memory: Option<Memory>,
    call_indirect: Arc<Mutex<HashMap<String, Option<Func>>>>,
    isolate_bootstrap_export: Option<Func>,
    isolate_import_export: Option<Func>,
    db_worker: Option<DbWorker>,
    db_pending: HashMap<u64, PendingDbRequest>,
    db_cancel_index: Vec<u64>,
    db_cancel_positions: HashMap<u64, usize>,
    db_cancel_cursor: usize,
    last_cancel_check: Option<Instant>,
    socket_manager: SocketManager,
    ws_manager: WebSocketManager,
    process_manager: ProcessManager,
}

struct SocketManager {
    next_id: u64,
    sockets: HashMap<u64, Socket>,
}

impl SocketManager {
    fn new() -> Self {
        Self {
            next_id: 1,
            sockets: HashMap::new(),
        }
    }

    fn insert(&mut self, socket: Socket) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.sockets.insert(id, socket);
        id
    }

    fn remove(&mut self, id: u64) -> Option<Socket> {
        self.sockets.remove(&id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Socket> {
        self.sockets.get_mut(&id)
    }
}

impl WebSocketManager {
    fn new() -> Self {
        Self {
            next_id: 1,
            sockets: HashMap::new(),
        }
    }

    fn insert(&mut self, socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.sockets.insert(
            id,
            WebSocketEntry {
                socket,
                queue: VecDeque::new(),
                closed: false,
            },
        );
        id
    }

    fn remove(&mut self, id: u64) -> Option<WebSocketEntry> {
        self.sockets.remove(&id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut WebSocketEntry> {
        self.sockets.get_mut(&id)
    }
}

struct WebSocketManager {
    next_id: u64,
    sockets: HashMap<u64, WebSocketEntry>,
}

struct WebSocketEntry {
    socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
    queue: VecDeque<Vec<u8>>,
    closed: bool,
}

fn indexed_track(index: &mut Vec<u64>, positions: &mut HashMap<u64, usize>, id: u64) {
    if positions.contains_key(&id) {
        return;
    }
    let pos = index.len();
    index.push(id);
    positions.insert(id, pos);
}

fn indexed_untrack(
    index: &mut Vec<u64>,
    positions: &mut HashMap<u64, usize>,
    cursor: &mut usize,
    id: u64,
) {
    let Some(pos) = positions.remove(&id) else {
        return;
    };
    let last = index.len().saturating_sub(1);
    index.swap_remove(pos);
    if pos < last
        && let Some(moved) = index.get(pos).copied()
    {
        positions.insert(moved, pos);
    }
    if index.is_empty() || *cursor >= index.len() {
        *cursor = 0;
    }
}

fn indexed_next_batch(index: &[u64], cursor: &mut usize, max_batch: usize) -> Vec<u64> {
    if index.is_empty() || max_batch == 0 {
        return Vec::new();
    }
    let batch = max_batch.min(index.len());
    let mut out = Vec::with_capacity(batch);
    for _ in 0..batch {
        if *cursor >= index.len() {
            *cursor = 0;
        }
        out.push(index[*cursor]);
        *cursor += 1;
        if *cursor >= index.len() {
            *cursor = 0;
        }
    }
    out
}

fn db_cancel_track(state: &mut HostState, req_id: u64) {
    indexed_track(
        &mut state.db_cancel_index,
        &mut state.db_cancel_positions,
        req_id,
    );
}

fn db_cancel_untrack(state: &mut HostState, req_id: u64) {
    indexed_untrack(
        &mut state.db_cancel_index,
        &mut state.db_cancel_positions,
        &mut state.db_cancel_cursor,
        req_id,
    );
}

fn resolve_wasm_path(arg: Option<String>) -> Result<PathBuf> {
    let env_path = env::var("MOLT_WASM_PATH").ok().map(PathBuf::from);
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = wasm_path_candidates(arg.map(PathBuf::from), env_path, &cwd);

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!("WASM path not found (arg, MOLT_WASM_PATH, or ./dist/output.wasm)");
}

fn resolve_linked_path(wasm_path: &Path) -> Option<PathBuf> {
    let env_path = env::var("MOLT_WASM_LINKED_PATH").ok().map(PathBuf::from);
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    linked_path_candidates(wasm_path, env_path, &cwd)
        .into_iter()
        .find(|candidate| candidate.exists())
}

fn wasm_path_candidates(
    arg: Option<PathBuf>,
    env_path: Option<PathBuf>,
    cwd: &Path,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = arg {
        candidates.push(path);
    }
    if let Some(path) = env_path
        && !candidates.iter().any(|candidate| candidate == &path)
    {
        candidates.push(path);
    }
    let canonical = cwd.join("dist").join("output.wasm");
    if !candidates.iter().any(|candidate| candidate == &canonical) {
        candidates.push(canonical);
    }
    candidates
}

fn linked_path_candidates(wasm_path: &Path, env_path: Option<PathBuf>, cwd: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env_path {
        candidates.push(path);
    }
    if let Some(stem) = wasm_path.file_stem().and_then(|s| s.to_str()) {
        let ext = wasm_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("wasm");
        let sibling = wasm_path.with_file_name(format!("{stem}_linked.{ext}"));
        if !candidates.iter().any(|candidate| candidate == &sibling) {
            candidates.push(sibling);
        }
    }
    let canonical = cwd.join("dist").join("output_linked.wasm");
    if !candidates.iter().any(|candidate| candidate == &canonical) {
        candidates.push(canonical);
    }
    candidates
}

fn prefer_linked() -> bool {
    match env::var("MOLT_WASM_PREFER_LINKED") {
        Ok(val) => !matches!(val.to_lowercase().as_str(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

fn force_linked() -> bool {
    matches!(env::var("MOLT_WASM_LINKED").as_deref(), Ok("1"))
}

fn define_isolate_host_imports(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
    engine: &Engine,
) -> Result<()> {
    let bootstrap_ty = FuncType::new(engine, [], [ValType::I64]);
    let bootstrap = Func::new(
        &mut *store,
        bootstrap_ty,
        |mut caller: Caller<'_, HostState>, params, results| {
            debug_log(|| "env::molt_isolate_bootstrap -> app export".to_string());
            let func = caller
                .data()
                .isolate_bootstrap_export
                .as_ref()
                .cloned()
                .ok_or_else(|| {
                    wasmtime::Error::msg("molt_isolate_bootstrap export not registered")
                })?;
            let result = func.call(&mut caller, params, results);
            debug_log(|| format!("env::molt_isolate_bootstrap <- {result:?}"));
            result
        },
    );
    linker.define(&mut *store, "env", "molt_isolate_bootstrap", bootstrap)?;

    let import_ty = FuncType::new(engine, [ValType::I64], [ValType::I64]);
    let import = Func::new(
        &mut *store,
        import_ty,
        |mut caller: Caller<'_, HostState>, params, results| {
            debug_log(|| format!("env::molt_isolate_import -> app export params={params:?}"));
            let func = caller
                .data()
                .isolate_import_export
                .as_ref()
                .cloned()
                .ok_or_else(|| wasmtime::Error::msg("molt_isolate_import export not registered"))?;
            let result = func.call(&mut caller, params, results);
            debug_log(|| format!("env::molt_isolate_import <- {result:?} results={results:?}"));
            result
        },
    );
    linker.define(&mut *store, "env", "molt_isolate_import", import)?;
    Ok(())
}

fn register_isolate_exports(store: &mut Store<HostState>, instance: &Instance) -> Result<()> {
    let bootstrap = instance
        .get_func(&mut *store, "molt_isolate_bootstrap")
        .context("missing molt_isolate_bootstrap export")?;
    let import = instance
        .get_func(&mut *store, "molt_isolate_import")
        .context("missing molt_isolate_import export")?;
    let state = store.data_mut();
    state.isolate_bootstrap_export = Some(bootstrap);
    state.isolate_import_export = Some(import);
    Ok(())
}

fn call_zero_arg_export(
    store: &mut Store<HostState>,
    instance: &Instance,
    export_name: &'static str,
) -> Result<()> {
    let func = instance
        .get_func(&mut *store, export_name)
        .with_context(|| format!("missing {export_name} export"))?;
    debug_log(|| format!("calling {export_name}"));
    let mut results = alloc_results(&func.ty(&*store), export_name)?;
    func.call(&mut *store, &[], &mut results)
        .map_err(|err| anyhow::anyhow!("call {export_name}: {err}"))?;
    debug_log(|| format!("{export_name} returned"));
    Ok(())
}

fn call_app_startup_entries(store: &mut Store<HostState>, instance: &Instance) -> Result<()> {
    // Normal execution has exactly one startup authority: the exported
    // molt_main wrapper. It owns runtime init, manifest install, table init, and
    // app entry execution. Host-export setup is routed through molt_host_init
    // in the JS/browser hosts; pre-calling raw isolate bootstrap here creates a
    // second initialization lane before the wrapper has run.
    call_zero_arg_export(store, instance, "molt_main")
}

#[cfg(test)]
mod tests {
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
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_env = env::var("PATH").unwrap_or_default();
    for dir in env::split_paths(&path_env) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_exports_path() -> Option<PathBuf> {
    if let Ok(path) = env::var("MOLT_WASM_DB_EXPORTS").or_else(|_| env::var("MOLT_WORKER_EXPORTS"))
    {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    let packaged = PathBuf::from("src/molt_accel/default_exports.json");
    if packaged.exists() {
        return Some(packaged);
    }
    let demo = PathBuf::from("demo/molt_worker_app/molt_exports.json");
    if demo.exists() {
        return Some(demo);
    }
    None
}

fn resolve_worker_cmd() -> Result<Vec<String>> {
    if let Ok(cmd) = env::var("MOLT_WASM_DB_WORKER_CMD").or_else(|_| env::var("MOLT_WORKER_CMD")) {
        let parts = cmd
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if parts.is_empty() {
            bail!("MOLT_WASM_DB_WORKER_CMD is empty");
        }
        return Ok(parts);
    }
    let worker = find_in_path("molt-worker").or_else(|| find_in_path("molt_worker"));
    let Some(worker) = worker else {
        bail!("molt-worker not found; set MOLT_WASM_DB_WORKER_CMD or MOLT_WORKER_CMD");
    };
    let exports_path = resolve_exports_path()
        .context("molt-worker exports manifest not found (set MOLT_WASM_DB_EXPORTS)")?;
    let mut cmd = vec![
        worker.to_string_lossy().to_string(),
        "--stdio".into(),
        "--exports".into(),
    ];
    cmd.push(exports_path.to_string_lossy().to_string());
    if let Ok(compiled) = env::var("MOLT_WASM_DB_COMPILED_EXPORTS") {
        cmd.push("--compiled-exports".into());
        cmd.push(compiled);
    }
    Ok(cmd)
}

fn resolve_timeout_ms() -> u64 {
    if let Ok(raw) =
        env::var("MOLT_WASM_DB_TIMEOUT_MS").or_else(|_| env::var("MOLT_DB_QUERY_TIMEOUT_MS"))
        && let Ok(val) = raw.parse::<u64>()
    {
        return val;
    }
    250
}

fn write_frame(mut writer: impl Write, payload: &[u8]) -> Result<()> {
    let len = payload.len();
    if len > u32::MAX as usize {
        bail!("frame too large: {len}");
    }
    let header = (len as u32).to_le_bytes();
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    Ok(())
}

fn read_frame(mut reader: impl Read) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header) as usize;
    if size > MAX_DB_FRAME_SIZE {
        bail!("worker frame too large: {size}");
    }
    let mut payload = vec![0u8; size];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

#[derive(Deserialize)]
struct WorkerEnvelope {
    request_id: Option<u64>,
    status: Option<String>,
    codec: Option<String>,
    payload_b64: Option<String>,
    error: Option<String>,
    metrics: Option<JsonValue>,
}

struct WorkerResponse {
    request_id: u64,
    status: String,
    codec: String,
    payload: Vec<u8>,
    error: Option<String>,
    metrics: Option<JsonValue>,
}

struct PendingDbRequest {
    stream_bits: u64,
    token_id: u64,
    cancel_sent: bool,
}

enum WorkerMessage {
    Response(WorkerResponse),
    Error(anyhow::Error),
}

enum WorkerError {
    Unavailable(anyhow::Error),
    SendFailed(anyhow::Error),
}

fn decode_worker_frame(frame: &[u8]) -> Result<WorkerResponse> {
    let envelope: WorkerEnvelope = serde_json::from_slice(frame)?;
    let request_id = envelope.request_id.unwrap_or(0);
    let status = envelope
        .status
        .unwrap_or_else(|| "InternalError".to_string());
    let codec = envelope.codec.unwrap_or_else(|| "raw".to_string());
    let payload = match envelope.payload_b64 {
        Some(encoded) => STANDARD.decode(encoded)?,
        None => Vec::new(),
    };
    Ok(WorkerResponse {
        request_id,
        status,
        codec,
        payload,
        error: envelope.error,
        metrics: envelope.metrics,
    })
}

fn map_worker_status(status: &str) -> &'static str {
    match status {
        "Ok" => "ok",
        "InvalidInput" => "invalid_input",
        "Busy" => "busy",
        "Timeout" => "timeout",
        "Cancelled" => "cancelled",
        "InternalError" => "internal_error",
        _ => "internal_error",
    }
}

fn json_to_msgpack(value: &JsonValue) -> MsgpackValue {
    match value {
        JsonValue::Null => MsgpackValue::Nil,
        JsonValue::Bool(val) => MsgpackValue::from(*val),
        JsonValue::Number(num) => {
            if let Some(int) = num.as_i64() {
                MsgpackValue::from(int)
            } else if let Some(uint) = num.as_u64() {
                MsgpackValue::from(uint)
            } else if let Some(float) = num.as_f64() {
                MsgpackValue::from(float)
            } else {
                MsgpackValue::Nil
            }
        }
        JsonValue::String(val) => MsgpackValue::from(val.as_str()),
        JsonValue::Array(items) => MsgpackValue::Array(items.iter().map(json_to_msgpack).collect()),
        JsonValue::Object(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (key, val) in map {
                entries.push((MsgpackValue::from(key.as_str()), json_to_msgpack(val)));
            }
            MsgpackValue::Map(entries)
        }
    }
}

fn encode_msgpack_header(
    status: &str,
    codec: &str,
    payload: Option<&[u8]>,
    error: Option<&str>,
    metrics: Option<&JsonValue>,
) -> Result<Vec<u8>> {
    let mut map = Vec::new();
    map.push((MsgpackValue::from("status"), MsgpackValue::from(status)));
    map.push((MsgpackValue::from("codec"), MsgpackValue::from(codec)));
    if let Some(payload) = payload {
        map.push((
            MsgpackValue::from("payload"),
            MsgpackValue::Binary(payload.to_vec()),
        ));
    }
    if let Some(error) = error {
        map.push((MsgpackValue::from("error"), MsgpackValue::from(error)));
    }
    if let Some(metrics) = metrics {
        map.push((MsgpackValue::from("metrics"), json_to_msgpack(metrics)));
    }
    let mut out = Vec::new();
    write_value(&mut out, &MsgpackValue::Map(map))?;
    Ok(out)
}

struct DbWorker {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    responses: mpsc::Receiver<WorkerMessage>,
    next_id: u64,
}

impl DbWorker {
    fn new() -> Result<Self> {
        let cmd = resolve_worker_cmd()?;
        let mut command = Command::new(&cmd[0]);
        command.args(&cmd[1..]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command.envs(env::vars());
        let mut child = command.spawn().context("spawn molt-worker")?;
        let stdin = child.stdin.take().context("missing worker stdin")?;
        let stdout = child.stdout.take().context("missing worker stdout")?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let frame = match read_frame(&mut reader) {
                    Ok(frame) => frame,
                    Err(err) => {
                        let _ = tx.send(WorkerMessage::Error(err));
                        break;
                    }
                };
                let response = match decode_worker_frame(&frame) {
                    Ok(resp) => WorkerMessage::Response(resp),
                    Err(err) => WorkerMessage::Error(err),
                };
                if tx.send(response).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            responses: rx,
            next_id: 1,
        })
    }

    fn send_request(&mut self, entry: &str, payload: &[u8], timeout_ms: u64) -> Result<u64> {
        let request_id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let payload_b64 = STANDARD.encode(payload);
        let msg = serde_json::json!({
            "request_id": request_id,
            "entry": entry,
            "timeout_ms": timeout_ms,
            "codec": "msgpack",
            "payload_b64": payload_b64,
        });
        let bytes = serde_json::to_vec(&msg)?;
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| anyhow::anyhow!("stdin lock poisoned"))?;
        write_frame(&mut *stdin, &bytes)?;
        Ok(request_id)
    }
}

fn send_worker_cancel(stdin: &Arc<Mutex<ChildStdin>>, target_id: u64) -> Result<()> {
    let cancel_payload = serde_json::json!({ "request_id": target_id });
    let cancel_bytes = serde_json::to_vec(&cancel_payload)?;
    let payload_b64 = STANDARD.encode(cancel_bytes);
    let msg = serde_json::json!({
        "request_id": 0,
        "entry": "__cancel__",
        "timeout_ms": 0,
        "codec": "json",
        "payload_b64": payload_b64,
    });
    let bytes = serde_json::to_vec(&msg)?;
    let mut guard = stdin
        .lock()
        .map_err(|_| anyhow::anyhow!("stdin lock poisoned"))?;
    write_frame(&mut *guard, &bytes)?;
    Ok(())
}

fn ensure_locale_env(envs: &mut Vec<(String, String)>) {
    let has_locale = envs.iter().any(|(k, _)| {
        k == "MOLT_WASM_LOCALE_DECIMAL"
            || k == "MOLT_WASM_LOCALE_THOUSANDS"
            || k == "MOLT_WASM_LOCALE_GROUPING"
    });
    if has_locale {
        return;
    }
    let locale = match SystemLocale::default() {
        Ok(locale) => locale,
        Err(_) => return,
    };
    envs.push((
        "MOLT_WASM_LOCALE_DECIMAL".to_string(),
        locale.decimal().to_string(),
    ));
    let sep = locale.separator().to_string();
    if !sep.is_empty() {
        envs.push(("MOLT_WASM_LOCALE_THOUSANDS".to_string(), sep));
        let grouping = match locale.grouping() {
            Grouping::Posix => None,
            Grouping::Standard | Grouping::Indian => Some("3"),
        };
        if let Some(grouping) = grouping {
            envs.push((
                "MOLT_WASM_LOCALE_GROUPING".to_string(),
                grouping.to_string(),
            ));
        }
    }
}

fn build_wasi_ctx(extra_envs: &[(String, String)], guest_args: &[String]) -> Result<WasiP1Ctx> {
    let mut envs = env::vars().collect::<Vec<_>>();
    ensure_locale_env(&mut envs);
    envs.extend(extra_envs.iter().cloned());
    let mut builder = WasiCtxBuilder::new();
    builder.inherit_stdio();
    builder.envs(&envs);
    if guest_args.is_empty() {
        builder.inherit_args();
    } else {
        // Pass only the guest-facing args: ["app", route, query, ...]
        let mut wasi_args: Vec<String> = vec!["app".to_string()];
        wasi_args.extend(guest_args.iter().cloned());
        builder.args(&wasi_args);
    }
    builder.preopened_dir(".", ".", DirPerms::all(), FilePerms::all())?;
    Ok(builder.build_p1())
}

fn merge_limits(
    left: Option<Limits>,
    right: Option<Limits>,
    label: &str,
) -> Result<Option<Limits>> {
    match (left, right) {
        (None, None) => Ok(None),
        (Some(lim), None) | (None, Some(lim)) => Ok(Some(lim)),
        (Some(a), Some(b)) => {
            let min = a.min.max(b.min);
            let max = match (a.max, b.max) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            if let Some(max) = max
                && min > max
            {
                bail!("incompatible {label} limits: min {min} > max {max}");
            }
            Ok(Some(Limits { min, max }))
        }
    }
}

fn memory_limits(module: &Module) -> Option<MemoryType> {
    module.imports().find_map(|import| {
        if import.module() != "env" || import.name() != "memory" {
            return None;
        }
        match import.ty() {
            ExternType::Memory(mem) => Some(mem),
            _ => None,
        }
    })
}

fn table_limits(module: &Module) -> Option<TableType> {
    module.imports().find_map(|import| {
        if import.module() != "env" || import.name() != "__indirect_function_table" {
            return None;
        }
        match import.ty() {
            ExternType::Table(table) => Some(table),
            _ => None,
        }
    })
}

fn collect_call_indirect_imports(module: &Module) -> Vec<(String, FuncType)> {
    module
        .imports()
        .filter_map(|import| {
            let name = import.name();
            if import.module() != "env" || !name.starts_with("molt_call_indirect") {
                return None;
            }
            let ty = match import.ty() {
                ExternType::Func(func) => func,
                _ => return None,
            };
            Some((name.to_string(), ty))
        })
        .collect()
}

fn has_runtime_imports(module: &Module) -> bool {
    module
        .imports()
        .any(|import| import.module() == "molt_runtime")
}

fn make_call_indirect_func(
    store: &mut Store<HostState>,
    name: String,
    ty: FuncType,
    registry: Arc<Mutex<HashMap<String, Option<Func>>>>,
) -> Func {
    Func::new(store, ty, move |mut caller, params, results| {
        let func = registry
            .lock()
            .ok()
            .and_then(|map| map.get(&name).cloned())
            .flatten();
        let Some(func) = func else {
            return Err(wasmtime::Error::msg(format!(
                "{name} used before output instantiation"
            )));
        };
        func.call(&mut caller, params, results)
    })
}

fn box_int(value: u64) -> u64 {
    QNAN | TAG_INT | (value & INT_MASK)
}

fn is_bool_bits(bits: u64) -> bool {
    (bits & (QNAN | TAG_MASK)) == (QNAN | TAG_BOOL)
}

fn unbox_bool(bits: u64) -> bool {
    (bits & 1) == 1
}

struct RuntimeExports {
    stream_new: Func,
    stream_send: Func,
    stream_close: Func,
    alloc: Func,
    handle_resolve: Func,
    dec_ref_obj: Func,
    header_size: Option<Func>,
    cancel_is_cancelled: Option<Func>,
}

fn runtime_exports(caller: &mut Caller<HostState>) -> Result<RuntimeExports> {
    let stream_new = caller
        .get_export("molt_stream_new")
        .and_then(Extern::into_func)
        .context("missing molt_stream_new export")?;
    let stream_send = caller
        .get_export("molt_stream_send")
        .and_then(Extern::into_func)
        .context("missing molt_stream_send export")?;
    let stream_close = caller
        .get_export("molt_stream_close")
        .and_then(Extern::into_func)
        .context("missing molt_stream_close export")?;
    let alloc = caller
        .get_export("molt_alloc")
        .and_then(Extern::into_func)
        .context("missing molt_alloc export")?;
    let handle_resolve = caller
        .get_export("molt_handle_resolve")
        .and_then(Extern::into_func)
        .context("missing molt_handle_resolve export")?;
    let dec_ref_obj = caller
        .get_export("molt_dec_ref_obj")
        .and_then(Extern::into_func)
        .context("missing molt_dec_ref_obj export")?;
    let header_size = caller
        .get_export("molt_header_size")
        .and_then(Extern::into_func);
    let cancel_is_cancelled = caller
        .get_export("molt_cancel_token_is_cancelled")
        .and_then(Extern::into_func);
    Ok(RuntimeExports {
        stream_new,
        stream_send,
        stream_close,
        alloc,
        handle_resolve,
        dec_ref_obj,
        header_size,
        cancel_is_cancelled,
    })
}

fn call_i64(func: &Func, caller: &mut Caller<HostState>, args: &[Val]) -> Result<i64> {
    let mut results = [Val::I64(0)];
    func.call(caller, args, &mut results)?;
    match results[0] {
        Val::I64(val) => Ok(val),
        _ => bail!("unexpected wasm result type"),
    }
}

fn ensure_memory(caller: &mut Caller<HostState>) -> Result<Memory> {
    if let Some(mem) = caller.data().memory {
        return Ok(mem);
    }
    if let Some(mem) = caller
        .get_export("molt_memory")
        .and_then(Extern::into_memory)
    {
        caller.data_mut().memory = Some(mem);
        return Ok(mem);
    }
    if let Some(mem) = caller.get_export("memory").and_then(Extern::into_memory) {
        caller.data_mut().memory = Some(mem);
        return Ok(mem);
    }
    bail!("wasm memory not available");
}

fn alloc_temp_bytes(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    bytes: &[u8],
) -> Result<(u64, u64)> {
    let alloc_bits = call_i64(&exports.alloc, caller, &[Val::I64(bytes.len() as i64)])? as u64;
    if alloc_bits == 0 {
        bail!("molt_alloc failed");
    }
    let ptr_bits = call_i64(
        &exports.handle_resolve,
        caller,
        &[Val::I64(alloc_bits as i64)],
    )? as u64;
    if ptr_bits == 0 {
        bail!("molt_handle_resolve failed");
    }
    let header_size = if let Some(ref func) = exports.header_size {
        call_i64(func, caller, &[])? as u64
    } else {
        40
    };
    let payload_ptr = ptr_bits + header_size;
    memory.write(caller, payload_ptr as usize, bytes)?;
    Ok((alloc_bits, payload_ptr))
}

fn send_stream_frame(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    payload: &[u8],
) -> Result<()> {
    let (alloc_bits, payload_ptr) = alloc_temp_bytes(caller, exports, memory, payload)?;
    let _ = call_i64(
        &exports.stream_send,
        caller,
        &[
            Val::I64(stream_bits as i64),
            Val::I32(payload_ptr as i32),
            Val::I64(payload.len() as i64),
        ],
    )?;
    exports
        .dec_ref_obj
        .call(caller, &[Val::I64(alloc_bits as i64)], &mut [])?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn send_stream_header(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    status: &str,
    codec: &str,
    payload: Option<&[u8]>,
    error: Option<&str>,
    metrics: Option<&JsonValue>,
) -> Result<()> {
    let header = encode_msgpack_header(status, codec, payload, error, metrics)?;
    send_stream_frame(caller, exports, memory, stream_bits, &header)
}

fn send_stream_error(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    message: &str,
) -> Result<()> {
    send_stream_header(
        caller,
        exports,
        memory,
        stream_bits,
        "internal_error",
        "raw",
        None,
        Some(message),
        None,
    )?;
    exports
        .stream_close
        .call(caller, &[Val::I64(stream_bits as i64)], &mut [])?;
    Ok(())
}

fn db_host_unavailable(caller: &mut Caller<HostState>, memory: &Memory, out_ptr: usize) -> i32 {
    if out_ptr == 0 {
        return 2;
    }
    let bytes = 0u64.to_le_bytes();
    if memory.write(caller, out_ptr, &bytes).is_err() {
        return 2;
    }
    7
}

fn read_bytes(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>> {
    if ptr == 0 || len <= 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; len as usize];
    memory.read(caller, ptr as usize, &mut buf)?;
    Ok(buf)
}

fn write_bytes(
    caller: &mut Caller<HostState>,
    memory: &Memory,
    ptr: i32,
    bytes: &[u8],
) -> Result<()> {
    if ptr == 0 {
        bail!("null pointer");
    }
    memory.write(caller, ptr as usize, bytes)?;
    Ok(())
}

fn write_u32(caller: &mut Caller<HostState>, memory: &Memory, ptr: i32, val: u32) -> Result<()> {
    write_bytes(caller, memory, ptr, &val.to_le_bytes())
}

fn write_u64(caller: &mut Caller<HostState>, memory: &Memory, ptr: i32, val: u64) -> Result<()> {
    write_bytes(caller, memory, ptr, &val.to_le_bytes())
}

fn map_io_error(err: &std::io::Error) -> i32 {
    if let Some(code) = err.raw_os_error() {
        return code;
    }
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return libc::EWOULDBLOCK;
    }
    libc::EIO
}

fn ws_get_mut(state: &mut HostState, handle: i64) -> Result<&mut WebSocketEntry, i32> {
    if handle <= 0 {
        return Err(libc::EBADF);
    }
    state.ws_manager.get_mut(handle as u64).ok_or(libc::EBADF)
}

fn ws_set_nonblocking(
    ws: &mut tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
) -> std::io::Result<()> {
    match ws.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_nonblocking(true)?;
        }
        MaybeTlsStream::Rustls(stream) => {
            stream.get_ref().set_nonblocking(true)?;
        }
        _ => {}
    }
    Ok(())
}

fn map_ws_error(err: &tungstenite::Error) -> i32 {
    match err {
        tungstenite::Error::Io(io_err) => map_io_error(io_err),
        tungstenite::Error::Url(_) => libc::EINVAL,
        tungstenite::Error::Http(_) => libc::ECONNREFUSED,
        tungstenite::Error::Tls(_) => libc::EIO,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => libc::EPIPE,
        _ => libc::EIO,
    }
}

fn ws_drain_incoming(entry: &mut WebSocketEntry) -> Result<(), i32> {
    if entry.closed {
        return Ok(());
    }
    loop {
        match entry.socket.read() {
            Ok(Message::Binary(bytes)) => {
                entry.queue.push_back(bytes.to_vec());
            }
            Ok(Message::Text(text)) => {
                entry.queue.push_back(text.to_string().into_bytes());
            }
            Ok(Message::Ping(payload)) => {
                let _ = entry.socket.send(Message::Pong(payload));
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) => {
                entry.closed = true;
                break;
            }
            Err(tungstenite::Error::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {
                break;
            }
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                entry.closed = true;
                break;
            }
            Err(err) => {
                entry.closed = true;
                return Err(map_ws_error(&err));
            }
        }
        if entry.queue.len() >= 64 {
            break;
        }
    }
    Ok(())
}

fn poll_ws_stream(stream: &TcpStream, events: u32) -> Result<u32, i32> {
    let mut poll_events: i16 = 0;
    if (events & IO_EVENT_READ) != 0 {
        poll_events |= HOST_POLLIN;
    }
    if (events & IO_EVENT_WRITE) != 0 {
        poll_events |= HOST_POLLOUT;
    }
    if poll_events == 0 {
        poll_events |= HOST_POLLIN;
    }
    #[cfg(unix)]
    {
        let fd = stream.as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & HOST_POLLERR) != 0
            || (revents & HOST_POLLHUP) != 0
            || (revents & HOST_POLLNVAL) != 0
        {
            ready |= IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return Ok(ready);
        }
        if (revents & HOST_POLLIN) != 0 {
            ready |= IO_EVENT_READ;
        }
        if (revents & HOST_POLLOUT) != 0 {
            ready |= IO_EVENT_WRITE;
        }
        Ok(ready)
    }
    #[cfg(windows)]
    {
        let fd = stream.as_raw_socket() as usize;
        let mut pfd = winsock::WSAPOLLFD {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { winsock::WSAPoll(&mut pfd, 1, 0) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & HOST_POLLERR) != 0
            || (revents & HOST_POLLHUP) != 0
            || (revents & HOST_POLLNVAL) != 0
        {
            ready |= IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return Ok(ready);
        }
        if (revents & HOST_POLLIN) != 0 {
            ready |= IO_EVENT_READ;
        }
        if (revents & HOST_POLLOUT) != 0 {
            ready |= IO_EVENT_WRITE;
        }
        Ok(ready)
    }
}

fn deliver_worker_response(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    response: WorkerResponse,
) {
    let status = map_worker_status(&response.status);
    if status != "ok" {
        let message = response
            .error
            .clone()
            .unwrap_or_else(|| response.status.clone());
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            None,
            Some(&message),
            response.metrics.as_ref(),
        );
        let _ = exports
            .stream_close
            .call(caller, &[Val::I64(stream_bits as i64)], &mut []);
        return;
    }

    if response.codec == "arrow_ipc" {
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            None,
            None,
            response.metrics.as_ref(),
        );
        if !response.payload.is_empty() {
            let _ = send_stream_frame(caller, exports, memory, stream_bits, &response.payload);
        }
    } else {
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            Some(&response.payload),
            None,
            response.metrics.as_ref(),
        );
    }
    let _ = exports
        .stream_close
        .call(caller, &[Val::I64(stream_bits as i64)], &mut []);
}

fn fail_pending_requests(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    pending: Vec<PendingDbRequest>,
    message: &str,
) {
    for entry in pending {
        let _ = send_stream_error(caller, exports, memory, entry.stream_bits, message);
    }
}

fn drain_db_pending(state: &mut HostState) -> Vec<PendingDbRequest> {
    state.db_cancel_index.clear();
    state.db_cancel_positions.clear();
    state.db_cancel_cursor = 0;
    std::mem::take(&mut state.db_pending)
        .into_values()
        .collect::<Vec<_>>()
}

fn handle_db_host_poll(mut caller: Caller<'_, HostState>) -> i32 {
    let memory = match ensure_memory(&mut caller) {
        Ok(mem) => mem,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    let exports = match runtime_exports(&mut caller) {
        Ok(exports) => exports,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };

    let mut deliveries = Vec::new();
    let mut failures: Option<(Vec<PendingDbRequest>, String)> = None;
    let mut drop_worker = false;
    {
        let state = caller.data_mut();
        let worker_status = match state.db_worker.as_mut() {
            Some(worker) => worker.child.try_wait(),
            None => return 0,
        };
        match worker_status {
            Ok(Some(_)) | Err(_) => {
                let pending = drain_db_pending(state);
                failures = Some((pending, "db host worker exited".to_string()));
                drop_worker = true;
            }
            Ok(None) => {}
        }
        if failures.is_none() {
            loop {
                let message = match state.db_worker.as_mut() {
                    Some(worker) => worker.responses.try_recv(),
                    None => Err(mpsc::TryRecvError::Disconnected),
                };
                match message {
                    Ok(WorkerMessage::Response(resp)) => {
                        if let Some(pending) = state.db_pending.remove(&resp.request_id) {
                            db_cancel_untrack(state, resp.request_id);
                            deliveries.push((pending, resp));
                        }
                    }
                    Ok(WorkerMessage::Error(err)) => {
                        let pending = drain_db_pending(state);
                        failures = Some((pending, format!("db host error: {err}")));
                        drop_worker = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let pending = drain_db_pending(state);
                        failures = Some((pending, "db host disconnected".to_string()));
                        drop_worker = true;
                        break;
                    }
                }
            }
        }
    }
    if drop_worker {
        caller.data_mut().db_worker = None;
    }

    if let Some((pending, message)) = failures {
        fail_pending_requests(&mut caller, &exports, &memory, pending, &message);
        return 0;
    }

    for (pending, response) in deliveries {
        deliver_worker_response(
            &mut caller,
            &exports,
            &memory,
            pending.stream_bits,
            response,
        );
    }

    let now = Instant::now();
    let should_check = {
        let state = caller.data();
        state
            .last_cancel_check
            .map(|last| now.duration_since(last) >= Duration::from_millis(CANCEL_POLL_MS))
            .unwrap_or(true)
    };
    if should_check {
        let cancel_func = exports.cancel_is_cancelled;
        if let Some(cancel_func) = cancel_func {
            let candidate_ids = {
                let state = caller.data_mut();
                let budget = state.db_cancel_index.len().min(CANCEL_POLL_BATCH);
                indexed_next_batch(&state.db_cancel_index, &mut state.db_cancel_cursor, budget)
            };
            let candidates = {
                let state = caller.data_mut();
                let mut stale_ids = Vec::new();
                let mut batch = Vec::with_capacity(candidate_ids.len());
                for req_id in candidate_ids {
                    if let Some(pending) = state.db_pending.get(&req_id)
                        && pending.token_id != 0
                        && !pending.cancel_sent
                    {
                        batch.push((req_id, pending.token_id));
                    } else {
                        stale_ids.push(req_id);
                    }
                }
                for req_id in stale_ids {
                    db_cancel_untrack(state, req_id);
                }
                batch
            };
            let mut cancel_ids = Vec::new();
            for (req_id, token_id) in candidates {
                let boxed = box_int(token_id);
                if let Ok(bits) = call_i64(&cancel_func, &mut caller, &[Val::I64(boxed as i64)]) {
                    let bits = bits as u64;
                    if is_bool_bits(bits) && unbox_bool(bits) {
                        cancel_ids.push(req_id);
                    }
                }
            }
            if !cancel_ids.is_empty() {
                let state = caller.data_mut();
                let worker_stdin = state.db_worker.as_ref().map(|worker| worker.stdin.clone());
                if let Some(worker_stdin) = worker_stdin {
                    for req_id in cancel_ids {
                        let mut stop_polling_token = false;
                        if let Some(pending) = state.db_pending.get_mut(&req_id)
                            && pending.token_id != 0
                            && !pending.cancel_sent
                            && send_worker_cancel(&worker_stdin, req_id).is_ok()
                        {
                            pending.cancel_sent = true;
                            stop_polling_token = true;
                        }
                        if stop_polling_token || !state.db_pending.contains_key(&req_id) {
                            db_cancel_untrack(state, req_id);
                        }
                    }
                }
            }
        }
        caller.data_mut().last_cancel_check = Some(now);
    }

    0
}

fn ptr_from_i64(ptr: i64) -> Result<usize, i32> {
    let ptr_u64 = u64::try_from(ptr).map_err(|_| 1)?;
    usize::try_from(ptr_u64).map_err(|_| 1)
}

#[cfg(unix)]
fn local_tm_for_secs(secs: i64) -> Option<libc::tm> {
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    let ts = secs as libc::time_t;
    let ptr = unsafe { libc::localtime_r(&ts, tm.as_mut_ptr()) };
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { tm.assume_init() })
}

#[cfg(unix)]
fn local_noon_epoch(year: i32, month_zero_based: i32, day: i32) -> Option<i64> {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month_zero_based;
    tm.tm_mday = day;
    tm.tm_hour = 12;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    let ts = unsafe { libc::mktime(&mut tm as *mut libc::tm) };
    if ts < 0 {
        return None;
    }
    Some(ts as i64)
}

#[cfg(unix)]
fn local_offset_west_seconds_for(secs: i64) -> Option<i64> {
    let tm = local_tm_for_secs(secs)?;
    Some(-(tm.tm_gmtoff as i64))
}

#[cfg(unix)]
fn tzname_for_secs(secs: i64) -> Option<String> {
    let tm = local_tm_for_secs(secs)?;
    let mut buf = [0 as libc::c_char; 96];
    let fmt = b"%Z\0";
    let written = unsafe {
        libc::strftime(
            buf.as_mut_ptr(),
            buf.len(),
            fmt.as_ptr() as *const libc::c_char,
            &tm as *const libc::tm,
        )
    };
    if written == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, written as usize) };
    Some(String::from_utf8_lossy(bytes).to_string())
}

#[cfg(unix)]
fn timezone_profile_now() -> Option<(i64, String, String)> {
    let now_secs = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs(),
    )
    .ok()?;
    let year = local_tm_for_secs(now_secs)?.tm_year + 1900;
    let jan_secs = local_noon_epoch(year, 0, 1).unwrap_or(now_secs);
    let jul_secs = local_noon_epoch(year, 6, 1).unwrap_or(now_secs);
    let jan_off = local_offset_west_seconds_for(jan_secs).unwrap_or(0);
    let jul_off = local_offset_west_seconds_for(jul_secs).unwrap_or(0);
    let jan_name = tzname_for_secs(jan_secs).unwrap_or_else(|| "UTC".to_string());
    let jul_name = tzname_for_secs(jul_secs).unwrap_or_else(|| jan_name.clone());
    if jan_off >= jul_off {
        let dst = if jan_off == jul_off {
            jan_name.clone()
        } else {
            jul_name
        };
        Some((jan_off, jan_name, dst))
    } else {
        let dst = if jan_off == jul_off {
            jul_name.clone()
        } else {
            jan_name
        };
        Some((jul_off, jul_name, dst))
    }
}

fn host_time_timezone() -> i64 {
    #[cfg(unix)]
    {
        timezone_profile_now()
            .map(|profile| profile.0)
            .unwrap_or(i64::MIN)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn host_time_local_offset(secs: i64) -> i64 {
    #[cfg(unix)]
    {
        local_offset_west_seconds_for(secs).unwrap_or(i64::MIN)
    }
    #[cfg(not(unix))]
    {
        let _ = secs;
        0
    }
}

fn host_time_tzname(which: i32) -> Option<String> {
    if which != 0 && which != 1 {
        return None;
    }
    #[cfg(unix)]
    {
        let profile = timezone_profile_now()?;
        if which == 0 {
            return Some(profile.1);
        }
        Some(profile.2)
    }
    #[cfg(not(unix))]
    {
        Some("UTC".to_string())
    }
}

fn handle_db_host(
    mut caller: Caller<'_, HostState>,
    entry: &str,
    req_ptr: usize,
    len_bits: i64,
    out_ptr: usize,
    token_bits: i64,
) -> i32 {
    let len_bits_u64 = match u64::try_from(len_bits) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    let len = match usize::try_from(len_bits_u64) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    if out_ptr == 0 {
        return 2;
    }
    if req_ptr == 0 && len != 0 {
        return 1;
    }
    let memory = match ensure_memory(&mut caller) {
        Ok(mem) => mem,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    let mut payload = vec![0u8; len];
    if len > 0 && memory.read(&mut caller, req_ptr, &mut payload).is_err() {
        return 1;
    }

    let exports = match runtime_exports(&mut caller) {
        Ok(exports) => exports,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };

    let stream_bits = match call_i64(&exports.stream_new, &mut caller, &[Val::I64(0)]) {
        Ok(bits) => bits as u64,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    if memory
        .write(&mut caller, out_ptr, &stream_bits.to_le_bytes())
        .is_err()
    {
        return 2;
    }

    let timeout_ms = resolve_timeout_ms();
    let token_id = u64::try_from(token_bits).unwrap_or(0);
    let request_id = 'worker: {
        let state = caller.data_mut();
        let mut need_spawn = state.db_worker.is_none();
        if let Some(worker) = state.db_worker.as_mut() {
            match worker.child.try_wait() {
                Ok(Some(_)) => need_spawn = true,
                Ok(None) => {}
                Err(_) => need_spawn = true,
            }
        }
        if need_spawn {
            match DbWorker::new() {
                Ok(worker) => state.db_worker = Some(worker),
                Err(err) => break 'worker Err(WorkerError::Unavailable(err)),
            }
        }
        let worker = state
            .db_worker
            .as_mut()
            .expect("db_worker should be initialized");
        match worker.send_request(entry, &payload, timeout_ms) {
            Ok(id) => {
                state.db_pending.insert(
                    id,
                    PendingDbRequest {
                        stream_bits,
                        token_id,
                        cancel_sent: false,
                    },
                );
                if token_id != 0 {
                    db_cancel_track(state, id);
                }
                Ok(id)
            }
            Err(err) => Err(WorkerError::SendFailed(err)),
        }
    };
    match request_id {
        Ok(_) => 0,
        Err(WorkerError::Unavailable(err)) => {
            eprintln!("{err}");
            db_host_unavailable(&mut caller, &memory, out_ptr)
        }
        Err(WorkerError::SendFailed(err)) => {
            let _ = send_stream_error(
                &mut caller,
                &exports,
                &memory,
                stream_bits,
                &format!("db host send failed: {err}"),
            );
            0
        }
    }
}

fn define_db_host(linker: &mut Linker<HostState>, store: &mut Store<HostState>) -> Result<()> {
    let query = Func::wrap(
        &mut *store,
        |caller: Caller<'_, HostState>, req_ptr: i64, len: i64, out_ptr: i64, token: i64| {
            let req_ptr = match ptr_from_i64(req_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            let out_ptr = match ptr_from_i64(out_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            handle_db_host(caller, "db_query", req_ptr, len, out_ptr, token)
        },
    );
    let exec = Func::wrap(
        &mut *store,
        |caller: Caller<'_, HostState>, req_ptr: i64, len: i64, out_ptr: i64, token: i64| {
            let req_ptr = match ptr_from_i64(req_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            let out_ptr = match ptr_from_i64(out_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            handle_db_host(caller, "db_exec", req_ptr, len, out_ptr, token)
        },
    );
    let poll = Func::wrap(&mut *store, |caller: Caller<'_, HostState>| {
        handle_db_host_poll(caller)
    });
    linker.define(&mut *store, "env", "molt_db_query_host", query)?;
    linker.define(&mut *store, "env", "molt_db_exec_host", exec)?;
    linker.define(&mut *store, "env", "molt_db_host_poll", poll)?;
    Ok(())
}

fn define_ws_host(linker: &mut Linker<HostState>, store: &mut Store<HostState>) -> Result<()> {
    let ws_connect = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, url_ptr: i32, url_len: i64, out_handle: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if out_handle == 0 {
                return -libc::EFAULT;
            }
            if url_len < 0 || url_len > i64::from(i32::MAX) {
                return -libc::EINVAL;
            }
            let url_len = url_len as i32;
            let url_bytes = match read_bytes(&mut caller, &memory, url_ptr, url_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let url_str = match String::from_utf8(url_bytes) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            let url = match Url::parse(&url_str) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            if url.scheme() != "ws" && url.scheme() != "wss" {
                return -libc::EINVAL;
            }
            let (mut socket, _) = match connect(url.as_str()) {
                Ok(val) => val,
                Err(err) => return -map_ws_error(&err),
            };
            if let Err(err) = ws_set_nonblocking(&mut socket) {
                return -map_io_error(&err);
            }
            let handle = {
                let state = caller.data_mut();
                state.ws_manager.insert(socket)
            };
            if write_u64(&mut caller, &memory, out_handle, handle).is_err() {
                caller.data_mut().ws_manager.remove(handle);
                return -libc::EFAULT;
            }
            0
        },
    );
    let ws_send = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, data_ptr: i32, len: i64| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if len < 0 || len > i64::from(i32::MAX) {
                return -libc::EINVAL;
            }
            let len = len as i32;
            let payload = match read_bytes(&mut caller, &memory, data_ptr, len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let entry = match ws_get_mut(caller.data_mut(), handle) {
                Ok(entry) => entry,
                Err(errno) => return -errno,
            };
            if entry.closed {
                return -libc::EPIPE;
            }
            match entry.socket.send(Message::Binary(payload.into())) {
                Ok(_) => 0,
                Err(tungstenite::Error::Io(err))
                    if err.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    -libc::EWOULDBLOCK
                }
                Err(err) => {
                    entry.closed = true;
                    -map_ws_error(&err)
                }
            }
        },
    );
    let ws_recv = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_cap: i32,
         out_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if out_len == 0 {
                return -libc::EFAULT;
            }
            let cap = if buf_cap < 0 {
                return -libc::EINVAL;
            } else {
                buf_cap as usize
            };

            let (pending_bytes, needed_len, closed) = {
                let mut pending_bytes: Option<Vec<u8>> = None;
                let mut needed_len: Option<usize> = None;
                let entry = match ws_get_mut(caller.data_mut(), handle) {
                    Ok(entry) => entry,
                    Err(errno) => return -errno,
                };
                if entry.queue.is_empty()
                    && !entry.closed
                    && let Err(errno) = ws_drain_incoming(entry)
                {
                    return -errno;
                }
                if let Some(front) = entry.queue.front() {
                    if front.len() > cap {
                        needed_len = Some(front.len());
                    } else {
                        pending_bytes = entry.queue.pop_front();
                    }
                }
                (pending_bytes, needed_len, entry.closed)
            };

            if let Some(len) = needed_len {
                let _ = write_u32(&mut caller, &memory, out_len, len as u32);
                return -libc::ENOMEM;
            }
            if let Some(bytes) = pending_bytes {
                if write_bytes(&mut caller, &memory, buf_ptr, &bytes).is_err() {
                    return -libc::EFAULT;
                }
                let _ = write_u32(&mut caller, &memory, out_len, bytes.len() as u32);
                return 0;
            }
            let _ = write_u32(&mut caller, &memory, out_len, 0);
            if closed { 0 } else { -libc::EWOULDBLOCK }
        },
    );
    let ws_poll = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, events: i32| -> i32 {
            let entry = match ws_get_mut(caller.data_mut(), handle) {
                Ok(entry) => entry,
                Err(errno) => return -errno,
            };
            if entry.closed {
                return (IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE) as i32;
            }
            let events = events as u32;
            let mut ready = 0u32;
            if (events & IO_EVENT_READ) != 0 {
                if entry.queue.is_empty()
                    && let Err(errno) = ws_drain_incoming(entry)
                {
                    return -errno;
                }
                if !entry.queue.is_empty() {
                    ready |= IO_EVENT_READ;
                }
            }
            if (events & IO_EVENT_WRITE) != 0 {
                let stream_ref = match entry.socket.get_ref() {
                    MaybeTlsStream::Plain(stream) => stream,
                    MaybeTlsStream::Rustls(stream) => stream.get_ref(),
                    _ => return -libc::EIO,
                };
                let poll_ready = match poll_ws_stream(stream_ref, IO_EVENT_WRITE) {
                    Ok(mask) => mask,
                    Err(errno) => return -errno,
                };
                if (poll_ready & IO_EVENT_ERROR) != 0 {
                    return (IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE) as i32;
                }
                if (poll_ready & IO_EVENT_WRITE) != 0 {
                    ready |= IO_EVENT_WRITE;
                }
            }
            ready as i32
        },
    );
    let ws_close = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller.data_mut().ws_manager.remove(handle as u64) {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            if entry.closed {
                return 0;
            }
            let mut socket = entry.socket;
            let _ = socket.close(None);
            0
        },
    );
    linker.define(&mut *store, "env", "molt_ws_connect_host", ws_connect)?;
    linker.define(&mut *store, "env", "molt_ws_poll_host", ws_poll)?;
    linker.define(&mut *store, "env", "molt_ws_send_host", ws_send)?;
    linker.define(&mut *store, "env", "molt_ws_recv_host", ws_recv)?;
    linker.define(&mut *store, "env", "molt_ws_close_host", ws_close)?;
    Ok(())
}

fn define_time_host(linker: &mut Linker<HostState>, store: &mut Store<HostState>) -> Result<()> {
    let timezone = Func::wrap(&mut *store, || -> i64 { host_time_timezone() });
    let local_offset = Func::wrap(&mut *store, |secs: i64| -> i64 {
        host_time_local_offset(secs)
    });
    let tzname = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         which: i32,
         buf_ptr: i32,
         buf_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            if out_len_ptr == 0 {
                return -libc::EINVAL;
            }
            if buf_cap < 0 {
                return -libc::EINVAL;
            }
            let Some(label) = host_time_tzname(which) else {
                return -libc::EINVAL;
            };
            let bytes = label.as_bytes();
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::ENOSYS,
            };
            if write_u32(&mut caller, &memory, out_len_ptr, bytes.len() as u32).is_err() {
                return -libc::EINVAL;
            }
            let cap = buf_cap as usize;
            if bytes.len() > cap {
                return -libc::ENOMEM;
            }
            if !bytes.is_empty() && write_bytes(&mut caller, &memory, buf_ptr, bytes).is_err() {
                return -libc::EINVAL;
            }
            0
        },
    );
    linker.define(&mut *store, "env", "molt_time_timezone_host", timezone)?;
    linker.define(
        &mut *store,
        "env",
        "molt_time_local_offset_host",
        local_offset,
    )?;
    linker.define(&mut *store, "env", "molt_time_tzname_host", tzname)?;
    Ok(())
}

fn define_resource_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
    let on_allocate = Func::wrap(&mut *store, |size: i32| -> i32 {
        use molt_runtime::resource;
        match resource::with_tracker(|t| t.on_allocate(size as usize)) {
            Ok(()) => 0, // allocation permitted
            Err(_) => 1, // allocation denied
        }
    });
    let on_free = Func::wrap(&mut *store, |size: i32| {
        use molt_runtime::resource;
        resource::with_tracker(|t| t.on_free(size as usize));
    });
    linker.define(
        &mut *store,
        "env",
        "molt_resource_on_allocate_host",
        on_allocate,
    )?;
    linker.define(&mut *store, "env", "molt_resource_on_free_host", on_free)?;
    Ok(())
}

fn set_memory_from_exports(store: &mut Store<HostState>, instance: &wasmtime::Instance) {
    if store.data().memory.is_some() {
        return;
    }
    if let Some(mem) = instance.get_memory(&mut *store, "molt_memory") {
        store.data_mut().memory = Some(mem);
        return;
    }
    if let Some(mem) = instance.get_memory(&mut *store, "memory") {
        store.data_mut().memory = Some(mem);
    }
}

fn register_call_indirect_exports(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    registry: &Arc<Mutex<HashMap<String, Option<Func>>>>,
    names: &[String],
) -> Result<()> {
    let mut map = registry
        .lock()
        .map_err(|_| anyhow::anyhow!("call_indirect registry poisoned"))?;
    for name in names {
        let func = instance
            .get_func(&mut *store, name)
            .with_context(|| format!("missing export {name}"))?;
        map.insert(name.clone(), Some(func));
    }
    Ok(())
}

fn alloc_results(ty: &FuncType, export_name: &str) -> Result<Vec<Val>> {
    let mut results = Vec::new();
    for val_ty in ty.results() {
        let Some(val) = Val::default_for_ty(&val_ty) else {
            bail!("unsupported {export_name} return type: {val_ty:?}");
        };
        results.push(val);
    }
    Ok(results)
}

/// Compute the SHA-256 hash of a WASM module file for snapshot validation.
fn compute_module_hash(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let hash = Sha256::digest(&bytes);
    let hex = hash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("sha256:{hex}"))
}

/// Capture a snapshot of WASM linear memory after init completes.
fn capture_snapshot(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    header: &SnapshotHeader,
    output_path: &Path,
) -> Result<()> {
    // Get the memory export â€” try "molt_memory" first, then "memory"
    let memory = instance
        .get_memory(&mut *store, "molt_memory")
        .or_else(|| instance.get_memory(&mut *store, "memory"))
        .ok_or_else(|| anyhow::anyhow!("molt_memory export not found"))?;

    // Read the entire linear memory
    let data = memory.data(&store);
    let memory_bytes = data.to_vec();

    // Write header + blob
    let header_json = serde_json::to_string_pretty(&header.to_json())?;
    let mut file = std::fs::File::create(output_path)?;
    // Write header length (4 bytes LE) + header JSON + memory blob
    let header_bytes = header_json.as_bytes();
    file.write_all(&(header_bytes.len() as u32).to_le_bytes())?;
    file.write_all(header_bytes)?;
    file.write_all(&memory_bytes)?;
    debug_log(|| {
        format!(
            "snapshot captured: header={}B memory={}B -> {:?}",
            header_bytes.len(),
            memory_bytes.len(),
            output_path
        )
    });
    Ok(())
}

/// Restore a snapshot of WASM linear memory, skipping init if successful.
fn restore_snapshot(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    snapshot_path: &Path,
    expected_module_hash: &str,
) -> Result<bool> {
    if !snapshot_path.exists() {
        return Ok(false);
    }
    let data = std::fs::read(snapshot_path)?;
    if data.len() < 4 {
        return Ok(false);
    }
    // Read header length
    let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if data.len() < 4 + header_len {
        bail!(
            "snapshot file truncated: expected at least {} bytes, got {}",
            4 + header_len,
            data.len()
        );
    }
    let header_json = std::str::from_utf8(&data[4..4 + header_len])?;
    let header_value: serde_json::Value = serde_json::from_str(header_json)?;
    let header = SnapshotHeader::from_json(&header_value)
        .map_err(|e| anyhow::anyhow!("snapshot header parse error: {e}"))?;

    // Validate
    header
        .validate_against(expected_module_hash, "0.1.0")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    header
        .verify_integrity()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Restore memory
    let memory = instance
        .get_memory(&mut *store, "molt_memory")
        .or_else(|| instance.get_memory(&mut *store, "memory"))
        .ok_or_else(|| anyhow::anyhow!("molt_memory export not found"))?;
    let memory_blob = &data[4 + header_len..];
    let mem_data = memory.data_mut(&mut *store);
    if memory_blob.len() > mem_data.len() {
        bail!(
            "snapshot memory blob ({} bytes) exceeds linear memory ({} bytes)",
            memory_blob.len(),
            mem_data.len()
        );
    }
    mem_data[..memory_blob.len()].copy_from_slice(memory_blob);

    debug_log(|| {
        format!(
            "snapshot restored: header={}B memory={}B from {:?}",
            header_len,
            memory_blob.len(),
            snapshot_path
        )
    });
    Ok(true) // skip molt_main
}

fn main() -> Result<()> {
    debug_log(|| "starting".to_string());
    let mut args = env::args().skip(1);
    let mut bundle_path: Option<String> = None;
    let mut vfs_tmp_quota: Option<u64> = None;
    let mut snapshot_capture_path: Option<PathBuf> = None;
    let mut snapshot_restore_path: Option<PathBuf> = None;
    let mut positional: Option<String> = None;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "-h" | "--help" => {
                eprintln!(
                    "usage: molt-wasm-host [--bundle <path>] [--vfs-tmp-quota <MB>] \
                     [--snapshot-capture <path>] [--snapshot-restore <path>] [output.wasm]"
                );
                return Ok(());
            }
            "--bundle" => {
                bundle_path = Some(args.next().context("--bundle requires a path argument")?);
            }
            "--vfs-tmp-quota" => {
                let val = args
                    .next()
                    .context("--vfs-tmp-quota requires a value in MB")?;
                vfs_tmp_quota = Some(
                    val.parse::<u64>()
                        .context("--vfs-tmp-quota must be a positive integer (MB)")?,
                );
            }
            "--snapshot-capture" => {
                snapshot_capture_path = Some(PathBuf::from(
                    args.next()
                        .context("--snapshot-capture requires a path argument")?,
                ));
            }
            "--snapshot-restore" => {
                snapshot_restore_path = Some(PathBuf::from(
                    args.next()
                        .context("--snapshot-restore requires a path argument")?,
                ));
            }
            _ => {
                positional = Some(flag);
                break;
            }
        }
    }
    let arg = positional;
    // Collect remaining positional args as guest argv (route, query, etc.)
    let guest_args: Vec<String> = args.collect();

    // Build extra env vars for VFS configuration.
    let mut vfs_envs: Vec<(String, String)> = Vec::new();
    if let Some(ref bp) = bundle_path {
        // Resolve to absolute so the WASM guest can find it via preopened dirs.
        let abs =
            std::fs::canonicalize(bp).with_context(|| format!("--bundle path not found: {bp}"))?;
        vfs_envs.push((
            "MOLT_VFS_BUNDLE".to_string(),
            abs.to_string_lossy().to_string(),
        ));
    }
    vfs_envs.push((
        "MOLT_VFS_TMP_QUOTA_MB".to_string(),
        vfs_tmp_quota.unwrap_or(64).to_string(),
    ));

    let wasm_path = resolve_wasm_path(arg)?;
    let linked_path = resolve_linked_path(&wasm_path);
    let mut use_linked = force_linked() || (prefer_linked() && linked_path.is_some());
    let mut main_path = if use_linked {
        linked_path.clone().unwrap_or_else(|| wasm_path.clone())
    } else {
        wasm_path.clone()
    };

    let engine = build_engine()?;
    let mut output_module =
        load_or_compile_module(&engine, &main_path, "main", "MOLT_WASM_PRECOMPILED_PATH")?;
    let mut needs_runtime = has_runtime_imports(&output_module);
    if needs_runtime {
        if use_linked {
            bail!("linked wasm still imports molt_runtime; link step incomplete");
        }
        let Some(linked_path) = linked_path.clone() else {
            bail!(
                "linked wasm required for Molt runtime outputs; build with --linked or set MOLT_WASM_LINK=1."
            );
        };
        output_module =
            load_or_compile_module(&engine, &linked_path, "main", "MOLT_WASM_PRECOMPILED_PATH")?;
        needs_runtime = has_runtime_imports(&output_module);
        if needs_runtime {
            bail!("linked wasm still imports molt_runtime; link step incomplete");
        }
        main_path = linked_path;
        use_linked = true;
    }
    debug_log(|| format!("main wasm: {main_path:?} (linked={use_linked})"));

    let runtime_module = if needs_runtime {
        let runtime_path = env::var("MOLT_RUNTIME_WASM")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("wasm/molt_runtime.wasm"));
        Some(load_or_compile_module(
            &engine,
            &runtime_path,
            "runtime",
            "MOLT_WASM_PRECOMPILED_RUNTIME_PATH",
        )?)
    } else {
        None
    };

    let output_mem = memory_limits(&output_module);
    let output_table = table_limits(&output_module);
    let runtime_mem = runtime_module.as_ref().and_then(memory_limits);
    let runtime_table = runtime_module.as_ref().and_then(table_limits);

    let memory_limits = merge_limits(
        output_mem.as_ref().map(|mem| Limits {
            min: mem.minimum() as u32,
            max: mem.maximum().map(|v| v as u32),
        }),
        runtime_mem.as_ref().map(|mem| Limits {
            min: mem.minimum() as u32,
            max: mem.maximum().map(|v| v as u32),
        }),
        "memory",
    )?;
    let table_limits = merge_limits(
        output_table.as_ref().map(|table| Limits {
            min: table.minimum() as u32,
            max: table.maximum().map(|v| v as u32),
        }),
        runtime_table.as_ref().map(|table| Limits {
            min: table.minimum() as u32,
            max: table.maximum().map(|v| v as u32),
        }),
        "table",
    )?;

    let mut store = Store::new(
        &engine,
        HostState {
            wasi: build_wasi_ctx(&vfs_envs, &guest_args)?,
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
        },
    );

    let mut linker = Linker::new(&engine);
    p1::add_to_linker_sync(&mut linker, |state: &mut HostState| &mut state.wasi)?;

    if let Some(limits) = memory_limits {
        let output_is_64 = output_mem.as_ref().map(|mem| mem.is_64()).unwrap_or(false);
        let runtime_is_64 = runtime_mem.as_ref().map(|mem| mem.is_64()).unwrap_or(false);
        if output_is_64 || runtime_is_64 {
            bail!("memory64 not supported in wasm host");
        }
        let memory = Memory::new(&mut store, MemoryType::new(limits.min, limits.max))?;
        linker.define(&mut store, "env", "memory", memory)?;
        store.data_mut().memory = Some(memory);
    }
    if let Some(limits) = table_limits {
        let element = match (
            output_table.as_ref().map(|table| table.element().clone()),
            runtime_table.as_ref().map(|table| table.element().clone()),
        ) {
            (Some(left), Some(_right)) => left,
            (Some(left), None) => left,
            (None, Some(right)) => right,
            (None, None) => wasmtime::RefType::FUNCREF,
        };
        let table = Table::new(
            &mut store,
            TableType::new(element, limits.min, limits.max),
            Ref::Func(None),
        )?;
        linker.define(&mut store, "env", "__indirect_function_table", table)?;
    }

    define_db_host(&mut linker, &mut store)?;
    define_socket_host(&mut linker, &mut store)?;
    define_ws_host(&mut linker, &mut store)?;
    define_process_host(&mut linker, &mut store)?;
    define_time_host(&mut linker, &mut store)?;
    define_resource_host(&mut linker, &mut store)?;
    define_isolate_host_imports(&mut linker, &mut store, &engine)?;
    let getpid = Func::wrap(&mut store, || -> i64 { std::process::id() as i64 });
    linker.define(&mut store, "env", "molt_getpid_host", getpid)?;

    // GPU dispatch stub -- returns -ENOSYS when no WebGPU host is available.
    let gpu_dispatch = Func::wrap(
        &mut store,
        |_source_ptr: u32,
         _source_len: u32,
         _entry_ptr: u32,
         _entry_len: u32,
         _bindings_ptr: u32,
         _bindings_len: u32,
         _grid: u32,
         _workgroup_size: u32,
         _err_ptr: u32,
         _err_cap: u32,
         _out_err_len_ptr: u32|
         -> i32 { -38 },
    );
    linker.define(
        &mut store,
        "env",
        "molt_gpu_webgpu_dispatch_host",
        gpu_dispatch,
    )?;

    let registry = store.data().call_indirect.clone();
    let call_imports = if let Some(runtime_module) = runtime_module.as_ref() {
        collect_call_indirect_imports(runtime_module)
    } else {
        collect_call_indirect_imports(&output_module)
    };
    let call_names = call_imports
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    for (name, ty) in call_imports {
        let func = make_call_indirect_func(&mut store, name.clone(), ty, registry.clone());
        linker.define(&mut store, "env", &name, func)?;
    }

    // Compute module hash for snapshot validation.
    let module_hash = if snapshot_capture_path.is_some() || snapshot_restore_path.is_some() {
        Some(compute_module_hash(&main_path)?)
    } else {
        None
    };

    if let Some(runtime_module) = runtime_module {
        debug_log(|| "instantiating runtime".to_string());
        let runtime_instance = linker
            .instantiate(&mut store, &runtime_module)
            .map_err(|err| err.context("instantiate runtime"))?;
        debug_log(|| "runtime instantiated".to_string());
        for import in output_module.imports() {
            if import.module() != "molt_runtime" {
                continue;
            }
            let name = import.name();
            let export_name = format!("molt_{name}");
            let export = runtime_instance
                .get_export(&mut store, &export_name)
                .with_context(|| format!("missing runtime export {export_name}"))?;
            linker.define(&mut store, "molt_runtime", name, export)?;
        }
        debug_log(|| "instantiating output module".to_string());
        let output_instance = linker
            .instantiate(&mut store, &output_module)
            .map_err(|err| err.context("instantiate output"))?;
        debug_log(|| "output module instantiated".to_string());
        register_isolate_exports(&mut store, &output_instance)?;
        register_call_indirect_exports(&mut store, &output_instance, &registry, &call_names)?;
        set_memory_from_exports(&mut store, &output_instance);

        // Snapshot restore: if valid, skip molt_main.
        let restored = if let Some(ref restore_path) = snapshot_restore_path {
            restore_snapshot(
                &mut store,
                &output_instance,
                restore_path,
                module_hash.as_deref().unwrap(),
            )?
        } else {
            false
        };

        if !restored {
            call_app_startup_entries(&mut store, &output_instance)?;
        } else {
            debug_log(|| "molt_main skipped (restored from snapshot)".to_string());
        }

        // Snapshot capture: after molt_main returns (or after restore).
        if let Some(ref capture_path) = snapshot_capture_path {
            let memory = store
                .data()
                .memory
                .ok_or_else(|| anyhow::anyhow!("no linear memory available for snapshot"))?;
            let mem_size = memory.data_size(&store) as u64;
            let header = SnapshotHeader {
                snapshot_version: 1,
                abi_version: "0.1.0".into(),
                target_profile: "wasm_host".into(),
                module_hash: module_hash.as_deref().unwrap().to_string(),
                mount_plan: Vec::new(),
                capability_manifest: Vec::new(),
                determinism_stamp: String::new(),
                init_state_size: mem_size,
                integrity_hash: None,
            };
            capture_snapshot(&mut store, &output_instance, &header, capture_path)?;
        }
    } else {
        debug_log(|| "instantiating linked output".to_string());
        let output_instance = linker
            .instantiate(&mut store, &output_module)
            .map_err(|err| err.context("instantiate linked output"))?;
        debug_log(|| "linked output instantiated".to_string());
        register_isolate_exports(&mut store, &output_instance)?;
        register_call_indirect_exports(&mut store, &output_instance, &registry, &call_names)?;
        set_memory_from_exports(&mut store, &output_instance);

        // Snapshot restore: if valid, skip molt_main.
        let restored = if let Some(ref restore_path) = snapshot_restore_path {
            restore_snapshot(
                &mut store,
                &output_instance,
                restore_path,
                module_hash.as_deref().unwrap(),
            )?
        } else {
            false
        };

        if !restored {
            call_app_startup_entries(&mut store, &output_instance)?;
        } else {
            debug_log(|| "molt_main skipped (restored from snapshot)".to_string());
        }

        // Snapshot capture: after molt_main returns (or after restore).
        if let Some(ref capture_path) = snapshot_capture_path {
            let memory = store
                .data()
                .memory
                .ok_or_else(|| anyhow::anyhow!("no linear memory available for snapshot"))?;
            let mem_size = memory.data_size(&store) as u64;
            let header = SnapshotHeader {
                snapshot_version: 1,
                abi_version: "0.1.0".into(),
                target_profile: "wasm_host".into(),
                module_hash: module_hash.as_deref().unwrap().to_string(),
                mount_plan: Vec::new(),
                capability_manifest: Vec::new(),
                determinism_stamp: String::new(),
                init_state_size: mem_size,
                integrity_hash: None,
            };
            capture_snapshot(&mut store, &output_instance, &header, capture_path)?;
        }
    }

    Ok(())
}
