use super::{await_waiters_take, wake_task_ptr};
#[cfg(not(target_arch = "wasm32"))]
use super::process_task_state;
use crate::*;

#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::process::Stdio;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

// --- Process ---

#[cfg(not(target_arch = "wasm32"))]
const PROCESS_STDIO_INHERIT: i32 = 0;
#[cfg(not(target_arch = "wasm32"))]
const PROCESS_STDIO_PIPE: i32 = 1;
#[cfg(not(target_arch = "wasm32"))]
const PROCESS_STDIO_DEVNULL: i32 = 2;

#[cfg(not(target_arch = "wasm32"))]
fn process_stdio_mode(_py: &PyToken<'_>, bits: u64, name: &str) -> i32 {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return PROCESS_STDIO_INHERIT;
    }
    match to_i64(obj) {
        Some(val) => {
            let val = val as i32;
            match val {
                PROCESS_STDIO_INHERIT | PROCESS_STDIO_PIPE | PROCESS_STDIO_DEVNULL => val,
                _ => {
                    return raise_exception::<_>(_py, "ValueError", &format!("invalid {name} mode"))
                }
            }
        }
        None => {
            return raise_exception::<_>(_py, "TypeError", &format!("{name} must be int or None"));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_process_reader(mut reader: impl Read + Send + 'static, stream_bits: u64) {
    unsafe {
        let _ = molt_stream_clone(stream_bits);
    }
    thread::spawn(move || {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        let sender = stream.sender.clone();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = buf[..n].to_vec();
                    let _ = sender.send(bytes);
                }
                Err(_) => break,
            }
        }
        stream.closed.store(true, AtomicOrdering::Release);
        unsafe {
            molt_stream_drop(stream_bits);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_process_writer(mut writer: impl Write + Send + 'static, stream_bits: u64) {
    unsafe {
        let _ = molt_stream_clone(stream_bits);
    }
    thread::spawn(move || {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        let receiver = stream.receiver.clone();
        loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(bytes) => {
                    if bytes.is_empty() {
                        continue;
                    }
                    let _ = writer.write_all(&bytes);
                    let _ = writer.flush();
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if stream.closed.load(AtomicOrdering::Acquire) {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        stream.closed.store(true, AtomicOrdering::Release);
        unsafe {
            molt_stream_drop(stream_bits);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_spawn(
    args_bits: u64,
    env_bits: u64,
    cwd_bits: u64,
    stdin_bits: u64,
    stdout_bits: u64,
    stderr_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
        let mut cmd = std::process::Command::new(&args[0]);
        if args.len() > 1 {
            cmd.args(&args[1..]);
        }
        let env_entries = match env_from_bits(_py, env_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if let Some(env_entries) = env_entries {
            cmd.env_clear();
            for (key, value) in env_entries {
                cmd.env(key, value);
            }
        }
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

        let stdin_stream = if stdin_mode == PROCESS_STDIO_PIPE {
            molt_stream_new(0)
        } else {
            0
        };
        let stdout_stream = if stdout_mode == PROCESS_STDIO_PIPE {
            molt_stream_new(0)
        } else {
            0
        };
        let stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
            molt_stream_new(0)
        } else {
            0
        };

        match stdin_mode {
            PROCESS_STDIO_PIPE => cmd.stdin(Stdio::piped()),
            PROCESS_STDIO_DEVNULL => cmd.stdin(Stdio::null()),
            _ => cmd.stdin(Stdio::inherit()),
        };
        match stdout_mode {
            PROCESS_STDIO_PIPE => cmd.stdout(Stdio::piped()),
            PROCESS_STDIO_DEVNULL => cmd.stdout(Stdio::null()),
            _ => cmd.stdout(Stdio::inherit()),
        };
        match stderr_mode {
            PROCESS_STDIO_PIPE => cmd.stderr(Stdio::piped()),
            PROCESS_STDIO_DEVNULL => cmd.stderr(Stdio::null()),
            _ => cmd.stderr(Stdio::inherit()),
        };

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                if stdin_stream != 0 {
                    molt_stream_drop(stdin_stream);
                }
                if stdout_stream != 0 {
                    molt_stream_drop(stdout_stream);
                }
                if stderr_stream != 0 {
                    molt_stream_drop(stderr_stream);
                }
                return raise_os_error::<u64>(_py, err, "spawn");
            }
        };

        if stdin_stream != 0 {
            if let Some(stdin) = child.stdin.take() {
                spawn_process_writer(stdin, stdin_stream);
            }
        }
        if stdout_stream != 0 {
            if let Some(stdout) = child.stdout.take() {
                spawn_process_reader(stdout, stdout_stream);
            }
        }
        if stderr_stream != 0 {
            if let Some(stderr) = child.stderr.take() {
                spawn_process_reader(stderr, stderr_stream);
            }
        }

        let pid = child.id();
        let state = Arc::new(ProcessState {
            child: Mutex::new(child),
            pid,
            exit_code: AtomicI32::new(PROCESS_EXIT_PENDING),
            wait_future: Mutex::new(None),
            stdin_stream,
            stdout_stream,
            stderr_stream,
            wait_lock: Mutex::new(()),
            condvar: Condvar::new(),
        });
        let worker_state = Arc::clone(&state);
        thread::spawn(move || process_wait_worker(worker_state));
        let handle = Box::new(MoltProcessHandle { state });
        bits_from_ptr(Box::into_raw(handle) as *mut u8)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_wait_future(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_poll(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
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

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_process_spawn(
    _args_bits: u64,
    _env_bits: u64,
    _cwd_bits: u64,
    _stdin_bits: u64,
    _stdout_bits: u64,
    _stderr_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "process spawn unsupported on wasm")
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_process_wait_future(_proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "process wait unsupported on wasm")
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_process_poll(_obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, { pending_bits_i64() })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_pid(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let proc_ptr = ptr_from_bits(proc_bits);
        if proc_ptr.is_null() {
            return MoltObject::from_int(0).bits();
        }
        let handle = &*(proc_ptr as *mut MoltProcessHandle);
        MoltObject::from_int(handle.state.pid as i64).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_returncode(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_kill(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let proc_ptr = ptr_from_bits(proc_bits);
        if proc_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let handle = &*(proc_ptr as *mut MoltProcessHandle);
        if handle.state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            return MoltObject::none().bits();
        }
        let mut guard = handle.state.child.lock().unwrap();
        if let Err(err) = guard.kill() {
            return raise_os_error::<u64>(_py, err, "kill");
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_terminate(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let proc_ptr = ptr_from_bits(proc_bits);
        if proc_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let handle = &*(proc_ptr as *mut MoltProcessHandle);
        if handle.state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
            return MoltObject::none().bits();
        }
        #[cfg(unix)]
        {
            let pid = handle.state.pid as i32;
            let res = libc::kill(pid, libc::SIGTERM);
            if res != 0 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "terminate");
            }
            return MoltObject::none().bits();
        }
        #[cfg(not(unix))]
        {
            let mut guard = handle.state.child.lock().unwrap();
            if let Err(err) = guard.kill() {
                return raise_os_error::<u64>(_py, err, "terminate");
            }
            MoltObject::none().bits()
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_stdin(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_stdout(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_stderr(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_process_drop(proc_bits: u64) {
    crate::with_gil_entry!(_py, {
        let proc_ptr = ptr_from_bits(proc_bits);
        if proc_ptr.is_null() {
            return;
        }
        release_ptr(proc_ptr);
        drop(Box::from_raw(proc_ptr as *mut MoltProcessHandle));
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ProcessState {
    child: Mutex<std::process::Child>,
    pub(crate) pid: u32,
    pub(crate) exit_code: AtomicI32,
    pub(crate) wait_future: Mutex<Option<PtrSlot>>,
    stdin_stream: u64,
    stdout_stream: u64,
    stderr_stream: u64,
    wait_lock: Mutex<()>,
    pub(crate) condvar: Condvar,
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
impl Drop for ProcessState {
    fn drop(&mut self) {
        if self.exit_code.load(AtomicOrdering::Acquire) == PROCESS_EXIT_PENDING {
            if let Ok(mut guard) = self.child.lock() {
                let _ = guard.kill();
            }
        }
        if self.stdin_stream != 0 {
            unsafe {
                molt_stream_drop(self.stdin_stream);
            }
        }
        if self.stdout_stream != 0 {
            unsafe {
                molt_stream_drop(self.stdout_stream);
            }
        }
        if self.stderr_stream != 0 {
            unsafe {
                molt_stream_drop(self.stderr_stream);
            }
        }
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

#[cfg(not(target_arch = "wasm32"))]
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
            return -(sig as i32);
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
        let mut guard = state.child.lock().unwrap();
        match guard.try_wait() {
            Ok(Some(status)) => {
                let code = exit_code_from_status(status);
                state.exit_code.store(code, AtomicOrdering::Release);
                drop(guard);
                state.condvar.notify_all();
                if let Some(future) = state.wait_future.lock().unwrap().take() {
                    let gil = GilGuard::new();
                    let py = gil.token();
                    let waiters = await_waiters_take(&py, future.0);
                    for waiter in waiters {
                        wake_task_ptr(&py, waiter.0);
                    }
                }
                break;
            }
            Ok(None) => {}
            Err(_) => {}
        }
        drop(guard);
        thread::sleep(Duration::from_millis(10));
    }
}
