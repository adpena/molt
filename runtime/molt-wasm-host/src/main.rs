use base64::Engine as Base64Engine;
use base64::engine::general_purpose::STANDARD;
#[cfg(test)]
use molt_wasm_host::sha256_hex;
use num_format::{Grouping, SystemLocale};
use rmpv::Value as MsgpackValue;
use rmpv::encode::write_value;
use serde::Deserialize;
use serde_json::Value as JsonValue;
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
use wasmtime::error::{Context, bail};
use wasmtime::{
    Cache, Caller, Config, Engine, Extern, ExternType, Func, FuncType, Instance, Linker, Memory,
    MemoryType, Module, OptLevel, Ref, Result, Store, Table, TableType, Val, ValType,
};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder, p1};

mod db_host;
mod engine;
mod entrypoint;
mod indexed;
mod isolate_host;
#[cfg(test)]
mod main_tests;
mod path_resolver;
mod precompiled;
mod process_host;
mod runtime_bridge;
mod socket_host;
mod stream_bridge;
mod time_host;
mod wasi_env;
mod wasm_scan;
mod websocket_host;

use db_host::{DbWorker, PendingDbRequest, define_db_host};
use engine::*;
use entrypoint::*;
use indexed::*;
use isolate_host::*;
use path_resolver::*;
use precompiled::{ModuleRole, load_or_compile_module, precompile_execution};
use process_host::{ProcessManager, define_process_host};
use runtime_bridge::*;
use socket_host::define_socket_host;
use stream_bridge::*;
use time_host::define_time_host;
use wasi_env::*;
use wasm_scan::*;
use websocket_host::{WebSocketManager, define_ws_host};

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

struct HostState {
    wasi: WasiP1Ctx,
    memory: Option<Memory>,
    call_indirect: IndirectRegistry,
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

#[derive(Debug)]
struct HostOptions {
    bundle_path: Option<String>,
    vfs_tmp_quota: Option<u64>,
    execution: ExecutionRequest,
    guest_args: Vec<String>,
}

#[derive(Debug)]
enum ParsedHostArgs {
    Help,
    Run(HostOptions),
    Precompile(ExecutionRequest),
}

fn parse_host_args(args: impl IntoIterator<Item = String>) -> Result<ParsedHostArgs> {
    let mut args = args.into_iter();
    let mut bundle_path: Option<String> = None;
    let mut vfs_tmp_quota: Option<u64> = None;
    let mut execution: Option<ExecutionRequest> = None;
    let mut precompile = false;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "-h" | "--help" => return Ok(ParsedHostArgs::Help),
            "--precompile" => {
                if precompile {
                    bail!("--precompile may only be specified once");
                }
                precompile = true;
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
            "--snapshot-capture" | "--snapshot-restore" => {
                bail!(
                    "molt-wasm-host does not support executable snapshots: the current v2 artifact is metadata-only until full continuation, mutable-global, table, and host-resource state have one restore authority"
                );
            }
            "--wasi-command" => {
                let module = args
                    .next()
                    .context("--wasi-command requires a module path argument")?;
                execution = Some(ExecutionRequest::WasiCommand { module });
                break;
            }
            _ => {
                execution = Some(ExecutionRequest::MoltApplication {
                    manifest: Some(flag),
                });
                break;
            }
        }
    }

    let execution = execution.unwrap_or(ExecutionRequest::MoltApplication { manifest: None });
    // The execution selector ends host option parsing. Accept one conventional
    // separator, but otherwise preserve the guest tail byte-for-byte as argv.
    let mut guest_args = args.collect::<Vec<_>>();
    if guest_args.first().is_some_and(|arg| arg == "--") {
        guest_args.remove(0);
    }
    if precompile {
        if bundle_path.is_some() || vfs_tmp_quota.is_some() || !guest_args.is_empty() {
            bail!(
                "--precompile accepts an execution selector only, not guest arguments or VFS options"
            );
        }
        return Ok(ParsedHostArgs::Precompile(execution));
    }
    Ok(ParsedHostArgs::Run(HostOptions {
        bundle_path,
        vfs_tmp_quota,
        execution,
        guest_args,
    }))
}

fn validate_execution_imports(
    is_wasi_command: bool,
    use_linked: bool,
    needs_runtime: bool,
) -> Result<()> {
    if is_wasi_command && needs_runtime {
        bail!("WASI command module must not import molt_runtime");
    }
    if !is_wasi_command && use_linked && needs_runtime {
        bail!("linked wasm still imports molt_runtime; link step incomplete");
    }
    if !is_wasi_command && !use_linked && !needs_runtime {
        bail!("split-runtime app does not import molt_runtime");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadedGuestKind {
    MoltApplication { linked: bool },
    WasiCommand,
}

struct LoadedGuestOptions<'a> {
    kind: LoadedGuestKind,
    vfs_envs: &'a [(String, String)],
    guest_args: &'a [String],
    wasm_table_base: Option<u64>,
}

fn execute_loaded_guest(
    engine: &Engine,
    output_module: &Module,
    runtime_module: Option<&Module>,
    options: LoadedGuestOptions<'_>,
) -> Result<GuestTermination> {
    let LoadedGuestOptions {
        kind,
        vfs_envs,
        guest_args,
        wasm_table_base,
    } = options;
    let (is_wasi_command, use_linked) = match kind {
        LoadedGuestKind::MoltApplication { linked } => (false, linked),
        LoadedGuestKind::WasiCommand => (true, true),
    };
    let needs_runtime = has_runtime_imports(output_module);
    validate_execution_imports(is_wasi_command, use_linked, needs_runtime)?;

    match kind {
        LoadedGuestKind::WasiCommand => {
            if runtime_module.is_some() {
                bail!("WASI command mode must not provide a Molt runtime module");
            }
            validate_guest_module_entrypoint(GuestModuleEntrypoint::WasiCommand {
                command: output_module,
            })?;
        }
        LoadedGuestKind::MoltApplication { linked: true } => {
            if runtime_module.is_some() {
                bail!("linked wasm must not provide a separate runtime module");
            }
            validate_guest_module_entrypoint(GuestModuleEntrypoint::MoltApplication {
                application: output_module,
                runtime: output_module,
            })?;
        }
        LoadedGuestKind::MoltApplication { linked: false } => {
            let runtime =
                runtime_module.context("split-runtime app is missing its loaded runtime module")?;
            validate_guest_module_entrypoint(GuestModuleEntrypoint::MoltApplication {
                application: output_module,
                runtime,
            })?;
        }
    }
    let runtime_imports = RuntimeImportPlan::new(output_module, runtime_module)?;
    // Admit the complete Molt indirect-call family before either core start.
    // Other imports are checked against the real host linker below.
    let indirect_calls =
        plan_call_indirect_imports(output_module, runtime_module, is_wasi_command)?;
    let output_mem = memory_limits(output_module);
    let output_table = table_limits(output_module);
    let runtime_mem = runtime_module.and_then(memory_limits);
    let runtime_table = runtime_module.and_then(table_limits);

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
        engine,
        HostState {
            wasi: build_wasi_ctx(vfs_envs, guest_args)?,
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
        },
    );

    let mut linker = Linker::new(engine);
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
    define_isolate_host_imports(&mut linker, &mut store, engine)?;
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
    define_call_indirect_imports(&mut linker, &mut store, &indirect_calls, is_wasi_command)?;

    let runtime_instance = if let Some(runtime_module) = runtime_module {
        let runtime_pre = linker
            .instantiate_pre(runtime_module)
            .context("admit runtime host imports before guest start")?;
        admit_application_host_imports(&linker, &mut store, output_module)?;
        log::debug!("instantiating runtime");
        let runtime_instance = runtime_pre
            .instantiate(&mut store)
            .context("instantiate runtime")?;
        log::debug!("runtime instantiated");
        configure_wasm_table_base(&mut store, &runtime_instance, wasm_table_base)?;
        runtime_imports.bind(&mut linker, &mut store, &runtime_instance)?;
        Some(runtime_instance)
    } else {
        None
    };

    let output_label = if is_wasi_command {
        "WASI command"
    } else if runtime_instance.is_some() {
        "output"
    } else {
        "linked output"
    };
    log::debug!("instantiating {output_label}");
    let output_instance = if is_wasi_command {
        match classify_wasi_command_result(
            linker.instantiate(&mut store, output_module),
            "instantiate WASI command",
        )? {
            WasiCommandResult::Value(instance) => instance,
            WasiCommandResult::Exit(termination) => return Ok(termination),
        }
    } else {
        linker
            .instantiate(&mut store, output_module)
            .with_context(|| format!("instantiate {output_label}"))?
    };
    log::debug!("{output_label} instantiated");
    if !is_wasi_command {
        register_isolate_exports(&mut store, &output_instance)?;
    }
    if !is_wasi_command {
        register_call_indirect_exports(&mut store, &output_instance, &registry, &indirect_calls)?;
    }
    set_memory_from_exports(&mut store, &output_instance);
    if !is_wasi_command && runtime_instance.is_none() {
        configure_wasm_table_base(&mut store, &output_instance, wasm_table_base)?;
    }

    if is_wasi_command {
        return call_guest_entrypoint(
            &mut store,
            GuestEntrypoint::WasiCommand {
                command: &output_instance,
            },
        );
    }

    call_guest_entrypoint(
        &mut store,
        GuestEntrypoint::MoltApplication {
            application: &output_instance,
            runtime: runtime_instance.as_ref().unwrap_or(&output_instance),
        },
    )
}

fn main() -> Result<()> {
    let default_filter = if env::var_os("MOLT_WASM_HOST_DEBUG").is_some() {
        "off,molt_wasm_host=debug"
    } else {
        "off"
    };
    env_logger::Builder::from_env(
        env_logger::Env::new().filter_or("MOLT_WASM_HOST_LOG", default_filter),
    )
    .target(env_logger::Target::Stderr)
    .try_init()
    .context("initialize host diagnostics")?;
    let termination = run()?;
    if let GuestTermination::WasiExit(status) = termination
        && status != 0
    {
        std::process::exit(status);
    }
    Ok(())
}

fn run() -> Result<GuestTermination> {
    log::debug!("starting");
    let HostOptions {
        bundle_path,
        vfs_tmp_quota,
        execution,
        guest_args,
    } = match parse_host_args(env::args().skip(1))? {
        ParsedHostArgs::Help => {
            eprintln!(
                "usage: molt-wasm-host [--bundle <path>] [--vfs-tmp-quota <MB>] [manifest.json] \
                 | molt-wasm-host [--bundle <path>] [--vfs-tmp-quota <MB>] \
                 --wasi-command <module.wasm> [guest args...] \
                 | molt-wasm-host --precompile [manifest.json] \
                 | molt-wasm-host --precompile --wasi-command <module.wasm>"
            );
            return Ok(GuestTermination::Returned);
        }
        ParsedHostArgs::Run(options) => options,
        ParsedHostArgs::Precompile(request) => {
            let resolved = resolve_execution(request)?;
            let engine = build_engine()?;
            let receipt = precompile_execution(&engine, &resolved)?;
            let mut stdout = std::io::stdout().lock();
            serde_json::to_writer(&mut stdout, &receipt).context("encode precompile receipt")?;
            writeln!(stdout).context("write precompile receipt")?;
            return Ok(GuestTermination::Returned);
        }
    };
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

    let resolved = resolve_execution(execution)?;
    let (manifest_path, main_source, runtime_source, use_linked, is_wasi_command) = match resolved {
        ResolvedExecution::MoltApplication(ResolvedExecutionModules {
            manifest_path,
            main,
            runtime,
            linked,
        }) => (Some(manifest_path), main, runtime, linked, false),
        ResolvedExecution::WasiCommand { module } => (None, module, None, true, true),
    };
    let wasm_table_base = if is_wasi_command {
        None
    } else {
        detect_wasm_table_base(&main_source)?
    };
    if let Some(base) = wasm_table_base
        && env::var_os("MOLT_WASM_TABLE_BASE").is_none()
    {
        upsert_extra_env(&mut vfs_envs, "MOLT_WASM_TABLE_BASE", base.to_string());
    }

    let engine = build_engine()?;
    let output_module = load_or_compile_module(&engine, &main_source, ModuleRole::Main)?;
    let main_path = main_source.path();
    if is_wasi_command {
        log::debug!("WASI command wasm: {main_path:?}");
    } else {
        log::debug!(
            "runtime manifest: {manifest_path:?}; main wasm: {main_path:?} (linked={use_linked})"
        );
    }
    drop(main_source);

    let runtime_module = if let Some(runtime_source) = runtime_source.as_ref() {
        Some(load_or_compile_module(
            &engine,
            runtime_source,
            ModuleRole::Runtime,
        )?)
    } else {
        None
    };

    // Wasmtime now owns compiled modules; do not retain source buffers during
    // guest execution or keep a second disk-backed source authority alive.
    drop(runtime_source);

    execute_loaded_guest(
        &engine,
        &output_module,
        runtime_module.as_ref(),
        LoadedGuestOptions {
            kind: if is_wasi_command {
                LoadedGuestKind::WasiCommand
            } else {
                LoadedGuestKind::MoltApplication { linked: use_linked }
            },
            vfs_envs: &vfs_envs,
            guest_args: &guest_args,
            wasm_table_base,
        },
    )
}
