use super::super::{process_task_state, wake_await_waiters};
use super::stdio::{PROCESS_STDIO_PIPE, process_stdio_mode};
use super::{
    PROCESS_EXIT_PENDING, ProcessCompletionState, ProcessTaskState, ProcessWaitFutureInstall,
    ProcessWaiterId,
};
use crate::libc_compat as libc;
use crate::runtime_flags::RuntimeFlag;
use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

fn string_from_bits_wasm(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<String, u64> {
    use crate::builtins::io_path_utils::{PathFlavor, fspath_bits_with_flavor};
    let (path, flavor) = fspath_bits_with_flavor(_py, bits)?;
    let _owner = obj_from_bits(path).as_ptr().map(crate::PtrDropGuard::new);
    match flavor {
        PathFlavor::Str => string_obj_to_owned(obj_from_bits(path)).ok_or_else(|| {
            raise_exception::<u64>(_py, "SystemError", "path conversion returned invalid text")
        }),
        PathFlavor::Bytes => unsafe {
            let ptr = obj_from_bits(path).as_ptr().expect("validated bytes path");
            let bytes = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
            std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| {
                raise_exception::<u64>(_py, "TypeError", &format!("{label} bytes must be utf-8"))
            })
        },
    }
}

fn argv_from_bits_wasm(_py: &PyToken<'_>, args_bits: u64) -> Result<Vec<String>, u64> {
    let obj = obj_from_bits(args_bits);
    if obj.is_none() {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "args must be a sequence",
        ));
    }
    if let Some(ptr) = obj.as_ptr() {
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            let Some(elems) = (unsafe {
                crate::object::seq_access::snapshot(
                    _py,
                    ptr,
                    "process argument snapshot allocation failed",
                )
            }) else {
                return Err(MoltObject::none().bits());
            };
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
) -> Result<(Option<Vec<(String, String)>>, bool), u64> {
    let obj = obj_from_bits(env_bits);
    if obj.is_none() {
        return Ok((None, false));
    }
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "env must be a dict",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "env must be a dict",
            ));
        }
        let items_bits = crate::molt_dict_items(env_bits);
        let items =
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(_py.core_token(), items_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let mut entries = crate::object::iterable::OwnedIterator::new(_py, items.bits())
            .ok_or_else(|| MoltObject::none().bits())?;
        drop(items);
        let mut out = Vec::with_capacity(dict_len(ptr));
        let mut overlay = false;
        while let Some(pair_bits) = entries.next().map_err(|_| MoltObject::none().bits())? {
            // The real dict-items iterator checks mutation and owns this pair
            // through both conversions; no backing borrow survives a callback.
            let pair =
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(_py.core_token(), pair_bits);
            let pair_ptr = obj_from_bits(pair.bits())
                .as_ptr()
                .expect("dict-items pair");
            let (key_bits, val_bits) =
                crate::object::seq_access::tuple_pair(pair_ptr).expect("dict-items pair");
            let key = string_from_bits_wasm(_py, key_bits, "env key")?;
            let value = string_from_bits_wasm(_py, val_bits, "env value")?;
            if key == RuntimeFlag::MOLT_ENV_OVERLAY.name() && value == "1" {
                overlay = true;
            } else {
                out.push((key, value));
            }
        }
        Ok((Some(out), overlay))
    }
}

fn cwd_from_bits_wasm(_py: &PyToken<'_>, cwd_bits: u64) -> Result<Option<String>, u64> {
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
        if require_process_capability::<u64>(_py, crate::OperationId::ProcessExec).is_err() {
            return MoltObject::none().bits();
        }
        let args = match argv_from_bits_wasm(_py, args_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        if args.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "args must not be empty");
        }
        let (mut env_entries, mut overlay) = match env_from_bits_wasm(_py, env_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        super::child_resources::enforce_child_resource_env_entries(
            &mut env_entries,
            &mut overlay,
            runtime_state(_py).policy().resources.as_ref(),
        );
        let cwd = match cwd_from_bits_wasm(_py, cwd_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let stdin_mode = process_stdio_mode(_py, stdin_bits, "stdin");
        let stdout_mode = process_stdio_mode(_py, stdout_bits, "stdout");
        let stderr_mode = process_stdio_mode(_py, stderr_bits, "stderr");

        let args_buf = encode_string_list(&args);
        let args_ptr = alloc_bytes(_py, &args_buf);
        if args_ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
            completion: ProcessCompletionState::pending(),
            streams_released: AtomicBool::new(false),
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
        if !state.process.is_pending() {
            task_take_cancel_pending(obj_ptr);
        } else if task_cancel_pending(obj_ptr) {
            task_take_cancel_pending(obj_ptr);
            state.cancel_wait();
            return raise_cancelled_with_message::<i64>(_py, obj_ptr);
        }
        // Some hosts deliver process/stdio completion only while this import
        // drains their queues. No runtime registry lock is held across it;
        // the host may re-enter molt_process_host_notify.
        if state.process.is_pending() {
            let _ = unsafe { crate::molt_process_host_poll() };
        }
        let code = state.process.exit_code();
        if code != PROCESS_EXIT_PENDING {
            return MoltObject::from_int(code as i64).bits() as i64;
        }
        let mut out_code: i32 = 0;
        let rc = unsafe { crate::molt_process_wait_host(state.process.handle, 0, &mut out_code) };
        if rc == 0 {
            if state.process.publish_exit(out_code).is_some()
                && let Some(future) = state.process.completion.claim_wait_future(_py)
            {
                let _ = wake_await_waiters(_py, future.0);
            }
            return MoltObject::from_int(state.process.exit_code() as i64).bits() as i64;
        }
        if rc == -libc::EWOULDBLOCK || rc == -libc::EAGAIN {
            // Directly spawned process futures own this retry themselves;
            // inline futures register their scheduled awaiter at the shared
            // sleep_register_impl boundary after returning Pending.
            crate::async_rt::host_progress::register_host_progress_retry(_py, obj_ptr);
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
            let code = handle.state.exit_code();
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
            if !handle.state.is_pending() {
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
            if !handle.state.is_pending() {
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
pub unsafe extern "C" fn molt_process_drop(proc_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let proc_ptr = ptr_from_bits(proc_bits);
            if proc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let handle = &*(proc_ptr as *mut MoltProcessHandle);
            runtime_state(_py)
                .process_registry
                .remove_wasm_handle(handle.state.handle);
            release_ptr(proc_ptr);
            drop(Box::from_raw(proc_ptr as *mut MoltProcessHandle));
            MoltObject::none().bits()
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
            if handle_obj.state.publish_exit(exit_code).is_some()
                && let Some(future) = handle_obj.state.completion.claim_wait_future(_py)
            {
                let _ = wake_await_waiters(_py, future.0);
            }
        })
    }
}

pub(crate) struct ProcessState {
    handle: i64,
    pub(super) completion: ProcessCompletionState,
    streams_released: AtomicBool,
    stdin_stream: u64,
    stdout_stream: u64,
    stderr_stream: u64,
}

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
        if self.is_pending() {
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

#[cfg(test)]
mod process_state_tests {
    use super::*;

    #[test]
    fn wasm_process_wait_cancellation_uses_shared_completion_state() {
        let state = Arc::new(ProcessState {
            handle: -1,
            completion: ProcessCompletionState::pending(),
            streams_released: AtomicBool::new(false),
            stdin_stream: 0,
            stdout_stream: 0,
            stderr_stream: 0,
        });
        let waiter = PtrSlot(1usize as *mut u8);
        assert!(matches!(
            state.install_wait_future(waiter),
            ProcessWaitFutureInstall::Installed
        ));
        let task = ProcessTaskState {
            process: Arc::clone(&state),
            waiter_id: ProcessWaiterId::from_slot(waiter),
            cancelled: AtomicBool::new(false),
        };

        assert!(task.cancel_wait());
        assert!(!task.cancel_wait());
        assert_eq!(state.wait_future(), None);
        assert!(state.publish_exit(0).is_some());
    }
}
