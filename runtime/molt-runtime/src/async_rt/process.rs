use super::process_task_state;
use super::wake_await_waiters;
use crate::BuiltinExc;
use crate::runtime_flags::{self, RuntimeFlag};
use crate::*;

mod child_resources;
#[cfg(unix)]
pub(crate) use child_resources::child_memory_rlimit_bytes;
#[cfg(not(target_arch = "wasm32"))]
mod native_io;
mod stdio;
#[cfg(target_arch = "wasm32")]
mod wasm_host;

pub use stdio::molt_asyncio_subprocess_stdio_normalize;
#[cfg(target_arch = "wasm32")]
pub(crate) use wasm_host::{ProcessRegistry, ProcessState};
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
use std::num::NonZeroUsize;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Condvar;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
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

/// Target-independent terminal state for one spawned process.
///
/// The exit code is published exactly once. The single process-wait future is
/// separately consumed by completion or cancellation, so a completion/cancel
/// race can wake at most one scheduler edge.
struct ProcessCompletionState {
    exit_code: AtomicI32,
    wait_future: Mutex<Option<PtrSlot>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProcessWaiterId(NonZeroUsize);

impl ProcessWaiterId {
    #[inline]
    fn from_slot(waiter: PtrSlot) -> Self {
        Self(
            NonZeroUsize::new(waiter.0 as usize).expect("process waiter identity must be non-null"),
        )
    }

    #[inline]
    fn matches(self, waiter: PtrSlot) -> bool {
        self.0.get() == waiter.0 as usize
    }
}

enum ProcessWaitFutureInstall {
    Installed,
    AlreadyInstalled(PtrSlot),
    Terminal(i32),
}

impl ProcessCompletionState {
    fn pending() -> Self {
        Self {
            exit_code: AtomicI32::new(PROCESS_EXIT_PENDING),
            wait_future: Mutex::new(None),
        }
    }

    #[inline]
    fn exit_code(&self) -> i32 {
        self.exit_code.load(AtomicOrdering::Acquire)
    }

    #[inline]
    fn is_pending(&self) -> bool {
        self.exit_code() == PROCESS_EXIT_PENDING
    }

    fn publish_exit(&self, exit_code: i32) -> Option<()> {
        if exit_code == PROCESS_EXIT_PENDING {
            return None;
        }
        // Serialize with installation/cancellation and the blocking-wait CV,
        // but leave the raw waiter revocable until GIL-held dispatch claims it.
        let _wait_future = self.wait_future.lock().unwrap();
        if self
            .exit_code
            .compare_exchange(
                PROCESS_EXIT_PENDING,
                exit_code,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .is_err()
        {
            return None;
        }
        Some(())
    }

    fn wait_future(&self) -> Option<PtrSlot> {
        *self.wait_future.lock().unwrap()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn has_wait_future(&self) -> bool {
        self.wait_future.lock().unwrap().is_some()
    }

    fn claim_wait_future(&self, _py: &PyToken<'_>) -> Option<PtrSlot> {
        crate::gil_assert();
        assert!(
            !self.is_pending(),
            "process waiter claimed before terminal publication"
        );
        self.wait_future.lock().unwrap().take()
    }

    fn install_wait_future(&self, future: PtrSlot) -> ProcessWaitFutureInstall {
        let mut guard = self.wait_future.lock().unwrap();
        let exit_code = self.exit_code();
        if exit_code != PROCESS_EXIT_PENDING {
            return ProcessWaitFutureInstall::Terminal(exit_code);
        }
        if let Some(existing) = *guard {
            return ProcessWaitFutureInstall::AlreadyInstalled(existing);
        }
        *guard = Some(future);
        ProcessWaitFutureInstall::Installed
    }

    fn cancel_wait_future(&self, waiter_id: ProcessWaiterId) -> bool {
        let mut guard = self.wait_future.lock().unwrap();
        if guard.is_some_and(|waiter| waiter_id.matches(waiter)) {
            *guard = None;
            return true;
        }
        false
    }
}

pub(crate) struct ProcessTaskState {
    process: Arc<ProcessState>,
    // Stable identity only. The scheduler never carries a dereferenceable raw
    // pointer across threads, so the state is naturally Send + Sync.
    waiter_id: ProcessWaiterId,
    cancelled: AtomicBool,
}

impl ProcessTaskState {
    pub(crate) fn cancel_wait(&self) -> bool {
        if self.cancelled.swap(true, AtomicOrdering::AcqRel) {
            return false;
        }
        self.process.cancel_wait(self.waiter_id)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn trace_process_spawn() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_PROCESS_SPAWN)
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
        if require_process_capability::<u64>(_py, crate::OperationId::ProcessExec).is_err() {
            return MoltObject::none().bits();
        }
        let args = match argv_from_bits(_py, args_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        if args.is_empty() {
            return raise_exception::<_>(_py, BuiltinExc::ValueError, "args must not be empty");
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
            Err(bits) => return bits,
        };
        let mut overlay_env = false;
        if let Some(entries) = env_entries.as_mut() {
            entries.retain(|(key, value)| {
                if key == RuntimeFlag::MOLT_ENV_OVERLAY.name() && value == "1" {
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
                let mut has_capability_tier = false;
                for (key, _value) in env_entries {
                    if key == "MOLT_ENTRY_MODULE" {
                        has_entry = true;
                    } else if key == RuntimeFlag::MOLT_MP_SPAWN.name() {
                        has_spawn = true;
                    } else if key == "MOLT_CAPABILITY_TIER" {
                        has_capability_tier = true;
                    }
                }
                eprintln!(
                    "molt_process_env overlay={overlay_env} entry={has_entry} spawn={has_spawn} capability_tier={has_capability_tier}"
                );
            }
            for (key, value) in env_entries {
                cmd.env(key, value);
            }
        }
        apply_child_resource_env(
            &mut cmd,
            env_entries.as_deref(),
            runtime_state(_py).policy().resources.as_ref(),
        );
        if !obj_from_bits(cwd_bits).is_none() {
            let cwd = match path_from_bits(_py, cwd_bits) {
                Ok(path) => path,
                Err(bits) => return bits,
            };
            cmd.current_dir(cwd);
        }
        #[cfg(unix)]
        let owns_process_group = configure_unix_owned_process_group(&mut cmd, false, None);
        #[cfg(unix)]
        apply_child_memory_rlimit(
            &mut cmd,
            runtime_state(_py).policy().child_memory_rlimit_bytes,
        );
        let stdin_mode = process_stdio_mode(_py, stdin_bits, "stdin");
        let stdout_mode = process_stdio_mode(_py, stdout_bits, "stdout");
        let stderr_mode = process_stdio_mode(_py, stderr_bits, "stderr");
        if trace_process_io() {
            eprintln!(
                "molt_process_stdio stdin={stdin_mode} stdout={stdout_mode} stderr={stderr_mode}"
            );
        }

        let process_stdio =
            match configure_native_stdio(_py, &mut cmd, stdin_mode, stdout_mode, stderr_mode) {
                Ok(stdio) => stdio,
                Err(err) => return err,
            };

        let state = match runtime_state(_py).process_registry.spawn(
            _py,
            &mut cmd,
            process_stdio,
            owns_process_group,
        ) {
            Ok(state) => state,
            Err(err) => return raise_os_error::<u64>(_py, err, "spawn"),
        };
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
        if require_process_capability::<u64>(_py, crate::OperationId::ProcessExec).is_err() {
            return MoltObject::none().bits();
        }
        let args = match argv_from_bits(_py, args_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        if args.is_empty() {
            return raise_exception::<_>(_py, BuiltinExc::ValueError, "args must not be empty");
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
            Err(bits) => return bits,
        };
        let mut overlay_env = false;
        if let Some(entries) = env_entries.as_mut() {
            entries.retain(|(key, value)| {
                if key == RuntimeFlag::MOLT_ENV_OVERLAY.name() && value == "1" {
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
        apply_child_resource_env(
            &mut cmd,
            env_entries.as_deref(),
            runtime_state(_py).policy().resources.as_ref(),
        );
        if !obj_from_bits(cwd_bits).is_none() {
            let cwd = match path_from_bits(_py, cwd_bits) {
                Ok(path) => path,
                Err(bits) => return bits,
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
                BuiltinExc::RuntimeError,
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
                        BuiltinExc::TypeError,
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
                    BuiltinExc::TypeError,
                    "process_group must be an integer or None",
                );
            }
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "process_group is unavailable on this host",
            );
        }

        #[cfg(unix)]
        let owns_process_group =
            configure_unix_owned_process_group(&mut cmd, new_session, process_group_val);
        #[cfg(unix)]
        apply_child_memory_rlimit(
            &mut cmd,
            runtime_state(_py).policy().child_memory_rlimit_bytes,
        );

        let process_stdio =
            match configure_native_stdio(_py, &mut cmd, stdin_mode, stdout_mode, stderr_mode) {
                Ok(stdio) => stdio,
                Err(err) => return err,
            };

        let state = match runtime_state(_py).process_registry.spawn(
            _py,
            &mut cmd,
            process_stdio,
            owns_process_group,
        ) {
            Ok(state) => state,
            Err(err) => return raise_os_error::<u64>(_py, err, "spawn"),
        };
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
            if let Some(existing) = state.wait_future() {
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
                waiter_id: ProcessWaiterId::from_slot(PtrSlot(future_ptr)),
                cancelled: AtomicBool::new(false),
            });
            runtime_state(_py)
                .process_tasks
                .lock()
                .unwrap()
                .insert(PtrSlot(future_ptr), Arc::clone(&task_state));
            match task_state.process.install_wait_future(PtrSlot(future_ptr)) {
                ProcessWaitFutureInstall::Installed => future_bits,
                ProcessWaitFutureInstall::Terminal(exit_code) => {
                    debug_assert_eq!(task_state.process.exit_code(), exit_code);
                    future_bits
                }
                ProcessWaitFutureInstall::AlreadyInstalled(existing) => {
                    runtime_state(_py)
                        .process_tasks
                        .lock()
                        .unwrap()
                        .remove(&PtrSlot(future_ptr));
                    dec_ref_bits(_py, future_bits);
                    let bits = MoltObject::from_ptr(existing.0).bits();
                    inc_ref_bits(_py, bits);
                    bits
                }
            }
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
            return raise_exception::<i64>(_py, BuiltinExc::RuntimeError, "process task missing");
        };
        if !state.process.is_pending() {
            task_take_cancel_pending(obj_ptr);
        } else if task_cancel_pending(obj_ptr) {
            task_take_cancel_pending(obj_ptr);
            state.cancel_wait();
            return raise_cancelled_with_message::<i64>(_py, obj_ptr);
        }
        let code = state.process.exit_code();
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
            let code = handle.state.exit_code();
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
            if !handle.state.is_pending() {
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
            if !handle.state.is_pending() {
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
pub unsafe extern "C" fn molt_process_drop(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            release_ptr(proc_ptr);
            drop(Box::from_raw(proc_ptr as *mut MoltProcessHandle));
            MoltObject::none().bits()
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ProcessState {
    registry_id: u64,
    // The registry is inside this origin and cannot retire it until every
    // actual worker has joined. Only the waiter uses this pointer, under
    // canonical execution custody and a second teardown check.
    origin_runtime: usize,
    child: Mutex<std::process::Child>,
    pub(crate) pid: u32,
    #[cfg(unix)]
    owned_process_group: Option<i32>,
    completion: ProcessCompletionState,
    kill_requested: AtomicBool,
    teardown_draining: AtomicBool,
    streams_released: AtomicBool,
    stdin_stream: u64,
    stdout_stream: u64,
    stderr_stream: u64,
    pub(crate) condvar: Condvar,
}

impl ProcessState {
    #[inline]
    fn exit_code(&self) -> i32 {
        self.completion.exit_code()
    }

    #[inline]
    fn is_pending(&self) -> bool {
        self.completion.is_pending()
    }

    fn wait_future(&self) -> Option<PtrSlot> {
        self.completion.wait_future()
    }

    fn install_wait_future(&self, future: PtrSlot) -> ProcessWaitFutureInstall {
        self.completion.install_wait_future(future)
    }

    fn publish_exit(&self, exit_code: i32) -> Option<()> {
        self.completion.publish_exit(exit_code)?;
        #[cfg(not(target_arch = "wasm32"))]
        self.condvar.notify_all();
        Some(())
    }

    fn cancel_wait(&self, waiter_id: ProcessWaiterId) -> bool {
        let removed = self.completion.cancel_wait_future(waiter_id);
        #[cfg(not(target_arch = "wasm32"))]
        if removed {
            self.condvar.notify_all();
        }
        removed
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ProcessWorker {
    handle: Option<JoinHandle<()>>,
    joined: bool,
    // A failed worker may have failed runtime/TLS cleanup. Retain its exact
    // disposition and origin, even though its native thread has terminated.
    panic: Option<Box<dyn std::any::Any + Send + 'static>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct ProcessRegistryEntry {
    state: Arc<ProcessState>,
    workers: Vec<ProcessWorker>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessRegistryEntry {
    fn spawn_worker(
        &mut self,
        name: &'static str,
        worker: impl FnOnce() + Send + 'static,
    ) -> std::io::Result<()> {
        #[cfg(test)]
        if PROCESS_WORKER_SPAWN_FAILURE_AFTER.with(|remaining| match remaining.get() {
            Some(0) => {
                remaining.set(None);
                true
            }
            Some(count) => {
                remaining.set(Some(count - 1));
                false
            }
            None => false,
        }) {
            return Err(std::io::Error::other(
                "injected process worker spawn failure",
            ));
        }
        let handle = thread::Builder::new().name(name.into()).spawn(worker)?;
        self.workers.push(ProcessWorker {
            handle: Some(handle),
            joined: false,
            panic: None,
        });
        Ok(())
    }

    fn is_closed(&self) -> bool {
        !self.state.is_pending()
            && self
                .workers
                .iter()
                .all(|worker| worker.joined && worker.panic.is_none())
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ProcessRegistryInner {
    next_id: u64,
    closed: bool,
    entries: HashMap<u64, ProcessRegistryEntry>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ProcessRegistry {
    inner: Mutex<ProcessRegistryInner>,
    term_grace: Duration,
    join_timeout: Duration,
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessRegistry {
    pub(crate) fn new() -> Self {
        Self::with_deadlines(
            process_teardown_duration(
                PROCESS_TEARDOWN_TERM_GRACE_MS_ENV,
                PROCESS_TEARDOWN_TERM_GRACE_MS_DEFAULT,
            ),
            process_teardown_duration(
                PROCESS_TEARDOWN_JOIN_TIMEOUT_MS_ENV,
                PROCESS_TEARDOWN_JOIN_TIMEOUT_MS_DEFAULT,
            ),
        )
    }

    pub(crate) fn new_isolate(&self) -> Self {
        Self::with_deadlines(self.term_grace, self.join_timeout)
    }

    fn with_deadlines(term_grace: Duration, join_timeout: Duration) -> Self {
        Self {
            term_grace,
            join_timeout,
            inner: Mutex::new(ProcessRegistryInner {
                next_id: 1,
                closed: false,
                entries: HashMap::new(),
            }),
        }
    }

    fn spawn(
        &self,
        py: &PyToken<'_>,
        command: &mut std::process::Command,
        mut stdio: stdio::NativeProcessStdio,
        owns_process_group: bool,
    ) -> std::io::Result<Arc<ProcessState>> {
        // Only a Unix child can own a process group.
        #[cfg(not(unix))]
        debug_assert!(!owns_process_group);
        self.reap_finished();
        // Admission and publication share the registry mutex. Teardown cannot
        // observe an empty registry while an admitted child is being spawned.
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            drop(inner);
            stdio.drop_streams();
            return Err(std::io::Error::other("runtime process admission is closed"));
        }
        let registry_id = inner.next_id;
        let next_id = registry_id
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("runtime process id exhausted"));
        let next_id = match next_id {
            Ok(id) => id,
            Err(error) => {
                drop(inner);
                stdio.drop_streams();
                return Err(error);
            }
        };
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                drop(inner);
                stdio.drop_streams();
                return Err(error);
            }
        };
        let pid = child.id();
        inner.next_id = next_id;
        let state = Arc::new(ProcessState {
            registry_id,
            origin_runtime: (runtime_state(py) as *const RuntimeState).expose_provenance(),
            child: Mutex::new(child),
            pid,
            #[cfg(unix)]
            owned_process_group: owns_process_group.then_some(pid as i32),
            completion: ProcessCompletionState::pending(),
            kill_requested: AtomicBool::new(false),
            teardown_draining: AtomicBool::new(false),
            streams_released: AtomicBool::new(false),
            stdin_stream: stdio.stdin_stream,
            stdout_stream: stdio.stdout_stream,
            stderr_stream: stdio.stderr_stream,
            condvar: Condvar::new(),
        });
        let entry = inner
            .entries
            .entry(registry_id)
            .or_insert_with(|| ProcessRegistryEntry {
                state: Arc::clone(&state),
                workers: Vec::with_capacity(
                    1 + usize::from(stdio.stdin_stream != 0)
                        + usize::from(stdio.stdout_stream != 0)
                        + usize::from(stdio.stderr_stream != 0),
                ),
            });
        // Publish the allocation before the first native spawn. If a later
        // spawn fails, this same entry retains every already-started worker.
        let worker_state = Arc::clone(&state);
        let started = entry
            .spawn_worker("molt-process-wait", move || {
                process_wait_worker(worker_state)
            })
            .and_then(|()| {
                let mut child = state.child.lock().unwrap();
                attach_process_stdio(&mut child, &mut stdio, &state, entry)
            });
        if let Err(error) = started {
            state.teardown_draining.store(true, AtomicOrdering::Release);
            state.close_owned_streams();
            drop(inner);
            let closed = self.settle_workers(
                std::slice::from_ref(&state),
                Duration::ZERO,
                self.join_timeout,
            );
            return if closed {
                Err(error)
            } else {
                Err(std::io::Error::other(format!(
                    "{error}; process worker closure incomplete; origin retained"
                )))
            };
        }
        Ok(state)
    }

    fn reap_finished(&self) {
        // A finished native function may still be running its TLS destructors.
        // Never retain the GIL while completing the actual native join.
        {
            let _release = crate::gil_held().then(GilReleaseGuard::suspend);
            self.join_finished();
        }
        self.release_closed_entries();
    }

    fn join_finished(&self) {
        let ready = {
            let mut inner = self.inner.lock().unwrap();
            let mut ready = Vec::new();
            for (&id, entry) in &mut inner.entries {
                for (index, worker) in entry.workers.iter_mut().enumerate() {
                    if worker.handle.as_ref().is_some_and(JoinHandle::is_finished) {
                        ready.push((id, index, worker.handle.take().unwrap()));
                    }
                }
            }
            ready
        };
        // Neither the registry mutex nor the GIL may surround a native join:
        // the thread's final TLS destructor can itself enter runtime code.
        for (id, index, handle) in ready {
            let outcome = handle.join();
            let mut inner = self.inner.lock().unwrap();
            let worker = &mut inner
                .entries
                .get_mut(&id)
                .expect("joining process entry")
                .workers[index];
            worker.panic = outcome.err();
            worker.joined = true;
        }
    }

    fn release_closed_entries(&self) {
        let closed = {
            let mut inner = self.inner.lock().unwrap();
            let ids: Vec<_> = inner
                .entries
                .iter()
                .filter_map(|(&id, entry)| entry.is_closed().then_some(id))
                .collect();
            ids.into_iter()
                .filter_map(|id| inner.entries.remove(&id))
                .collect::<Vec<_>>()
        };
        // Stream release can enter the runtime. The caller's GIL/custody is
        // restored, and no registry mutex is held while owners are dropped.
        drop(closed);
    }

    fn settle_workers(
        &self,
        states: &[Arc<ProcessState>],
        term_grace: Duration,
        join_timeout: Duration,
    ) -> bool {
        let all_closed;
        {
            let _release = crate::gil_held().then(GilReleaseGuard::suspend);
            for state in states {
                state.request_terminate_for_teardown();
            }
            let term_deadline = Instant::now() + term_grace;
            while Instant::now() < term_deadline {
                let mut pending = false;
                for state in states {
                    // Also reaps a child whose waiter failed to start. Both
                    // this path and the waiter use the same child mutex and
                    // exactly-once terminal publication.
                    let _ = state.poll_child_exit();
                    pending |= state.is_pending();
                }
                if !pending {
                    break;
                }
                thread::sleep(
                    Duration::from_millis(1)
                        .min(term_deadline.saturating_duration_since(Instant::now())),
                );
            }
            for state in states {
                state.request_kill_for_teardown();
            }
            let join_deadline = Instant::now() + join_timeout;
            loop {
                for state in states {
                    let _ = state.poll_child_exit();
                }
                self.join_finished();
                let (closed, terminal) = {
                    let inner = self.inner.lock().unwrap();
                    let closed = states.iter().all(|state| {
                        inner
                            .entries
                            .get(&state.registry_id)
                            .is_none_or(ProcessRegistryEntry::is_closed)
                    });
                    let terminal = states.iter().all(|state| {
                        !state.is_pending()
                            && inner.entries.get(&state.registry_id).is_none_or(|entry| {
                                entry.workers.iter().all(|worker| worker.joined)
                            })
                    });
                    (closed, terminal)
                };
                if closed || terminal || Instant::now() >= join_deadline {
                    all_closed = closed;
                    break;
                }
                thread::sleep(
                    Duration::from_millis(1)
                        .min(join_deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
        self.release_closed_entries();
        all_closed
    }

    pub(crate) fn drain_for_teardown(&self) {
        self.drain_with_deadlines(self.term_grace, self.join_timeout);
    }

    fn drain_with_deadlines(&self, term_grace: Duration, join_timeout: Duration) {
        let states = {
            let mut inner = self.inner.lock().unwrap();
            inner.closed = true;
            inner
                .entries
                .values()
                .map(|entry| {
                    entry
                        .state
                        .teardown_draining
                        .store(true, AtomicOrdering::Release);
                    entry.state.close_owned_streams();
                    Arc::clone(&entry.state)
                })
                .collect::<Vec<_>>()
        };
        assert!(
            self.settle_workers(&states, term_grace, join_timeout),
            "runtime teardown refused: process workers remain live or failed; origin retained"
        );
    }

    #[cfg(test)]
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
    #[cfg(test)]
    fn wait_for_exit(&self, timeout: Duration) -> bool {
        if !self.is_pending() {
            return true;
        }
        let deadline = Instant::now() + timeout;
        // Wait on the same mutex that serializes terminal publication.  Using
        // a separate wait mutex permits publication to notify between the
        // predicate check and the condvar sleep, losing the only wakeup.
        let mut guard = self.completion.wait_future.lock().unwrap();
        loop {
            if !self.is_pending() {
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

    fn close_owned_streams(&self) {
        for stream_bits in [self.stdin_stream, self.stdout_stream, self.stderr_stream] {
            if stream_bits == 0 {
                continue;
            }
            let stream_ptr = ptr_from_bits(stream_bits);
            if !stream_ptr.is_null() {
                // The state owns these references until every worker joins.
                let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
                super::channels::stream_close_local(stream);
            }
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
        if !self.is_pending() {
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
        if !self.is_pending() {
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
        // Serialize the liveness check and signal with the sole Child reaper.
        // An unreaped child reserves its PID even if exit has already occurred.
        let _child = self.child.lock().unwrap();
        if !self.is_pending() {
            return Ok(());
        }
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
struct MoltProcessHandle {
    state: Arc<ProcessState>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for MoltProcessHandle {
    fn drop(&mut self) {
        self.state.request_kill_for_drop();
        self.state.close_owned_streams();
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
        if self.wait_finished() {
            return;
        }
        // Cancellation and terminal publication both mutate their predicates
        // while serialized by this mutex, so the condvar handoff is atomic
        // with respect to every wake source.
        let mut guard = self.process.completion.wait_future.lock().unwrap();
        loop {
            if self.wait_finished() {
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

    fn wait_finished(&self) -> bool {
        !self.process.is_pending() || self.cancelled.load(AtomicOrdering::Acquire)
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
impl ProcessState {
    fn poll_child_exit(&self) -> Option<()> {
        let mut child = self.child.lock().unwrap();
        if !self.is_pending() {
            return None;
        }
        let status = child.try_wait().ok().flatten()?;
        let code = exit_code_from_status(status);
        if trace_process_io() {
            eprintln!("molt_process_wait exit_code={code}");
        }
        let publication = self.publish_exit(code);
        if self.stdin_stream != 0 {
            let ptr = ptr_from_bits(self.stdin_stream);
            if !ptr.is_null() {
                // A terminal child cannot consume more input. Stop its writer
                // without truncating still-readable stdout/stderr queues.
                let stream = unsafe { &*(ptr as *mut MoltStream) };
                super::channels::stream_close_local(stream);
            }
        }
        publication
    }

    fn with_runtime_for_wake(&self, wake: impl FnOnce(&PyToken<'_>)) {
        if self.teardown_draining.load(AtomicOrdering::Acquire) {
            return;
        }
        assert_ne!(
            self.origin_runtime, 0,
            "process callback lost its runtime origin"
        );
        #[cfg(test)]
        PROCESS_WAKE_PAUSE.with(|slot| {
            if let Some((ready, proceed)) = slot.borrow_mut().take() {
                ready.send(()).unwrap();
                proceed.recv().unwrap();
            }
        });
        // Registry-first teardown retains this exact allocation through native
        // join. Set origin before try_ready's TLS/attachment initialization.
        crate::state::runtime_state::set_thread_runtime_state(
            std::ptr::with_exposed_provenance_mut(self.origin_runtime),
        );
        if let Some(execution) = crate::concurrency::RuntimeExecutionGuard::try_ready() {
            let py = execution.token();
            if !self.teardown_draining.load(AtomicOrdering::Acquire) {
                assert_eq!(
                    (runtime_state(&py) as *const RuntimeState).expose_provenance(),
                    self.origin_runtime,
                    "process callback resolved a different runtime"
                );
                wake(&py);
            }
            // WorkerEnd performs the canonical runtime/C TLS fixed point.
            // If cleanup panics, origin TLS remains and the retained join
            // disposition prevents the owning runtime from being destroyed.
            drop(execution);
        }
        crate::state::runtime_state::clear_thread_runtime_state();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
thread_local! {
    // One-shot failure at the external native spawn boundary. Already-started
    // worker handles and the actual child still follow ordinary settlement.
    static PROCESS_WORKER_SPAWN_FAILURE_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static PROCESS_WAKE_PAUSE: std::cell::RefCell<Option<(
        std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>
    )>> = const { std::cell::RefCell::new(None) };
}

#[cfg(not(target_arch = "wasm32"))]
fn process_wait_worker(state: Arc<ProcessState>) {
    while state.is_pending() {
        if state.kill_requested.load(AtomicOrdering::Acquire) {
            let _ = state.request_kill();
        }
        if state.poll_child_exit().is_some() && state.completion.has_wait_future() {
            state.with_runtime_for_wake(|py| {
                // Cancellation/free remains able to revoke this publication
                // while the native worker waits for exact-origin GIL custody.
                if let Some(future) = state.completion.claim_wait_future(py) {
                    let _ = wake_await_waiters(py, future.0);
                }
            });
        }
        if state.is_pending() {
            thread::sleep(Duration::from_millis(10));
        }
    }
    // Only the registry's completed join can remove this entry. Terminal child
    // publication alone says nothing about pipe workers or native TLS cleanup.
}

#[cfg(test)]
mod process_completion_state_tests {
    use super::*;

    fn slot(value: usize) -> PtrSlot {
        PtrSlot(value as *mut u8)
    }

    #[test]
    fn process_state_layout_metrics() {
        fn assert_send_sync<T: Send + Sync>() {}

        let task_bytes = std::mem::size_of::<ProcessTaskState>();
        let completion_bytes = std::mem::size_of::<ProcessCompletionState>();
        let process_bytes = std::mem::size_of::<ProcessState>();
        eprintln!(
            "process_state_layout task_bytes={task_bytes} completion_bytes={completion_bytes} process_bytes={process_bytes}"
        );
        assert_send_sync::<ProcessTaskState>();
        assert_eq!(task_bytes, 3 * std::mem::size_of::<usize>());
        assert_eq!(
            std::mem::size_of::<ProcessWaiterId>(),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn process_completion_transition_metrics() {
        const SAMPLES: usize = 100_000;
        let states: Vec<_> = std::iter::repeat_with(ProcessCompletionState::pending)
            .take(SAMPLES)
            .collect();
        let started = std::time::Instant::now();
        for state in &states {
            assert!(std::hint::black_box(state.publish_exit(0)).is_some());
        }
        let elapsed = started.elapsed();
        eprintln!(
            "process_completion_transition samples={SAMPLES} elapsed_ns={} ns_per_transition={:.2}",
            elapsed.as_nanos(),
            elapsed.as_nanos() as f64 / SAMPLES as f64
        );
    }

    #[test]
    fn wait_future_install_is_identity_safe_and_terminal_aware() {
        let completion = ProcessCompletionState::pending();
        let first = slot(1);
        let second = slot(2);

        assert!(matches!(
            completion.install_wait_future(first),
            ProcessWaitFutureInstall::Installed
        ));
        assert!(matches!(
            completion.install_wait_future(second),
            ProcessWaitFutureInstall::AlreadyInstalled(existing) if existing == first
        ));
        assert!(!completion.cancel_wait_future(ProcessWaiterId::from_slot(second)));
        assert_eq!(completion.wait_future(), Some(first));

        completion.publish_exit(17).expect("first terminal publish");
        assert_eq!(completion.wait_future(), Some(first));
        assert_eq!(completion.exit_code(), 17);
        assert!(completion.publish_exit(23).is_none());
        assert!(completion.publish_exit(PROCESS_EXIT_PENDING).is_none());
        assert!(matches!(
            completion.install_wait_future(second),
            ProcessWaitFutureInstall::Terminal(17)
        ));
        assert!(completion.cancel_wait_future(ProcessWaiterId::from_slot(first)));
        assert_eq!(completion.wait_future(), None);
    }

    #[test]
    fn cancellation_returns_the_registered_waiter_exactly_once() {
        let completion = ProcessCompletionState::pending();
        let waiter = slot(3);
        assert!(matches!(
            completion.install_wait_future(waiter),
            ProcessWaitFutureInstall::Installed
        ));
        let waiter_id = ProcessWaiterId::from_slot(waiter);
        assert!(completion.cancel_wait_future(waiter_id));
        assert!(!completion.cancel_wait_future(waiter_id));
        assert_eq!(completion.wait_future(), None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn install_and_publish_race_cannot_strand_a_waiter() {
        use std::sync::Barrier;

        for iteration in 0..64 {
            let completion = Arc::new(ProcessCompletionState::pending());
            let barrier = Arc::new(Barrier::new(3));
            let waiter_address = iteration + 16;
            let waiter = slot(waiter_address);

            let install_state = Arc::clone(&completion);
            let install_barrier = Arc::clone(&barrier);
            let installer = std::thread::spawn(move || {
                install_barrier.wait();
                install_state.install_wait_future(slot(waiter_address))
            });

            let publish_state = Arc::clone(&completion);
            let publish_barrier = Arc::clone(&barrier);
            let publisher = std::thread::spawn(move || {
                publish_barrier.wait();
                publish_state.publish_exit(31)
            });

            barrier.wait();
            let installation = installer.join().unwrap();
            publisher.join().unwrap().expect("publisher wins once");
            match installation {
                ProcessWaitFutureInstall::Installed => {
                    assert_eq!(completion.wait_future(), Some(waiter));
                }
                ProcessWaitFutureInstall::Terminal(31) => {
                    assert_eq!(completion.wait_future(), None);
                }
                ProcessWaitFutureInstall::AlreadyInstalled(_) => {
                    panic!("single installer cannot observe another waiter")
                }
                ProcessWaitFutureInstall::Terminal(other) => {
                    panic!("unexpected terminal code {other}")
                }
            }
            completion.cancel_wait_future(ProcessWaiterId::from_slot(waiter));
            assert_eq!(completion.wait_future(), None);
            assert_eq!(completion.exit_code(), 31);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn terminal_publication_does_not_take_cancellation_custody() {
        use std::sync::Barrier;

        for iteration in 0..64 {
            let completion = Arc::new(ProcessCompletionState::pending());
            let barrier = Arc::new(Barrier::new(3));
            let waiter = slot(iteration + 128);
            let waiter_id = ProcessWaiterId::from_slot(waiter);
            assert!(matches!(
                completion.install_wait_future(waiter),
                ProcessWaitFutureInstall::Installed
            ));

            let cancel_state = Arc::clone(&completion);
            let cancel_barrier = Arc::clone(&barrier);
            let canceller = std::thread::spawn(move || {
                cancel_barrier.wait();
                cancel_state.cancel_wait_future(waiter_id)
            });

            let publish_state = Arc::clone(&completion);
            let publish_barrier = Arc::clone(&barrier);
            let publisher = std::thread::spawn(move || {
                publish_barrier.wait();
                publish_state.publish_exit(47)
            });

            barrier.wait();
            let cancelled = canceller.join().unwrap();
            publisher.join().unwrap().expect("publisher wins once");
            assert!(
                cancelled,
                "terminal publication must not steal raw waiter custody"
            );
            assert_eq!(completion.wait_future(), None);
            assert_eq!(completion.exit_code(), 47);
        }
    }
}

// Real OS fixtures use this Cargo-owned image, not ambient shell utilities.
// The existing pipe-completion test is also the exact child entry; its normal
// invocation still runs the full semantic test and adds no gate inventory row.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_process_fixture {
    use super::*;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    const OWNER: &str = "async_rt::process::process_worker_lifetime_tests::normal_pipe_completion_preserves_bytes_and_reaps_all_handles";
    const MODE: &str = "MOLT_NATIVE_PROCESS_FIXTURE_MODE";
    pub(super) const PID_FILE: &str = "MOLT_NATIVE_PROCESS_FIXTURE_PID_FILE";
    const FRAME: &[u8] = b"MOLT_NATIVE_PROCESS_FIXTURE_V1\n";
    const PRELUDE: &[u8] = b"\nrunning 1 test\n";
    const MAX_OUTPUT: usize = 64 * 1024;

    pub(super) fn command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("current fixture image"));
        command
            .args([
                "--exact",
                OWNER,
                "--nocapture",
                "--test-threads=1",
                "--format=terse",
                "--color=never",
            ])
            // Replace any inherited child mode and remove the group parent's
            // handshake path before constructing an inert grandchild.
            .env(MODE, mode)
            .env_remove(PID_FILE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[cfg(unix)]
    struct OwnedChild(std::process::Child);

    #[cfg(unix)]
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            // On fixture failure retain the actual native handle through kill
            // and wait, rather than leaking a sleeper into a later test.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(unix)]
    static GROUP_TERMINATED: AtomicBool = AtomicBool::new(false);

    #[cfg(unix)]
    extern "C" fn retain_group_parent_until_reaped(_: libc::c_int) {
        GROUP_TERMINATED.store(true, AtomicOrdering::Relaxed);
    }

    pub(super) fn run_child_if_selected() {
        let Some(mode) = std::env::var_os(MODE) else {
            return;
        };
        // No runtime entry, test-state lock or process dispatcher is involved
        // in the fixture child. Unknown inherited selections fail closed.
        match mode.to_str().expect("fixture mode is UTF-8") {
            "exit" => {}
            "pipes" => {
                let mut stdout = std::io::stdout().lock();
                stdout.write_all(FRAME).unwrap();
                stdout.write_all(b"molt-stdout").unwrap();
                stdout.flush().unwrap();
                let mut stderr = std::io::stderr().lock();
                stderr.write_all(FRAME).unwrap();
                stderr.write_all(b"molt-stderr").unwrap();
                stderr.flush().unwrap();
            }
            "stdin-copy" => {
                let mut stdout = std::io::stdout().lock();
                stdout.write_all(FRAME).unwrap();
                let mut stdin = std::io::stdin().lock();
                let mut bytes = [0; 4096];
                loop {
                    let count = stdin.read(&mut bytes).unwrap();
                    if count == 0 {
                        break;
                    }
                    stdout.write_all(&bytes[..count]).unwrap();
                }
                stdout.flush().unwrap();
            }
            "sleep" | "group-sleep" => {
                if mode == "group-sleep" {
                    let path = std::env::var_os(PID_FILE).expect("grandchild readiness path");
                    // This publication occurs after exec has reset caught
                    // signal handlers. The owner must not signal the group
                    // while this child could still inherit its parent's handler.
                    std::fs::write(path, std::process::id().to_string()).unwrap();
                }
                thread::sleep(Duration::from_secs(30));
                std::process::exit(42); // A natural exit cannot prove a kill.
            }
            #[cfg(unix)]
            "group" => {
                use std::os::unix::process::ExitStatusExt;
                // The parent must reap the killed grandchild before exiting.
                // A caught handler resets to SIG_DFL on exec, so the real
                // grandchild remains subject to the owned group's SIGTERM.
                assert_ne!(
                    unsafe {
                        libc::signal(
                            libc::SIGTERM,
                            retain_group_parent_until_reaped as *const () as libc::sighandler_t,
                        )
                    },
                    libc::SIG_ERR
                );
                let path = std::env::var_os(PID_FILE).expect("group PID handshake path");
                let mut child = OwnedChild(
                    command("group-sleep")
                        .env(PID_FILE, path)
                        .spawn()
                        .expect("spawn group descendant"),
                );
                let mut term_deadline = None;
                let status = loop {
                    if let Some(status) = child.0.try_wait().expect("reap actual group descendant")
                    {
                        break status;
                    }
                    if GROUP_TERMINATED.load(AtomicOrdering::Relaxed) {
                        let deadline = *term_deadline
                            .get_or_insert_with(|| Instant::now() + Duration::from_secs(2));
                        assert!(
                            Instant::now() < deadline,
                            "group signal did not terminate its descendant"
                        );
                    }
                    thread::sleep(Duration::from_millis(1));
                };
                assert_eq!(
                    status.signal(),
                    Some(libc::SIGTERM),
                    "descendant was not terminated with its group"
                );
            }
            _ => panic!("unknown native process fixture mode"),
        }
        // Flushes above retain exact payload bytes; suppress libtest's success
        // epilogue so no harness output can be mistaken for fixture payload.
        std::process::exit(0);
    }

    pub(super) fn collect_to_eof(bits: u64) -> Vec<u8> {
        let stream = unsafe { &*(ptr_from_bits(bits) as *mut MoltStream) };
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut bytes = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "fixture pipe did not reach EOF");
            match stream
                .receiver
                .recv_timeout(remaining.min(Duration::from_millis(10)))
            {
                Ok(chunk) => {
                    super::super::channels::stream_release_queued_bytes(stream, chunk.len());
                    bytes.extend_from_slice(&chunk);
                    assert!(bytes.len() <= MAX_OUTPUT, "unbounded fixture output");
                }
                Err(_) if stream.closed.load(AtomicOrdering::Acquire) => {
                    // EOF is published after the last enqueue. Recheck the
                    // queue after observing that publication, even if the
                    // preceding timed receive raced with the final enqueue.
                    for chunk in stream.receiver.try_iter() {
                        super::super::channels::stream_release_queued_bytes(stream, chunk.len());
                        bytes.extend_from_slice(&chunk);
                        assert!(bytes.len() <= MAX_OUTPUT, "unbounded fixture output");
                    }
                    return bytes;
                }
                Err(_) => {}
            }
        }
    }

    pub(super) fn assert_live_pipe_bytes(bits: u64, expected: &[u8]) {
        // These two fixtures intentionally retain a writer after the child
        // exits, so EOF would contradict their premise. Still consume any
        // partition of the known write rather than assuming one read chunk.
        let stream = unsafe { &*(ptr_from_bits(bits) as *mut MoltStream) };
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut actual = Vec::new();
        while actual.len() < expected.len() {
            let chunk = stream
                .receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("retained pipe did not produce its exact bytes");
            super::super::channels::stream_release_queued_bytes(stream, chunk.len());
            actual.extend_from_slice(&chunk);
        }
        assert_eq!(actual, expected);
    }

    pub(super) fn assert_payload(bytes: &[u8], stdout: bool, expected: &[u8]) {
        // The selected terse, colorless, one-test libtest prelude is exact and
        // bounded. Neither searching for a marker nor a partial receive can
        // turn dropped, duplicated or trailing bytes into success.
        let mut complete = Vec::new();
        if stdout {
            complete.extend_from_slice(PRELUDE);
        }
        complete.extend_from_slice(FRAME);
        complete.extend_from_slice(expected);
        assert_eq!(bytes, complete);
    }

    pub(super) struct CleanupOnFailure<'a>(pub(super) &'a ProcessRegistry);

    impl Drop for CleanupOnFailure<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                // Preserve the failed assertion while giving every real child
                // and worker its existing registry-owned kill/join cleanup.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.0
                        .drain_with_deadlines(Duration::from_secs(5), Duration::from_secs(2));
                }));
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod process_wait_state_tests {
    use super::*;
    use std::sync::mpsc;

    fn inert_child() -> std::process::Child {
        let mut child = native_process_fixture::command("exit")
            .spawn()
            .expect("spawn inert child");
        // These completion-state tests publish their own terminal predicate;
        // they have no process waiter worker, so retain/reap the native child
        // here rather than letting Child::drop leave a zombie behind.
        assert!(child.wait().expect("reap inert child").success());
        child
    }

    fn process_state() -> Arc<ProcessState> {
        let child = inert_child();
        Arc::new(ProcessState {
            registry_id: 0,
            origin_runtime: 0,
            pid: child.id(),
            child: Mutex::new(child),
            #[cfg(unix)]
            owned_process_group: None,
            completion: ProcessCompletionState::pending(),
            kill_requested: AtomicBool::new(false),
            teardown_draining: AtomicBool::new(false),
            streams_released: AtomicBool::new(false),
            stdin_stream: 0,
            stdout_stream: 0,
            stderr_stream: 0,
            condvar: Condvar::new(),
        })
    }

    #[test]
    fn native_blocking_wait_observes_cancellation_predicate() {
        let state = process_state();
        let waiter = PtrSlot(std::ptr::dangling_mut::<u8>());
        assert!(matches!(
            state.install_wait_future(waiter),
            ProcessWaitFutureInstall::Installed
        ));
        let task = Arc::new(ProcessTaskState {
            process: Arc::clone(&state),
            waiter_id: ProcessWaiterId::from_slot(waiter),
            cancelled: AtomicBool::new(false),
        });
        let blocking_task = Arc::clone(&task);
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            blocking_task.wait_blocking(None);
            tx.send(()).unwrap();
        });

        std::thread::sleep(Duration::from_millis(20));
        assert!(task.cancel_wait());
        assert!(!task.cancel_wait());
        rx.recv_timeout(Duration::from_secs(1))
            .expect("cancelled process wait must wake");
        thread.join().unwrap();
        assert!(task.cancelled.load(AtomicOrdering::Acquire));
        assert_eq!(state.wait_future(), None);
    }

    #[test]
    fn native_terminal_publish_wakes_blocking_wait_exactly_once() {
        let state = process_state();
        let waiter = PtrSlot(2usize as *mut u8);
        assert!(matches!(
            state.install_wait_future(waiter),
            ProcessWaitFutureInstall::Installed
        ));
        let task = Arc::new(ProcessTaskState {
            process: Arc::clone(&state),
            waiter_id: ProcessWaiterId::from_slot(waiter),
            cancelled: AtomicBool::new(false),
        });
        let blocking_task = Arc::clone(&task);
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            blocking_task.wait_blocking(None);
            tx.send(()).unwrap();
        });

        std::thread::sleep(Duration::from_millis(20));
        state.publish_exit(9).expect("first terminal publish");
        assert_eq!(state.wait_future(), Some(waiter));
        assert!(state.publish_exit(10).is_none());
        rx.recv_timeout(Duration::from_secs(1))
            .expect("completed process wait must wake");
        thread.join().unwrap();
        assert_eq!(state.exit_code(), 9);
    }
}

#[cfg(all(test, unix, not(target_arch = "wasm32")))]
mod process_registry_tests {

    #[test]
    fn registry_settlement_deadlines_ignore_late_environment_changes() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let keys = [
            super::PROCESS_TEARDOWN_TERM_GRACE_MS_ENV,
            super::PROCESS_TEARDOWN_JOIN_TIMEOUT_MS_ENV,
        ];
        let saved = keys.map(|key| (key, std::env::var_os(key)));
        unsafe {
            std::env::set_var(keys[0], "3");
            std::env::set_var(keys[1], "7");
        }
        let registry = super::ProcessRegistry::new();
        unsafe {
            std::env::set_var(keys[0], "999999999");
            std::env::set_var(keys[1], "999999999");
        }
        let inherited = registry.new_isolate();
        let observed = (inherited.term_grace, inherited.join_timeout);
        for (key, value) in saved {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        assert_eq!(
            observed,
            (
                std::time::Duration::from_millis(3),
                std::time::Duration::from_millis(7)
            )
        );
    }
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn registered_child(cmd: &mut Command, registry: &ProcessRegistry) -> Arc<ProcessState> {
        let owns_process_group = configure_unix_owned_process_group(cmd, false, None);
        assert!(owns_process_group);
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let py = execution.token();
        let stdio = configure_native_stdio(
            &py,
            cmd,
            stdio::PROCESS_STDIO_DEVNULL,
            stdio::PROCESS_STDIO_DEVNULL,
            stdio::PROCESS_STDIO_DEVNULL,
        )
        .unwrap();
        registry
            .spawn(&py, cmd, stdio, owns_process_group)
            .expect("spawn test child")
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
            if let Ok(raw) = fs::read_to_string(path)
                && let Ok(pid) = raw.trim().parse::<i32>()
            {
                return pid;
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
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let registry = ProcessRegistry::new();
        let _cleanup = native_process_fixture::CleanupOnFailure(&registry);
        let mut cmd = native_process_fixture::command("sleep");
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
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let registry = ProcessRegistry::new();
        let _cleanup = native_process_fixture::CleanupOnFailure(&registry);
        let pid_path = unique_temp_path("process-group");
        let mut cmd = native_process_fixture::command("group");
        cmd.env(native_process_fixture::PID_FILE, &pid_path);
        let state = registered_child(&mut cmd, &registry);
        let parent_pid = state.pid as i32;
        let descendant_pid = wait_for_pid_file(&pid_path);
        // The fixture parent retains its native handle until its real child
        // is reaped. Give the independent two-second child observation bound
        // room before the registry's emergency SIGKILL deadline under ptrace.
        registry.drain_with_deadlines(Duration::from_secs(5), Duration::from_secs(2));
        let _ = fs::remove_file(pid_path);
        assert_eq!(registry.live_count(), 0);
        assert!(!state.is_pending());
        assert_eq!(
            state.exit_code(),
            0,
            "fixture parent must reap the group-killed descendant"
        );
        assert_process_exits(parent_pid);
        assert_process_exits(descendant_pid);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod process_worker_lifetime_tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;

    fn inert_command() -> std::process::Command {
        native_process_fixture::command("exit")
    }

    fn reader_state(
        py: &PyToken<'_>,
        registry: &ProcessRegistry,
        reader: os_pipe::PipeReader,
    ) -> Arc<ProcessState> {
        let child = inert_command().spawn().unwrap();
        let stream = super::super::channels::stream_new_with_byte_budget(0, 64 * 1024);
        let mut stdio = stdio::NativeProcessStdio {
            stdin_stream: 0,
            stdout_stream: stream,
            stderr_stream: 0,
            merged_stdout_reader: Some(reader),
        };
        let mut inner = registry.inner.lock().unwrap();
        assert!(!inner.closed);
        let id = inner.next_id;
        inner.next_id += 1;
        let state = Arc::new(ProcessState {
            registry_id: id,
            origin_runtime: (runtime_state(py) as *const RuntimeState).expose_provenance(),
            pid: child.id(),
            child: Mutex::new(child),
            #[cfg(unix)]
            owned_process_group: None,
            completion: ProcessCompletionState::pending(),
            kill_requested: AtomicBool::new(false),
            teardown_draining: AtomicBool::new(false),
            streams_released: AtomicBool::new(false),
            stdin_stream: 0,
            stdout_stream: stream,
            stderr_stream: 0,
            condvar: Condvar::new(),
        });
        let entry = inner
            .entries
            .entry(id)
            .or_insert_with(|| ProcessRegistryEntry {
                state: Arc::clone(&state),
                workers: Vec::with_capacity(4),
            });
        let worker = Arc::clone(&state);
        entry
            .spawn_worker("molt-process-wait", move || process_wait_worker(worker))
            .unwrap();
        attach_process_stdio(&mut state.child.lock().unwrap(), &mut stdio, &state, entry).unwrap();
        state
    }

    #[test]
    fn inherited_pipe_timeout_retains_exact_worker_and_stream_until_join() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let registry = ProcessRegistry::new();
        let (reader, mut inherited_writer) = os_pipe::pipe().unwrap();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let state = reader_state(&execution.token(), &registry, reader);
        let stream_bits = state.stdout_stream;
        let stream_ptr = ptr_from_bits(stream_bits);
        // Observe real pipe I/O before retaining the write endpoint across the
        // direct child's exit. This is not an is_finished/sleep timing oracle.
        inherited_writer.write_all(b"pipe-worker-is-live").unwrap();
        native_process_fixture::assert_live_pipe_bytes(stream_bits, b"pipe-worker-is-live");
        let state_weak = Arc::downgrade(&state);
        drop(execution);
        let gil = GilGuard::new();
        let rejected = crate::test_support::catch_expected_unwind(|| {
            registry.drain_with_deadlines(Duration::ZERO, Duration::from_millis(10));
        });
        assert!(rejected.is_err());
        assert_eq!(registry.live_count(), 1);
        assert_eq!(ptr_from_bits(stream_bits), stream_ptr);
        assert!(!state.streams_released.load(AtomicOrdering::Acquire));
        {
            let inner = registry.inner.lock().unwrap();
            let retained = &inner.entries[&state.registry_id];
            assert!(Arc::ptr_eq(&retained.state, &state));
            assert!(
                retained
                    .workers
                    .iter()
                    .any(|worker| worker.handle.is_some())
            );
        }
        // EOF makes the same retained pipe worker finish; no replacement
        // worker, synthetic close outcome, or runtime-entry fallback is used.
        drop(inherited_writer);
        registry.drain_with_deadlines(Duration::ZERO, Duration::from_secs(2));
        assert_eq!(registry.live_count(), 0);
        drop(state);
        assert!(state_weak.upgrade().is_none());
        assert!(ptr_from_bits(stream_bits).is_null());
        drop(gil);
    }

    #[test]
    fn terminal_process_waiter_cancellation_prevents_dispatch_after_gil_claim() {
        use super::super::scheduler::{
            await_waiter_clear, await_waiter_register, task_waiting_on_future,
        };
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let py = execution.token();
        let registry = ProcessRegistry::new();
        // Retain a real child and real waiter worker in the existing registry.
        // Install its future before starting the waiter, so even an immediate
        // child exit cannot bypass the controlled pre-GIL interval.
        let child = inert_command().spawn().unwrap();
        let state = Arc::new(ProcessState {
            registry_id: 1,
            origin_runtime: runtime_state(&py).identity(),
            pid: child.id(),
            child: Mutex::new(child),
            #[cfg(unix)]
            owned_process_group: None,
            completion: ProcessCompletionState::pending(),
            kill_requested: AtomicBool::new(false),
            teardown_draining: AtomicBool::new(false),
            streams_released: AtomicBool::new(false),
            stdin_stream: 0,
            stdout_stream: 0,
            stderr_stream: 0,
            condvar: Condvar::new(),
        });
        let handle = opaque_handle_bits(Box::into_raw(Box::new(MoltProcessHandle {
            state: Arc::clone(&state),
        })) as *mut u8);
        let original = unsafe { molt_process_wait_future(handle) };
        let original_ptr = resolve_obj_ptr(original).unwrap();
        let original_task = process_task_state(&py, original_ptr).unwrap();
        let observer = molt_future_new(0, 0);
        let observer_ptr = resolve_obj_ptr(observer).unwrap();
        await_waiter_register(&py, observer_ptr, original_ptr);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (proceed_tx, proceed_rx) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        {
            let mut inner = registry.inner.lock().unwrap();
            inner.next_id = 2;
            let entry = inner
                .entries
                .entry(1)
                .or_insert_with(|| ProcessRegistryEntry {
                    state: Arc::clone(&state),
                    workers: Vec::new(),
                });
            entry
                .spawn_worker("molt-process-revocable-wait-test", move || {
                    PROCESS_WAKE_PAUSE
                        .with(|slot| *slot.borrow_mut() = Some((ready_tx, proceed_rx)));
                    process_wait_worker(worker_state);
                })
                .unwrap();
        }
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            state.exit_code(),
            0,
            "pause must follow actual terminal publication"
        );
        // Cancellation is the real ProcessTaskState operation, not a forged
        // map key. The old pre-GIL take makes this assertion fail. Keep the
        // original future and its real observer edge alive so an incorrectly
        // copied pre-GIL waiter also fails deterministically after dispatch.
        assert!(
            original_task.cancel_wait(),
            "exit publication stole cancellation custody"
        );
        assert_eq!(state.wait_future(), None);
        assert_eq!(
            task_waiting_on_future(&py, observer_ptr),
            Some(original_ptr),
            "cancellation must leave the observer edge available to detect stale dispatch"
        );
        proceed_tx.send(()).unwrap();
        {
            let _release = GilReleaseGuard::suspend();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                registry.join_finished();
                if registry.inner.lock().unwrap().entries[&1]
                    .workers
                    .iter()
                    .all(|worker| worker.joined)
                {
                    break;
                }
                assert!(Instant::now() < deadline, "paused waiter did not finish");
                thread::yield_now();
            }
        }
        assert!(
            registry.inner.lock().unwrap().entries[&1]
                .workers
                .iter()
                .all(|worker| worker.panic.is_none())
        );
        assert_eq!(
            task_waiting_on_future(&py, observer_ptr),
            Some(original_ptr),
            "cancelled process completion consumed the observer's await edge"
        );
        assert_eq!(state.wait_future(), None);
        await_waiter_clear(&py, observer_ptr);
        unsafe {
            assert_eq!((*header_from_obj_ptr(original_ptr)).ref_count_snapshot(), 1);
        }
        dec_ref_bits(&py, original);
        assert!(process_task_state(&py, original_ptr).is_none());
        dec_ref_bits(&py, observer);
        unsafe {
            molt_process_drop(handle);
        }
        registry.reap_finished();
        assert_eq!(registry.live_count(), 0);
    }

    #[test]
    fn process_wake_rechecks_teardown_after_waiting_for_execution_custody() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let registry = ProcessRegistry::new();
        let mut command = inert_command();
        // The self-image fixture prints a libtest prelude. Keep its unused
        // streams private; INHERIT would overwrite the builder's null defaults.
        let stdio = configure_native_stdio(
            &execution.token(),
            &mut command,
            stdio::PROCESS_STDIO_DEVNULL,
            stdio::PROCESS_STDIO_DEVNULL,
            stdio::PROCESS_STDIO_DEVNULL,
        )
        .unwrap();
        let state = registry
            .spawn(&execution.token(), &mut command, stdio, false)
            .unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (proceed_tx, proceed_rx) = mpsc::channel();
        let called = Arc::new(AtomicBool::new(false));
        let worker_called = Arc::clone(&called);
        let worker_state = Arc::clone(&state);
        {
            let mut inner = registry.inner.lock().unwrap();
            inner
                .entries
                .get_mut(&state.registry_id)
                .unwrap()
                .spawn_worker("molt-process-wake-test", move || {
                    PROCESS_WAKE_PAUSE
                        .with(|slot| *slot.borrow_mut() = Some((ready_tx, proceed_rx)));
                    worker_state.with_runtime_for_wake(|_| {
                        worker_called.store(true, AtomicOrdering::Release);
                    });
                })
                .unwrap();
        }
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // The initial draining check has already admitted the callback. The
        // origin is still Ready, so only the post-custody check can refuse it.
        state.teardown_draining.store(true, AtomicOrdering::Release);
        proceed_tx.send(()).unwrap();
        registry.drain_with_deadlines(Duration::ZERO, Duration::from_secs(2));
        assert!(!called.load(AtomicOrdering::Acquire));
        assert_eq!(registry.live_count(), 0);
        drop(state);
        drop(execution);
    }

    #[test]
    fn normal_pipe_completion_preserves_bytes_and_reaps_all_handles() {
        native_process_fixture::run_child_if_selected();
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let registry = ProcessRegistry::new();
        let _cleanup = native_process_fixture::CleanupOnFailure(&registry);
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let mut command = native_process_fixture::command("pipes");
        let stdio = configure_native_stdio(
            &execution.token(),
            &mut command,
            stdio::PROCESS_STDIO_PIPE,
            stdio::PROCESS_STDIO_PIPE,
            stdio::PROCESS_STDIO_PIPE,
        )
        .unwrap();
        let state = registry
            .spawn(&execution.token(), &mut command, stdio, false)
            .unwrap();
        let stdout = native_process_fixture::collect_to_eof(state.stdout_stream);
        let stderr = native_process_fixture::collect_to_eof(state.stderr_stream);
        native_process_fixture::assert_payload(&stdout, true, b"molt-stdout");
        native_process_fixture::assert_payload(&stderr, false, b"molt-stderr");
        assert!(state.wait_for_exit(Duration::from_secs(2)));
        registry.drain_with_deadlines(Duration::ZERO, Duration::from_secs(2));
        assert_eq!(registry.live_count(), 0);
        assert_eq!(state.exit_code(), 0);
        drop(state);
        drop(execution);
    }

    #[test]
    fn ordinary_stdin_close_drains_bytes_queued_before_worker_start() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let registry = ProcessRegistry::new();
        let _cleanup = native_process_fixture::CleanupOnFailure(&registry);
        let mut command = native_process_fixture::command("stdin-copy");
        let stdio = configure_native_stdio(
            &execution.token(),
            &mut command,
            stdio::PROCESS_STDIO_PIPE,
            stdio::PROCESS_STDIO_PIPE,
            stdio::PROCESS_STDIO_DEVNULL,
        )
        .unwrap();
        let expected = b"queued-before-close\nsecond-line\n";
        let input = unsafe { &*(ptr_from_bits(stdio.stdin_stream) as *mut MoltStream) };
        assert!(super::super::channels::stream_enqueue_bytes_blocking(
            input,
            expected.to_vec()
        ));
        // Close before native worker creation, so scheduling cannot hide an
        // implementation which discards queued bytes on ordinary close.
        super::super::channels::stream_close_local(input);
        let state = registry
            .spawn(&execution.token(), &mut command, stdio, false)
            .unwrap();
        let actual = native_process_fixture::collect_to_eof(state.stdout_stream);
        native_process_fixture::assert_payload(&actual, true, expected);
        assert!(state.wait_for_exit(Duration::from_secs(2)));
        assert_eq!(state.exit_code(), 0);
        registry.drain_with_deadlines(Duration::ZERO, Duration::from_secs(2));
        assert_eq!(registry.live_count(), 0);
    }

    #[test]
    fn closed_registry_refuses_new_process_before_os_spawn() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let registry = ProcessRegistry::new();
        registry.drain_with_deadlines(Duration::ZERO, Duration::ZERO);
        let mut command =
            std::process::Command::new("molt-does-not-exist-process-admission-oracle");
        let stdio = configure_native_stdio(&execution.token(), &mut command, 0, 0, 0).unwrap();
        let error = match registry.spawn(&execution.token(), &mut command, stdio, false) {
            Ok(_) => panic!("closed registry spawned a child"),
            Err(error) => error,
        };
        assert_eq!(error.to_string(), "runtime process admission is closed");
        assert_eq!(registry.live_count(), 0);
        assert_eq!(registry.inner.lock().unwrap().next_id, 1);
    }

    #[test]
    fn process_spawn_error_releases_unpublished_pipe_streams() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let registry = ProcessRegistry::new();
        let mut command =
            std::process::Command::new("molt-does-not-exist-process-admission-oracle");
        let stdio = configure_native_stdio(&execution.token(), &mut command, 1, 1, 1).unwrap();
        let streams = [stdio.stdin_stream, stdio.stdout_stream, stdio.stderr_stream];
        assert!(
            registry
                .spawn(&execution.token(), &mut command, stdio, false)
                .is_err()
        );
        assert_eq!(registry.live_count(), 0);
        for stream in streams {
            assert!(ptr_from_bits(stream).is_null());
        }
    }

    #[test]
    fn partial_process_worker_start_failure_joins_before_stream_release() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        // Waiter, stdin writer, stdout reader, then stderr reader. Every
        // preceding native spawn is real; only the named failing call is fake.
        for started_workers in 0..4 {
            let registry = ProcessRegistry::new();
            let mut command = inert_command();
            let stdio = configure_native_stdio(
                &execution.token(),
                &mut command,
                stdio::PROCESS_STDIO_PIPE,
                stdio::PROCESS_STDIO_PIPE,
                stdio::PROCESS_STDIO_PIPE,
            )
            .unwrap();
            let streams = [stdio.stdin_stream, stdio.stdout_stream, stdio.stderr_stream];
            PROCESS_WORKER_SPAWN_FAILURE_AFTER
                .with(|remaining| remaining.set(Some(started_workers)));
            let error = match registry.spawn(&execution.token(), &mut command, stdio, false) {
                Ok(_) => panic!("injected worker-start failure was not reached"),
                Err(error) => error,
            };
            assert_eq!(error.to_string(), "injected process worker spawn failure");
            assert!(PROCESS_WORKER_SPAWN_FAILURE_AFTER.with(|remaining| remaining.get().is_none()));
            assert_eq!(
                registry.live_count(),
                0,
                "already-started worker was not joined"
            );
            assert_eq!(
                registry.inner.lock().unwrap().next_id,
                2,
                "no actual child was admitted"
            );
            for stream in streams {
                assert!(
                    ptr_from_bits(stream).is_null(),
                    "unpublished stream was retained after complete closure"
                );
            }
        }
    }

    #[test]
    fn panicking_worker_disposition_is_retained_and_cannot_close_origin() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let registry = ProcessRegistry::new();
        let mut command = inert_command();
        // The self-image fixture prints a libtest prelude. Keep its unused
        // streams private; INHERIT would overwrite the builder's null defaults.
        let stdio = configure_native_stdio(
            &execution.token(),
            &mut command,
            stdio::PROCESS_STDIO_DEVNULL,
            stdio::PROCESS_STDIO_DEVNULL,
            stdio::PROCESS_STDIO_DEVNULL,
        )
        .unwrap();
        let state = registry
            .spawn(&execution.token(), &mut command, stdio, false)
            .unwrap();
        {
            let mut inner = registry.inner.lock().unwrap();
            inner
                .entries
                .get_mut(&state.registry_id)
                .unwrap()
                .spawn_worker("molt-process-panic-test", || {
                    crate::test_support::with_expected_panic(|| {
                        panic!("retained process worker failure")
                    });
                })
                .unwrap();
        }
        assert!(
            crate::test_support::catch_expected_unwind(|| {
                registry.drain_with_deadlines(Duration::ZERO, Duration::from_secs(2));
            })
            .is_err()
        );
        assert_eq!(registry.live_count(), 1);
        let inner = registry.inner.lock().unwrap();
        let entry = &inner.entries[&state.registry_id];
        assert!(entry.workers.iter().all(|worker| worker.handle.is_none()));
        assert!(
            entry
                .workers
                .iter()
                .any(|worker| worker.panic.as_ref().is_some_and(|panic| {
                    panic.downcast_ref::<&str>() == Some(&"retained process worker failure")
                }))
        );
        assert!(!entry.is_closed());
        drop(inner);
        // This isolated registry fixture injected a plain Rust panic and has
        // observed every actual join. The production origin never discards
        // this retained failure; terminal RuntimeState coverage owns that gate.
        drop(registry);
        drop(state);
    }

    pub(crate) struct ProcessServiceTimeoutFixture {
        origin: usize,
        state: Arc<ProcessState>,
        inherited_writer: Option<os_pipe::PipeWriter>,
    }

    impl ProcessServiceTimeoutFixture {
        // The terminal child owner first verifies its original RuntimeState
        // storage is retained. Failed teardown intentionally leaves this same
        // allocation live; none of these observations enter a runtime.
        fn registry(&self) -> &ProcessRegistry {
            let origin: *const RuntimeState = std::ptr::with_exposed_provenance(self.origin);
            &unsafe { &*origin }.process_registry
        }

        pub(crate) fn assert_retained(&self) {
            let inner = self.registry().inner.lock().unwrap();
            assert!(inner.closed);
            let entry = inner
                .entries
                .get(&self.state.registry_id)
                .expect("retained process entry");
            assert!(Arc::ptr_eq(&entry.state, &self.state));
            assert!(entry.workers.iter().any(|worker| worker.handle.is_some()));
            assert!(!self.state.streams_released.load(AtomicOrdering::Acquire));
            assert!(!ptr_from_bits(self.state.stdout_stream).is_null());
        }

        pub(crate) fn release_and_assert_closed(mut self) {
            drop(self.inherited_writer.take());
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                // Joining is pure native custody. Do not call the normal
                // reaper, drop streams, or fabricate admission in Failed.
                self.registry().join_finished();
                let joined = {
                    let inner = self.registry().inner.lock().unwrap();
                    let entry = &inner.entries[&self.state.registry_id];
                    assert!(entry.workers.iter().all(|worker| worker.panic.is_none()));
                    entry.workers.iter().all(|worker| worker.joined)
                };
                if joined {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "released pipe worker did not finish"
                );
                thread::sleep(Duration::from_millis(1));
            }
            assert!(!self.state.streams_released.load(AtomicOrdering::Acquire));
            assert!(!ptr_from_bits(self.state.stdout_stream).is_null());
            assert_eq!(self.registry().live_count(), 1);
        }
    }

    pub(crate) fn install_process_service_timeout_fixture(
        py: &PyToken<'_>,
    ) -> ProcessServiceTimeoutFixture {
        let origin = (runtime_state(py) as *const RuntimeState).expose_provenance();
        let registry = &runtime_state(py).process_registry;
        assert_eq!(registry.live_count(), 0);
        let (reader, mut writer) = os_pipe::pipe().unwrap();
        let state = reader_state(py, registry, reader);
        writer.write_all(b"retained-origin-pipe").unwrap();
        native_process_fixture::assert_live_pipe_bytes(
            state.stdout_stream,
            b"retained-origin-pipe",
        );
        assert!(state.wait_for_exit(Duration::from_secs(2)));
        ProcessServiceTimeoutFixture {
            origin,
            state,
            inherited_writer: Some(writer),
        }
    }

    // Called by the existing native-lifetime child family while its selected
    // primary or isolate RuntimeState is live; no second child runner/registry.
    pub(crate) fn assert_origin_wake(py: &PyToken<'_>) {
        let origin = (runtime_state(py) as *const RuntimeState).expose_provenance();
        let registry = &runtime_state(py).process_registry;
        let mut command = inert_command();
        // The self-image fixture prints a libtest prelude. Keep its unused
        // streams private; INHERIT would overwrite the builder's null defaults.
        let stdio = configure_native_stdio(
            py,
            &mut command,
            stdio::PROCESS_STDIO_DEVNULL,
            stdio::PROCESS_STDIO_DEVNULL,
            stdio::PROCESS_STDIO_DEVNULL,
        )
        .unwrap();
        let state = registry.spawn(py, &mut command, stdio, false).unwrap();
        let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_observed = Arc::clone(&observed);
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            worker_state.with_runtime_for_wake(|py| {
                worker_observed.store(
                    (runtime_state(py) as *const RuntimeState).expose_provenance(),
                    AtomicOrdering::Release,
                );
            });
        });
        {
            let _release = GilReleaseGuard::suspend();
            worker.join().unwrap();
        }
        assert_eq!(observed.load(AtomicOrdering::Acquire), origin);
        assert!(state.wait_for_exit(Duration::from_secs(2)));
        assert!(registry.settle_workers(
            std::slice::from_ref(&state),
            Duration::ZERO,
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn process_callback_uses_captured_runtime_and_cleans_worker_attachment() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let execution = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        assert_origin_wake(&execution.token());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) use process_worker_lifetime_tests::{
    assert_origin_wake as assert_process_origin_wake, install_process_service_timeout_fixture,
};
