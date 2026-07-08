use super::super::{process_task_state, wake_await_waiters};
use super::PROCESS_EXIT_PENDING;
use super::stdio::{PROCESS_STDIO_PIPE, process_stdio_mode};
use crate::libc_compat as libc;
use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

fn string_from_bits_wasm(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<String, String> {
    let obj = obj_from_bits(bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(text);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| format!("{label} bytes must be utf-8"))?;
                return Ok(text.to_string());
            }
            let fspath_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.fspath_name, b"__fspath__");
            if let Some(call_bits) = attr_lookup_ptr(_py, ptr, fspath_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return Err(format!("{label} __fspath__ failed"));
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(text) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(_py, res_bits);
                    return Ok(text);
                }
                if let Some(res_ptr) = res_obj.as_ptr()
                    && object_type_id(res_ptr) == TYPE_ID_BYTES
                {
                    let len = bytes_len(res_ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                    let text = std::str::from_utf8(bytes)
                        .map_err(|_| format!("{label} bytes must be utf-8"))?;
                    dec_ref_bits(_py, res_bits);
                    return Ok(text.to_string());
                }
                dec_ref_bits(_py, res_bits);
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("{label} must be str or bytes, not {obj_type}"))
}

fn argv_from_bits_wasm(_py: &PyToken<'_>, args_bits: u64) -> Result<Vec<String>, String> {
    let obj = obj_from_bits(args_bits);
    if obj.is_none() {
        return Err("args must be a sequence".to_string());
    }
    if let Some(ptr) = obj.as_ptr() {
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            let elems = unsafe { seq_vec_ref(ptr) };
            let mut args = Vec::with_capacity(elems.len());
            for &elem in elems.iter() {
                args.push(string_from_bits_wasm(_py, elem, "arg")?);
            }
            return Ok(args);
        }
    }
    Ok(vec![string_from_bits_wasm(_py, args_bits, "arg")?])
}

#[allow(clippy::type_complexity)]
fn env_from_bits_wasm(
    _py: &PyToken<'_>,
    env_bits: u64,
) -> Result<(Option<Vec<(String, String)>>, bool), String> {
    let obj = obj_from_bits(env_bits);
    if obj.is_none() {
        return Ok((None, false));
    }
    let Some(ptr) = obj.as_ptr() else {
        return Err("env must be a dict".to_string());
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return Err("env must be a dict".to_string());
        }
        let order = dict_order(ptr);
        let mut out = Vec::with_capacity(order.len() / 2);
        let mut overlay = false;
        let mut idx = 0;
        while idx + 1 < order.len() {
            let key_bits = order[idx];
            let val_bits = order[idx + 1];
            let key = string_from_bits_wasm(_py, key_bits, "env key")?;
            let value = string_from_bits_wasm(_py, val_bits, "env value")?;
            if key == "MOLT_ENV_OVERLAY" && value == "1" {
                overlay = true;
            } else {
                out.push((key, value));
            }
            idx += 2;
        }
        Ok((Some(out), overlay))
    }
}

fn cwd_from_bits_wasm(_py: &PyToken<'_>, cwd_bits: u64) -> Result<Option<String>, String> {
    let obj = obj_from_bits(cwd_bits);
    if obj.is_none() {
        return Ok(None);
    }
    Ok(Some(string_from_bits_wasm(_py, cwd_bits, "cwd")?))
}

fn encode_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn encode_string_list(values: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_u32(&mut out, values.len() as u32);
    for value in values {
        let bytes = value.as_bytes();
        encode_u32(&mut out, bytes.len() as u32);
        out.extend_from_slice(bytes);
    }
    out
}

fn encode_env_entries(entries: &[(String, String)], overlay: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mode: u8 = if overlay { 2 } else { 1 };
    out.push(mode);
    encode_u32(&mut out, entries.len() as u32);
    for (key, value) in entries {
        let key_bytes = key.as_bytes();
        let value_bytes = value.as_bytes();
        encode_u32(&mut out, key_bytes.len() as u32);
        out.extend_from_slice(key_bytes);
        encode_u32(&mut out, value_bytes.len() as u32);
        out.extend_from_slice(value_bytes);
    }
    out
}

/// # Safety
/// All arguments must be valid runtime-encoded objects.
/// WASM target ignores start_new_session and process_group (no Unix process model).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_spawn_ex(
    args_bits: u64,
    env_bits: u64,
    cwd_bits: u64,
    stdin_bits: u64,
    stdout_bits: u64,
    stderr_bits: u64,
    _start_new_session_bits: u64,
    _process_group_bits: u64,
) -> u64 {
    unsafe {
        molt_process_spawn(
            args_bits,
            env_bits,
            cwd_bits,
            stdin_bits,
            stdout_bits,
            stderr_bits,
        )
    }
}

extern "C" fn process_stdin_send_host_hook(ctx: *mut u8, data_ptr: *const u8, len: usize) -> i64 {
    if ctx.is_null() {
        return pending_bits_i64();
    }
    let handle = unsafe { *(ctx as *mut i64) };
    let rc = unsafe { crate::molt_process_write_host(handle, data_ptr, len as u64) };
    if rc == 0 {
        0
    } else if rc == -libc::EWOULDBLOCK || rc == -libc::EAGAIN {
        pending_bits_i64()
    } else {
        MoltObject::none().bits() as i64
    }
}

extern "C" fn process_stdin_close_host_hook(ctx: *mut u8) {
    if ctx.is_null() {
        return;
    }
    let handle = unsafe { *(ctx as *mut i64) };
    let _ = unsafe { crate::molt_process_close_stdin_host(handle) };
    unsafe {
        drop(Box::from_raw(ctx as *mut i64));
    }
}

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
        if require_process_capability::<u64>(_py, &["process", "process.exec"]).is_err() {
            return MoltObject::none().bits();
        }
        let args = match argv_from_bits_wasm(_py, args_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if args.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "args must not be empty");
        }
        let (mut env_entries, mut overlay) = match env_from_bits_wasm(_py, env_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        super::child_resources::enforce_child_resource_env_entries(&mut env_entries, &mut overlay);
        let cwd = match cwd_from_bits_wasm(_py, cwd_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let stdin_mode = process_stdio_mode(_py, stdin_bits, "stdin");
        let stdout_mode = process_stdio_mode(_py, stdout_bits, "stdout");
        let stderr_mode = process_stdio_mode(_py, stderr_bits, "stderr");

        let args_buf = encode_string_list(&args);
        let args_ptr = alloc_bytes(_py, &args_buf);
        if args_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "args allocation failed");
        }
        let args_bits_obj = MoltObject::from_ptr(args_ptr).bits();
        let args_data = unsafe { bytes_data(args_ptr) } as u32;
        let args_len = unsafe { bytes_len(args_ptr) } as u32;

        let mut env_bits_obj = MoltObject::none().bits();
        let mut env_data: u32 = 0;
        let mut env_len: u32 = 0;
        if let Some(entries) = env_entries.as_ref() {
            let env_buf = encode_env_entries(entries, overlay);
            let env_ptr = alloc_bytes(_py, &env_buf);
            if env_ptr.is_null() {
                dec_ref_bits(_py, args_bits_obj);
                return raise_exception::<_>(_py, "MemoryError", "env allocation failed");
            }
            env_bits_obj = MoltObject::from_ptr(env_ptr).bits();
            env_data = unsafe { bytes_data(env_ptr) } as u32;
            env_len = unsafe { bytes_len(env_ptr) } as u32;
        }

        let mut cwd_bits_obj = MoltObject::none().bits();
        let mut cwd_data: u32 = 0;
        let mut cwd_len: u32 = 0;
        if let Some(cwd) = cwd.as_ref() {
            let cwd_ptr = alloc_bytes(_py, cwd.as_bytes());
            if cwd_ptr.is_null() {
                dec_ref_bits(_py, args_bits_obj);
                if !obj_from_bits(env_bits_obj).is_none() {
                    dec_ref_bits(_py, env_bits_obj);
                }
                return raise_exception::<_>(_py, "MemoryError", "cwd allocation failed");
            }
            cwd_bits_obj = MoltObject::from_ptr(cwd_ptr).bits();
            cwd_data = unsafe { bytes_data(cwd_ptr) } as u32;
            cwd_len = unsafe { bytes_len(cwd_ptr) } as u32;
        }

        let mut handle: i64 = 0;
        let rc = unsafe {
            crate::molt_process_spawn_host(
                args_data,
                args_len,
                env_data,
                env_len,
                cwd_data,
                cwd_len,
                stdin_mode,
                stdout_mode,
                stderr_mode,
                &mut handle as *mut i64,
            )
        };

        dec_ref_bits(_py, args_bits_obj);
        if !obj_from_bits(env_bits_obj).is_none() {
            dec_ref_bits(_py, env_bits_obj);
        }
        if !obj_from_bits(cwd_bits_obj).is_none() {
            dec_ref_bits(_py, cwd_bits_obj);
        }

        if rc != 0 || handle == 0 {
            return raise_exception::<_>(_py, "RuntimeError", "process spawn failed");
        }

        let stdin_stream = if stdin_mode == PROCESS_STDIO_PIPE {
            let ctx_ptr = Box::into_raw(Box::new(handle)) as *mut u8;
            let stream_ptr = molt_stream_new_with_hooks(
                process_stdin_send_host_hook as *const () as usize,
                process_stdin_close_host_hook as *const () as usize,
                ctx_ptr,
            );
            if stream_ptr.is_null() {
                let _ = unsafe { crate::molt_process_terminate_host(handle) };
                unsafe {
                    drop(Box::from_raw(ctx_ptr as *mut i64));
                }
                return raise_exception::<_>(_py, "RuntimeError", "stdin stream creation failed");
            }
            opaque_handle_bits(stream_ptr)
        } else {
            0
        };

        let stdout_stream = if stdout_mode == PROCESS_STDIO_PIPE {
            let mut stream_bits: u64 = 0;
            let rc =
                unsafe { crate::molt_process_stdio_host(handle, 1, &mut stream_bits as *mut u64) };
            if rc != 0 || stream_bits == 0 {
                if stdin_stream != 0 {
                    unsafe {
                        molt_stream_drop(stdin_stream);
                    }
                }
                let _ = unsafe { crate::molt_process_terminate_host(handle) };
                return raise_exception::<_>(_py, "RuntimeError", "stdout stream failed");
            }
            stream_bits
        } else {
            0
        };

        let stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
            let mut stream_bits: u64 = 0;
            let rc =
                unsafe { crate::molt_process_stdio_host(handle, 2, &mut stream_bits as *mut u64) };
            if rc != 0 || stream_bits == 0 {
                if stdin_stream != 0 {
                    unsafe {
                        molt_stream_drop(stdin_stream);
                    }
                }
                if stdout_stream != 0 {
                    unsafe {
                        molt_stream_drop(stdout_stream);
                    }
                }
                let _ = unsafe { crate::molt_process_terminate_host(handle) };
                return raise_exception::<_>(_py, "RuntimeError", "stderr stream failed");
            }
            stream_bits
        } else {
            0
        };

        let state = Arc::new(ProcessState {
            handle,
            exit_code: AtomicI32::new(PROCESS_EXIT_PENDING),
            streams_released: AtomicBool::new(false),
            wait_future: Mutex::new(None),
            stdin_stream,
            stdout_stream,
            stderr_stream,
        });
        let handle_obj = Box::new(MoltProcessHandle {
            state: Arc::clone(&state),
        });
        let handle_ptr = Box::into_raw(handle_obj) as *mut u8;
        runtime_state(_py)
            .process_registry
            .insert_wasm_handle(handle, PtrSlot(handle_ptr));
        opaque_handle_bits(handle_ptr)
    })
}

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
        if code != PROCESS_EXIT_PENDING {
            return MoltObject::from_int(code as i64).bits() as i64;
        }
        let mut out_code: i32 = 0;
        let rc = unsafe { crate::molt_process_wait_host(state.process.handle, 0, &mut out_code) };
        if rc == 0 {
            state
                .process
                .exit_code
                .store(out_code, AtomicOrdering::Release);
            return MoltObject::from_int(out_code as i64).bits() as i64;
        }
        if rc == -libc::EWOULDBLOCK || rc == -libc::EAGAIN {
            return pending_bits_i64();
        }
        raise_exception::<i64>(_py, "RuntimeError", "process wait failed")
    })
}

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
            MoltObject::from_int(handle.state.handle).bits()
        })
    }
}

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
            let rc = unsafe { crate::molt_process_kill_host(handle.state.handle) };
            if rc != 0 {
                return raise_exception::<_>(_py, "OSError", "process kill failed");
            }
            MoltObject::none().bits()
        })
    }
}

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
            let rc = unsafe { crate::molt_process_terminate_host(handle.state.handle) };
            if rc != 0 {
                return raise_exception::<_>(_py, "OSError", "process terminate failed");
            }
            MoltObject::none().bits()
        })
    }
}

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
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            runtime_state(_py)
                .process_registry
                .remove_wasm_handle(handle.state.handle);
            release_ptr(proc_ptr);
            drop(Box::from_raw(proc_ptr as *mut MoltProcessHandle));
        })
    }
}

/// # Safety
/// `handle` must be a valid wasm process handle owned by this runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_host_notify(handle: i64, exit_code: i32) {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let entry = runtime_state(_py).process_registry.get_wasm_handle(handle);
            let Some(slot) = entry else {
                return;
            };
            let proc_ptr = slot.0;
            if proc_ptr.is_null() {
                return;
            }
            let handle_obj = &*(proc_ptr as *mut MoltProcessHandle);
            if handle_obj.state.exit_code.load(AtomicOrdering::Acquire) != PROCESS_EXIT_PENDING {
                return;
            }
            handle_obj
                .state
                .exit_code
                .store(exit_code, AtomicOrdering::Release);
            if let Some(future) = handle_obj.state.wait_future.lock().unwrap().take() {
                let _ = wake_await_waiters(_py, future.0);
            }
        })
    }
}

pub(crate) struct ProcessState {
    handle: i64,
    pub(crate) exit_code: AtomicI32,
    streams_released: AtomicBool,
    pub(crate) wait_future: Mutex<Option<PtrSlot>>,
    stdin_stream: u64,
    stdout_stream: u64,
    stderr_stream: u64,
}

pub(crate) struct ProcessTaskState {
    pub(crate) process: Arc<ProcessState>,
    pub(crate) cancelled: AtomicBool,
}

unsafe impl Send for ProcessTaskState {}
unsafe impl Sync for ProcessTaskState {}

struct MoltProcessHandle {
    state: Arc<ProcessState>,
}

impl Drop for ProcessState {
    fn drop(&mut self) {
        self.request_terminate_for_teardown();
        self.release_owned_streams();
    }
}

impl ProcessState {
    fn request_terminate_for_teardown(&self) {
        if self.exit_code.load(AtomicOrdering::Acquire) == PROCESS_EXIT_PENDING {
            let _ = unsafe { crate::molt_process_terminate_host(self.handle) };
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
            super::super::channels::stream_close_local(stream);
        }
        unsafe {
            molt_stream_drop(stream_bits);
        }
    }
}

pub(crate) struct ProcessRegistry {
    handles: Mutex<HashMap<i64, PtrSlot>>,
}

impl ProcessRegistry {
    pub(crate) fn new() -> Self {
        Self {
            handles: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn insert_wasm_handle(&self, handle: i64, slot: PtrSlot) {
        self.handles.lock().unwrap().insert(handle, slot);
    }

    pub(crate) fn remove_wasm_handle(&self, handle: i64) {
        self.handles.lock().unwrap().remove(&handle);
    }

    pub(crate) fn get_wasm_handle(&self, handle: i64) -> Option<PtrSlot> {
        self.handles.lock().unwrap().get(&handle).copied()
    }

    pub(crate) fn drain_for_teardown(&self) {
        let handles = {
            let mut guard = self.handles.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for slot in handles.into_values() {
            let proc_ptr = slot.0;
            if proc_ptr.is_null() {
                continue;
            }
            let handle_obj = unsafe { &*(proc_ptr as *mut MoltProcessHandle) };
            handle_obj.state.request_terminate_for_teardown();
            handle_obj.state.release_owned_streams();
        }
    }
}
