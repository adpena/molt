use super::process_task_state;
use super::wake_await_waiters;
use crate::*;

mod child_resources;
#[cfg(not(target_arch = "wasm32"))]
mod native_io;
mod stdio;
#[cfg(target_arch = "wasm32")]
mod wasm_host;

pub use stdio::molt_asyncio_subprocess_stdio_normalize;
#[cfg(target_arch = "wasm32")]
pub(crate) use wasm_host::{ProcessRegistry, ProcessTaskState};
#[cfg(target_arch = "wasm32")]
pub use wasm_host::{
    molt_process_drop, molt_process_host_notify, molt_process_kill, molt_process_pid,
    molt_process_poll, molt_process_returncode, molt_process_spawn, molt_process_spawn_ex,
    molt_process_stderr, molt_process_stdin, molt_process_stdout, molt_process_terminate,
    molt_process_wait_future,
};

#[cfg(not(target_arch = "wasm32"))]
use child_resources::apply_child_resource_env;
#[cfg(unix)]
use child_resources::{apply_child_memory_rlimit, configure_unix_owned_process_group};
#[cfg(not(target_arch = "wasm32"))]
use native_io::{attach_process_stdio, ignore_sigpipe, trace_process_io};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, JoinHandle};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
#[cfg(not(target_arch = "wasm32"))]
use stdio::configure_native_stdio;
#[cfg(not(target_arch = "wasm32"))]
use stdio::process_stdio_mode;

// --- Process ---

#[cfg(not(target_arch = "wasm32"))]
const PROCESS_TEARDOWN_TERM_GRACE_MS_ENV: &str = "MOLT_PROCESS_TEARDOWN_TERM_GRACE_MS";
#[cfg(not(target_arch = "wasm32"))]
const PROCESS_TEARDOWN_JOIN_TIMEOUT_MS_ENV: &str = "MOLT_PROCESS_TEARDOWN_JOIN_TIMEOUT_MS";
#[cfg(not(target_arch = "wasm32"))]
const PROCESS_TEARDOWN_TERM_GRACE_MS_DEFAULT: u64 = 50;
#[cfg(not(target_arch = "wasm32"))]
const PROCESS_TEARDOWN_JOIN_TIMEOUT_MS_DEFAULT: u64 = 1_000;

#[cfg(not(target_arch = "wasm32"))]
fn trace_process_spawn() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_PROCESS_SPAWN").ok().as_deref(),
            Some("1")
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `args_bits`, `env_bits`, and `cwd_bits` must be valid runtime-encoded objects.
/// The runtime must be initialized and the call must be allowed to enter the GIL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_spawn(
    args_bits: u64,
    env_bits: u64,
    cwd_bits: u64,
    stdin_bits: u64,
    stdout_bits: u64,
    stderr_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        ignore_sigpipe();
        if require_process_capability::<u64>(_py, &["process", "process.exec"]).is_err() {
            return MoltObject::none().bits();
        }
        let args = match argv_from_bits(_py, args_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if args.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "args must not be empty");
        }
        if trace_process_spawn() {
            let head = args
                .iter()
                .take(3)
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            eprintln!("molt_process_spawn args_head={head:?}");
        }
        let mut cmd = std::process::Command::new(&args[0]);
        if args.len() > 1 {
            cmd.args(&args[1..]);
        }
        let mut env_entries = match env_from_bits(_py, env_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mut overlay_env = false;
        if let Some(entries) = env_entries.as_mut() {
            entries.retain(|(key, value)| {
                if key == "MOLT_ENV_OVERLAY" && value == "1" {
                    overlay_env = true;
                    false
                } else {
                    true
                }
            });
        }
        if let Some(env_entries) = env_entries.as_ref() {
            if !overlay_env {
                cmd.env_clear();
            }
            if trace_process_io() {
                let mut has_entry = false;
                let mut has_spawn = false;
                let mut has_trusted = false;
                for (key, _value) in env_entries {
                    if key == "MOLT_ENTRY_MODULE" {
                        has_entry = true;
                    } else if key == "MOLT_MP_SPAWN" {
                        has_spawn = true;
                    } else if key == "MOLT_TRUSTED" {
                        has_trusted = true;
                    }
                }
                eprintln!(
                    "molt_process_env overlay={overlay_env} entry={has_entry} spawn={has_spawn} trusted={has_trusted}"
                );
            }
            for (key, value) in env_entries {
                cmd.env(key, value);
            }
        }
        apply_child_resource_env(&mut cmd, env_entries.as_deref());
        if !obj_from_bits(cwd_bits).is_none() {
            let cwd = match path_from_bits(_py, cwd_bits) {
                Ok(path) => path,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            cmd.current_dir(cwd);
        }
        #[cfg(unix)]
        let owns_process_group = configure_unix_owned_process_group(&mut cmd, false, None);
        #[cfg(not(unix))]
        let owns_process_group = false;
        #[cfg(unix)]
        apply_child_memory_rlimit(&mut cmd);
        let stdin_mode = process_stdio_mode(_py, stdin_bits, "stdin");
        let stdout_mode = process_stdio_mode(_py, stdout_bits, "stdout");
        let stderr_mode = process_stdio_mode(_py, stderr_bits, "stderr");
        if trace_process_io() {
            eprintln!(
                "molt_process_stdio stdin={stdin_mode} stdout={stdout_mode} stderr={stderr_mode}"
            );
        }

        let mut process_stdio =
            match configure_native_stdio(_py, &mut cmd, stdin_mode, stdout_mode, stderr_mode) {
                Ok(stdio) => stdio,
                Err(err) => return err,
            };

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                process_stdio.drop_streams();
                return raise_os_error::<u64>(_py, err, "spawn");
            }
        };

        attach_process_stdio(&mut child, &mut process_stdio);

        let pid = child.id();
        let owned_process_group = if owns_process_group {
            Some(pid as i32)
        } else {
            None
        };
        let registry_id = runtime_state(_py).process_registry.allocate_id();
        let state = Arc::new(ProcessState {
            registry_id,
            child: Mutex::new(child),
            pid,
            owned_process_group,
            exit_code: AtomicI32::new(PROCESS_EXIT_PENDING),
            kill_requested: AtomicBool::new(false),
            teardown_draining: AtomicBool::new(false),
            streams_released: AtomicBool::new(false),
            wait_future: Mutex::new(None),
            stdin_stream: process_stdio.stdin_stream,
            stdout_stream: process_stdio.stdout_stream,
            stderr_stream: process_stdio.stderr_stream,
            wait_lock: Mutex::new(()),
            condvar: Condvar::new(),
        });
        runtime_state(_py)
            .process_registry
            .register_pending(Arc::clone(&state));
        let worker_state = Arc::clone(&state);
        let wait_thread = thread::spawn(move || process_wait_worker(worker_state));
        runtime_state(_py)
            .process_registry
            .attach_wait_thread(registry_id, wait_thread);
        let handle = Box::new(MoltProcessHandle { state });
        opaque_handle_bits(Box::into_raw(handle) as *mut u8)
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// All arguments must be valid runtime-encoded objects.
/// `start_new_session_bits` should be a bool (truthy → setsid).
/// `process_group_bits` should be int or None (None → ignore, int → setpgid(0, pgid)).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_spawn_ex(
    args_bits: u64,
    env_bits: u64,
    cwd_bits: u64,
    stdin_bits: u64,
    stdout_bits: u64,
    stderr_bits: u64,
    start_new_session_bits: u64,
    process_group_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        ignore_sigpipe();
        if require_process_capability::<u64>(_py, &["process", "process.exec"]).is_err() {
            return MoltObject::none().bits();
        }
        let args = match argv_from_bits(_py, args_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if args.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "args must not be empty");
        }
        if trace_process_spawn() {
            let head = args
                .iter()
                .take(3)
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            eprintln!("molt_process_spawn_ex args_head={head:?}");
        }
        let mut cmd = std::process::Command::new(&args[0]);
        if args.len() > 1 {
            cmd.args(&args[1..]);
        }
        let mut env_entries = match env_from_bits(_py, env_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mut overlay_env = false;
        if let Some(entries) = env_entries.as_mut() {
            entries.retain(|(key, value)| {
                if key == "MOLT_ENV_OVERLAY" && value == "1" {
                    overlay_env = true;
                    false
                } else {
                    true
                }
            });
        }
        if let Some(env_entries) = env_entries.as_ref() {
            if !overlay_env {
                cmd.env_clear();
            }
            for (key, value) in env_entries {
                cmd.env(key, value);
            }
        }
        apply_child_resource_env(&mut cmd, env_entries.as_deref());
        if !obj_from_bits(cwd_bits).is_none() {
            let cwd = match path_from_bits(_py, cwd_bits) {
                Ok(path) => path,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            cmd.current_dir(cwd);
        }
        let stdin_mode = process_stdio_mode(_py, stdin_bits, "stdin");
        let stdout_mode = process_stdio_mode(_py, stdout_bits, "stdout");
        let stderr_mode = process_stdio_mode(_py, stderr_bits, "stderr");

        // Process-session controls are Unix process-model operations.
        #[cfg(unix)]
        let new_session = is_truthy(_py, obj_from_bits(start_new_session_bits));
        #[cfg(not(unix))]
        if is_truthy(_py, obj_from_bits(start_new_session_bits)) {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "start_new_session is unavailable on this host",
            );
        }
        #[cfg(unix)]
        let pg_obj = obj_from_bits(process_group_bits);
        #[cfg(not(unix))]
        let pg_obj = obj_from_bits(process_group_bits);
        #[cfg(unix)]
        let process_group_val: Option<i64> = if pg_obj.is_none() {
            None
        } else {
            match to_i64(pg_obj) {
                Some(v) => Some(v),
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "process_group must be an integer or None",
                    );
                }
            }
        };
        #[cfg(not(unix))]
        if !pg_obj.is_none() {
            if to_i64(pg_obj).is_none() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "process_group must be an integer or None",
                );
            }
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "process_group is unavailable on this host",
            );
        }

        #[cfg(unix)]
        let owns_process_group =
            configure_unix_owned_process_group(&mut cmd, new_session, process_group_val);
        #[cfg(not(unix))]
        let owns_process_group = false;
        #[cfg(unix)]
        apply_child_memory_rlimit(&mut cmd);

        let mut process_stdio =
            match configure_native_stdio(_py, &mut cmd, stdin_mode, stdout_mode, stderr_mode) {
                Ok(stdio) => stdio,
                Err(err) => return err,
            };

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                process_stdio.drop_streams();
                return raise_os_error::<u64>(_py, err, "spawn");
            }
        };

        attach_process_stdio(&mut child, &mut process_stdio);

        let pid = child.id();
        let owned_process_group = if owns_process_group {
            Some(pid as i32)
        } else {
            None
        };
        let registry_id = runtime_state(_py).process_registry.allocate_id();
        let state = Arc::new(ProcessState {
            registry_id,
            child: Mutex::new(child),
            pid,
            owned_process_group,
            exit_code: AtomicI32::new(PROCESS_EXIT_PENDING),
            kill_requested: AtomicBool::new(false),
            teardown_draining: AtomicBool::new(false),
            streams_released: AtomicBool::new(false),
            wait_future: Mutex::new(None),
            stdin_stream: process_stdio.stdin_stream,
            stdout_stream: process_stdio.stdout_stream,
            stderr_stream: process_stdio.stderr_stream,
            wait_lock: Mutex::new(()),
            condvar: Condvar::new(),
        });
        runtime_state(_py)
            .process_registry
            .register_pending(Arc::clone(&state));
        let worker_state = Arc::clone(&state);
        let wait_thread = thread::spawn(move || process_wait_worker(worker_state));
        runtime_state(_py)
            .process_registry
            .attach_wait_thread(registry_id, wait_thread);
        let handle = Box::new(MoltProcessHandle { state });
        opaque_handle_bits(Box::into_raw(handle) as *mut u8)
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_wait_future(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            let state = Arc::clone(&handle.state);
            if let Some(existing) = *state.wait_future.lock().unwrap() {
                let bits = MoltObject::from_ptr(existing.0).bits();
                inc_ref_bits(_py, bits);
                return bits;
            }
            let future_bits = molt_future_new(process_poll_fn_addr(), 0);
            let Some(future_ptr) = resolve_obj_ptr(future_bits) else {
                return MoltObject::none().bits();
            };
            let task_state = Arc::new(ProcessTaskState {
                process: state,
                cancelled: AtomicBool::new(false),
            });
            runtime_state(_py)
                .process_tasks
                .lock()
                .unwrap()
                .insert(PtrSlot(future_ptr), Arc::clone(&task_state));
            *task_state.process.wait_future.lock().unwrap() = Some(PtrSlot(future_ptr));
            future_bits
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `obj_bits` must be a valid process wait future object from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let Some(state) = process_task_state(_py, obj_ptr) else {
            return raise_exception::<i64>(_py, "RuntimeError", "process task missing");
        };
        if state.process.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            task_take_cancel_pending(obj_ptr);
        } else if task_cancel_pending(obj_ptr) {
            task_take_cancel_pending(obj_ptr);
            state.cancelled.store(true, AtomicOrdering::Release);
            return raise_cancelled_with_message::<i64>(_py, obj_ptr);
        }
        let code = state.process.exit_code.load(AtomicOrdering::Acquire);
        if code == PROCESS_EXIT_PENDING {
            return pending_bits_i64();
        }
        MoltObject::from_int(code as i64).bits() as i64
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_pid(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::from_int(0).bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            MoltObject::from_int(handle.state.pid as i64).bits()
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_returncode(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            let code = handle.state.exit_code.load(AtomicOrdering::Acquire);
            if code == PROCESS_EXIT_PENDING {
                MoltObject::none().bits()
            } else {
                MoltObject::from_int(code as i64).bits()
            }
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_kill(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            if handle.state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
                return MoltObject::none().bits();
            }
            if let Err(err) = handle.state.request_kill() {
                return raise_os_error::<u64>(_py, err, "kill");
            }
            MoltObject::none().bits()
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_terminate(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            if handle.state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
                return MoltObject::none().bits();
            }
            if let Err(err) = handle.state.request_terminate() {
                return raise_os_error::<u64>(_py, err, "terminate");
            }
            MoltObject::none().bits()
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_stdin(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            if handle.state.stdin_stream == 0 {
                return MoltObject::none().bits();
            }
            molt_stream_clone(handle.state.stdin_stream)
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_stdout(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            if handle.state.stdout_stream == 0 {
                return MoltObject::none().bits();
            }
            molt_stream_clone(handle.state.stdout_stream)
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_stderr(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            if handle.state.stderr_stream == 0 {
                return MoltObject::none().bits();
            }
            molt_stream_clone(handle.state.stderr_stream)
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_drop(proc_bits: u64) {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return;
            }
            release_ptr(proc_ptr);
            drop(Box::from_raw(proc_ptr as *mut MoltProcessHandle));
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ProcessState {
    registry_id: u64,
    child: Mutex<std::process::Child>,
    pub(crate) pid: u32,
    #[cfg_attr(not(unix), allow(dead_code))]
    owned_process_group: Option<i32>,
    pub(crate) exit_code: AtomicI32,
    kill_requested: AtomicBool,
    teardown_draining: AtomicBool,
    streams_released: AtomicBool,
    pub(crate) wait_future: Mutex<Option<PtrSlot>>,
    stdin_stream: u64,
    stdout_stream: u64,
    stderr_stream: u64,
    wait_lock: Mutex<()>,
    pub(crate) condvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
struct ProcessRegistryEntry {
    state: Arc<ProcessState>,
    wait_thread: Option<JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct ProcessRegistryInner {
    next_id: u64,
    entries: HashMap<u64, ProcessRegistryEntry>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ProcessRegistry {
    inner: Mutex<ProcessRegistryInner>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(ProcessRegistryInner {
                next_id: 1,
                entries: HashMap::new(),
            }),
        }
    }

    pub(crate) fn allocate_id(&self) -> u64 {
        let mut guard = self.inner.lock().unwrap();
        let id = guard.next_id;
        guard.next_id = guard.next_id.checked_add(1).unwrap_or(1);
        id
    }

    pub(crate) fn register_pending(&self, state: Arc<ProcessState>) {
        let mut guard = self.inner.lock().unwrap();
        guard.entries.insert(
            state.registry_id,
            ProcessRegistryEntry {
                state,
                wait_thread: None,
            },
        );
    }

    pub(crate) fn attach_wait_thread(&self, id: u64, wait_thread: JoinHandle<()>) {
        let mut wait_thread = Some(wait_thread);
        {
            let mut guard = self.inner.lock().unwrap();
            if let Some(entry) = guard.entries.get_mut(&id) {
                entry.wait_thread = wait_thread.take();
            }
        }
        if let Some(wait_thread) = wait_thread
            && wait_thread.is_finished()
        {
            let _ = wait_thread.join();
        }
    }

    pub(crate) fn finish_wait_worker(&self, id: u64) {
        let mut guard = self.inner.lock().unwrap();
        guard.entries.remove(&id);
    }

    pub(crate) fn drain_for_teardown(&self) {
        let entries = {
            let mut guard = self.inner.lock().unwrap();
            std::mem::take(&mut guard.entries)
        };
        if entries.is_empty() {
            return;
        }

        let term_grace = process_teardown_duration(
            PROCESS_TEARDOWN_TERM_GRACE_MS_ENV,
            PROCESS_TEARDOWN_TERM_GRACE_MS_DEFAULT,
        );
        let join_timeout = process_teardown_duration(
            PROCESS_TEARDOWN_JOIN_TIMEOUT_MS_ENV,
            PROCESS_TEARDOWN_JOIN_TIMEOUT_MS_DEFAULT,
        );
        for entry in entries.values() {
            entry
                .state
                .teardown_draining
                .store(true, AtomicOrdering::Release);
            let _ = entry.state.wait_future.lock().unwrap().take();
            entry.state.request_terminate_for_teardown();
        }
        let term_deadline = Instant::now() + term_grace;
        for entry in entries.values() {
            let remaining = term_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = entry.state.wait_for_exit(remaining);
        }
        for entry in entries.values() {
            if entry.state.exit_code.load(AtomicOrdering::Acquire) == PROCESS_EXIT_PENDING {
                entry.state.request_kill_for_teardown();
            }
            entry.state.release_owned_streams();
        }
        let join_deadline = Instant::now() + join_timeout;
        for mut entry in entries {
            if let Some(wait_thread) = entry.1.wait_thread.take() {
                let remaining = join_deadline.saturating_duration_since(Instant::now());
                if !remaining.is_zero() {
                    let _ = entry.1.state.wait_for_exit(remaining);
                }
                if wait_thread.is_finished() {
                    let _ = wait_thread.join();
                }
            }
        }
    }

    #[cfg(all(test, unix, not(target_arch = "wasm32")))]
    fn live_count(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn process_teardown_duration(env_key: &str, default_ms: u64) -> Duration {
    let millis = std::env::var(env_key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(default_ms);
    Duration::from_millis(millis)
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessState {
    fn wait_for_exit(&self, timeout: Duration) -> bool {
        if self.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            return true;
        }
        let deadline = Instant::now() + timeout;
        let mut guard = self.wait_lock.lock().unwrap();
        loop {
            if self.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next_guard, _) = self.condvar.wait_timeout(guard, remaining).unwrap();
            guard = next_guard;
        }
    }

    fn release_owned_streams(&self) {
        if self.streams_released.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        self.close_and_drop_stream(self.stdin_stream);
        self.close_and_drop_stream(self.stdout_stream);
        self.close_and_drop_stream(self.stderr_stream);
    }

    fn close_and_drop_stream(&self, stream_bits: u64) {
        if stream_bits == 0 {
            return;
        }
        let stream_ptr = ptr_from_bits(stream_bits);
        if !stream_ptr.is_null() {
            let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
            super::channels::stream_close_local(stream);
        }
        unsafe {
            molt_stream_drop(stream_bits);
        }
    }

    fn request_terminate_for_teardown(&self) {
        let _ = self.request_terminate();
    }

    fn request_kill_for_teardown(&self) {
        let _ = self.request_kill();
    }

    fn request_kill_for_drop(&self) {
        let _ = self.request_kill();
    }

    fn request_terminate(&self) -> Result<(), std::io::Error> {
        if self.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            return Ok(());
        }
        #[cfg(unix)]
        {
            self.signal_unix(libc::SIGTERM)
        }
        #[cfg(not(unix))]
        {
            self.kill_child_handle()
        }
    }

    fn request_kill(&self) -> Result<(), std::io::Error> {
        if self.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            return Ok(());
        }
        self.kill_requested.store(true, AtomicOrdering::Release);
        #[cfg(unix)]
        {
            self.signal_unix(libc::SIGKILL)
        }
        #[cfg(not(unix))]
        {
            self.kill_child_handle()
        }
    }

    #[cfg(unix)]
    fn signal_unix(&self, signal: i32) -> Result<(), std::io::Error> {
        if let Some(pgid) = self.owned_process_group {
            let rc = unsafe { libc::kill(-pgid as libc::pid_t, signal) };
            if rc == 0 {
                return Ok(());
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err);
            }
        }
        let rc = unsafe { libc::kill(self.pid as libc::pid_t, signal) };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(err)
        }
    }

    #[cfg(not(unix))]
    fn kill_child_handle(&self) -> Result<(), std::io::Error> {
        let mut guard = self.child.lock().unwrap();
        guard.kill()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ProcessTaskState {
    pub(crate) process: Arc<ProcessState>,
    pub(crate) cancelled: AtomicBool,
}

// Process tasks only touch shared state under locks; safe to share across threads.
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Send for ProcessTaskState {}
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Sync for ProcessTaskState {}

#[cfg(not(target_arch = "wasm32"))]
struct MoltProcessHandle {
    state: Arc<ProcessState>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for MoltProcessHandle {
    fn drop(&mut self) {
        self.state.request_kill_for_drop();
        self.state.release_owned_streams();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for ProcessState {
    fn drop(&mut self) {
        self.request_kill_for_drop();
        self.release_owned_streams();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessTaskState {
    pub(crate) fn wait_blocking(&self, timeout: Option<Duration>) {
        if self.process.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            return;
        }
        let mut guard = self.process.wait_lock.lock().unwrap();
        loop {
            if self.process.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
                break;
            }
            match timeout {
                Some(wait) => {
                    let _ = self.process.condvar.wait_timeout(guard, wait).unwrap();
                    break;
                }
                None => {
                    guard = self.process.condvar.wait(guard).unwrap();
                }
            }
        }
    }
}

const PROCESS_EXIT_PENDING: i32 = i32::MIN;

#[cfg(not(target_arch = "wasm32"))]
fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -sig;
        }
    }
    -1
}

#[cfg(not(target_arch = "wasm32"))]
fn process_wait_worker(state: Arc<ProcessState>) {
    loop {
        if state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            break;
        }
        if state.kill_requested.load(AtomicOrdering::Acquire) {
            let _ = state.request_kill();
        }
        let mut guard = state.child.lock().unwrap();
        match guard.try_wait() {
            Ok(Some(status)) => {
                let code = exit_code_from_status(status);
                state.exit_code.store(code, AtomicOrdering::Release);
                if trace_process_io() {
                    eprintln!("molt_process_wait exit_code={code}");
                }
                drop(guard);
                state.condvar.notify_all();
                if !state.teardown_draining.load(AtomicOrdering::Acquire)
                    && let Some(future) = state.wait_future.lock().unwrap().take()
                {
                    let gil = GilGuard::new();
                    let py = gil.token();
                    let _ = wake_await_waiters(&py, future.0);
                }
                break;
            }
            Ok(None) => {}
            Err(_) => {}
        }
        drop(guard);
        thread::sleep(Duration::from_millis(10));
    }
    if let Some(runtime) = crate::state::runtime_state::runtime_state_for_gil() {
        runtime
            .process_registry
            .finish_wait_worker(state.registry_id);
    }
}

#[cfg(all(test, unix, not(target_arch = "wasm32")))]
mod process_registry_tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn registered_child(cmd: &mut Command, registry: &ProcessRegistry) -> Arc<ProcessState> {
        let owns_process_group = configure_unix_owned_process_group(cmd, false, None);
        assert!(owns_process_group);
        let child = cmd.spawn().expect("spawn test child");
        let pid = child.id();
        let registry_id = registry.allocate_id();
        let state = Arc::new(ProcessState {
            registry_id,
            child: Mutex::new(child),
            pid,
            owned_process_group: Some(pid as i32),
            exit_code: AtomicI32::new(PROCESS_EXIT_PENDING),
            kill_requested: AtomicBool::new(false),
            teardown_draining: AtomicBool::new(false),
            streams_released: AtomicBool::new(false),
            wait_future: Mutex::new(None),
            stdin_stream: 0,
            stdout_stream: 0,
            stderr_stream: 0,
            wait_lock: Mutex::new(()),
            condvar: Condvar::new(),
        });
        registry.register_pending(Arc::clone(&state));
        let worker_state = Arc::clone(&state);
        let wait_thread = thread::spawn(move || process_wait_worker(worker_state));
        registry.attach_wait_thread(registry_id, wait_thread);
        state
    }

    fn unique_temp_path(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("molt-{name}-{}-{stamp}.pid", std::process::id()))
    }

    fn wait_for_pid_file(path: &std::path::Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(raw) = fs::read_to_string(path) {
                if let Ok(pid) = raw.trim().parse::<i32>() {
                    return pid;
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for child pid file"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_process_exits(pid: i32) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return;
            }
            assert!(Instant::now() < deadline, "process {pid} is still alive");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn handle_drop_kills_child_even_while_wait_worker_holds_state() {
        let registry = ProcessRegistry::new();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 30");
        let state = registered_child(&mut cmd, &registry);
        let pid = state.pid as i32;
        drop(MoltProcessHandle {
            state: Arc::clone(&state),
        });
        assert!(state.wait_for_exit(Duration::from_secs(2)));
        registry.drain_for_teardown();
        assert_eq!(registry.live_count(), 0);
        assert_process_exits(pid);
    }

    #[test]
    fn registry_teardown_kills_owned_process_group_descendants() {
        let registry = ProcessRegistry::new();
        let pid_path = unique_temp_path("process-group");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("sleep 30 & echo $! > \"$MOLT_TEST_PID_FILE\"; wait")
            .env("MOLT_TEST_PID_FILE", &pid_path);
        let state = registered_child(&mut cmd, &registry);
        let shell_pid = state.pid as i32;
        let sleep_pid = wait_for_pid_file(&pid_path);
        registry.drain_for_teardown();
        let _ = fs::remove_file(pid_path);
        assert_eq!(registry.live_count(), 0);
        assert!(state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING);
        assert_process_exits(shell_pid);
        assert_process_exits(sleep_pid);
    }
}
