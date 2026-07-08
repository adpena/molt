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

mod db_host;
mod engine;
mod indexed;
mod isolate_host;
#[cfg(test)]
mod main_tests;
mod path_resolver;
mod process_host;
mod runtime_bridge;
mod snapshot;
mod socket_host;
mod time_host;
mod wasi_env;
mod wasm_scan;
mod websocket_host;

use db_host::{DbWorker, PendingDbRequest, define_db_host};
use engine::*;
use indexed::*;
use isolate_host::*;
use path_resolver::*;
use process_host::{ProcessManager, define_process_host};
use runtime_bridge::*;
use snapshot::*;
use socket_host::define_socket_host;
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
    let mut wasm_table_base = detect_wasm_table_base(&main_path)?;
    if let Some(base) = wasm_table_base
        && env::var_os("MOLT_WASM_TABLE_BASE").is_none()
    {
        upsert_extra_env(&mut vfs_envs, "MOLT_WASM_TABLE_BASE", base.to_string());
    }

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
        wasm_table_base = detect_wasm_table_base(&main_path)?;
        if let Some(base) = wasm_table_base
            && env::var_os("MOLT_WASM_TABLE_BASE").is_none()
        {
            upsert_extra_env(&mut vfs_envs, "MOLT_WASM_TABLE_BASE", base.to_string());
        }
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
        configure_wasm_table_base(&mut store, &runtime_instance, wasm_table_base)?;
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
        configure_wasm_table_base(&mut store, &output_instance, wasm_table_base)?;

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
