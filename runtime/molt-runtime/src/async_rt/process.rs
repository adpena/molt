use super::generators::{asyncio_call_method0, asyncio_clear_pending_exception};
use super::process_task_state;
use super::{await_waiters_take, wake_task_ptr};
use crate::*;

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(target_arch = "wasm32")]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(target_arch = "wasm32")]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

// --- Process ---

const PROCESS_STDIO_INHERIT: i32 = 0;
const PROCESS_STDIO_PIPE: i32 = 1;
const PROCESS_STDIO_DEVNULL: i32 = 2;
const PROCESS_STDIO_STDOUT: i32 = -2;
const PROCESS_STDIO_FD_BASE: i32 = 1 << 30;

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
fn trace_process_io() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_PROCESS_IO").ok().as_deref(),
            Some("1")
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn ignore_sigpipe() {
    static IGNORE: OnceLock<()> = OnceLock::new();
    IGNORE.get_or_init(|| unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn stdio_from_fd(fd: i32) -> Option<Stdio> {
    if fd < 0 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;
        let duped = unsafe { libc::dup(fd as libc::c_int) };
        if duped < 0 {
            return None;
        }
        let file = unsafe { std::fs::File::from_raw_fd(duped) };
        Some(Stdio::from(file))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::FromRawHandle;
        let duped = unsafe { libc::_dup(fd as libc::c_int) };
        if duped < 0 {
            return None;
        }
        let handle = unsafe { libc::_get_osfhandle(duped as libc::c_int) };
        if handle == -1 {
            unsafe {
                libc::_close(duped as libc::c_int);
            }
            return None;
        }
        let file = unsafe { std::fs::File::from_raw_handle(handle as *mut _) };
        return Some(Stdio::from(file));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fd;
        None
    }
}

fn process_stdio_mode(_py: &PyToken<'_>, bits: u64, name: &str) -> i32 {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return PROCESS_STDIO_INHERIT;
    }
    match to_i64(obj) {
        Some(val) => {
            let Ok(val) = i32::try_from(val) else {
                return raise_exception::<_>(_py, "ValueError", &format!("invalid {name} mode"));
            };
            match val {
                PROCESS_STDIO_INHERIT | PROCESS_STDIO_PIPE | PROCESS_STDIO_DEVNULL => val,
                PROCESS_STDIO_STDOUT if name == "stderr" => val,
                val if val >= PROCESS_STDIO_FD_BASE => val,
                _ => raise_exception::<_>(_py, "ValueError", &format!("invalid {name} mode")),
            }
        }
        None => raise_exception::<_>(_py, "TypeError", &format!("{name} must be int or None")),
    }
}

/// # Safety
/// - All arguments must be valid runtime objects.
#[no_mangle]
pub extern "C" fn molt_asyncio_subprocess_stdio_normalize(
    value_bits: u64,
    allow_stdout_bits: u64,
    pipe_const_bits: u64,
    devnull_const_bits: u64,
    stdout_const_bits: u64,
    inherit_mode_bits: u64,
    pipe_mode_bits: u64,
    devnull_mode_bits: u64,
    stdout_mode_bits: u64,
    fd_base_bits: u64,
    fd_max_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(inherit_mode) = to_i64(obj_from_bits(inherit_mode_bits)) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid asyncio subprocess stdio inherit mode constant",
            );
        };
        let Some(pipe_mode) = to_i64(obj_from_bits(pipe_mode_bits)) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid asyncio subprocess stdio pipe mode constant",
            );
        };
        let Some(devnull_mode) = to_i64(obj_from_bits(devnull_mode_bits)) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid asyncio subprocess stdio devnull mode constant",
            );
        };
        let Some(stdout_mode) = to_i64(obj_from_bits(stdout_mode_bits)) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid asyncio subprocess stdio stdout mode constant",
            );
        };
        let Some(fd_base) = to_i64(obj_from_bits(fd_base_bits)) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid asyncio subprocess stdio fd_base constant",
            );
        };
        let Some(fd_max) = to_i64(obj_from_bits(fd_max_bits)) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "invalid asyncio subprocess stdio fd_max constant",
            );
        };

        let value_obj = obj_from_bits(value_bits);
        if value_obj.is_none() {
            return MoltObject::from_int(inherit_mode).bits();
        }
        let allow_stdout = is_truthy(_py, obj_from_bits(allow_stdout_bits));

        if obj_eq(_py, value_obj, obj_from_bits(pipe_const_bits)) {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return MoltObject::from_int(pipe_mode).bits();
        }
        if obj_eq(_py, value_obj, obj_from_bits(devnull_const_bits)) {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return MoltObject::from_int(devnull_mode).bits();
        }
        if allow_stdout && obj_eq(_py, value_obj, obj_from_bits(stdout_const_bits)) {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return MoltObject::from_int(stdout_mode).bits();
        }

        let mut fd = to_i64(value_obj);
        if fd.is_none() {
            let fileno_bits = unsafe { asyncio_call_method0(_py, value_bits, b"fileno") };
            if exception_pending(_py) {
                unsafe { asyncio_clear_pending_exception(_py) };
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "unsupported subprocess stdio option",
                );
            }
            fd = to_i64(obj_from_bits(fileno_bits));
            if !obj_from_bits(fileno_bits).is_none() {
                dec_ref_bits(_py, fileno_bits);
            }
        }
        let Some(fd) = fd else {
            return raise_exception::<u64>(_py, "TypeError", "unsupported subprocess stdio option");
        };
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "file descriptor must be >= 0");
        }
        if fd > fd_max {
            return raise_exception::<u64>(_py, "ValueError", "file descriptor is too large");
        }
        match fd_base.checked_add(fd) {
            Some(encoded) => MoltObject::from_int(encoded).bits(),
            None => raise_exception::<u64>(_py, "ValueError", "file descriptor is too large"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
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
                if let Some(res_ptr) = res_obj.as_ptr() {
                    if object_type_id(res_ptr) == TYPE_ID_BYTES {
                        let len = bytes_len(res_ptr);
                        let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                        let text = std::str::from_utf8(bytes)
                            .map_err(|_| format!("{label} bytes must be utf-8"))?;
                        dec_ref_bits(_py, res_bits);
                        return Ok(text.to_string());
                    }
                }
                dec_ref_bits(_py, res_bits);
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("{label} must be str or bytes, not {obj_type}"))
}

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
fn cwd_from_bits_wasm(_py: &PyToken<'_>, cwd_bits: u64) -> Result<Option<String>, String> {
    let obj = obj_from_bits(cwd_bits);
    if obj.is_none() {
        return Ok(None);
    }
    Ok(Some(string_from_bits_wasm(_py, cwd_bits, "cwd")?))
}

#[cfg(target_arch = "wasm32")]
fn encode_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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
                    if trace_process_io() {
                        let limit = 256usize;
                        let preview = bytes
                            .iter()
                            .take(limit)
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if bytes.len() > limit {
                            eprintln!(
                                "molt_process_reader read {} bytes [{} ...]",
                                bytes.len(),
                                preview
                            );
                        } else {
                            eprintln!(
                                "molt_process_reader read {} bytes [{}]",
                                bytes.len(),
                                preview
                            );
                        }
                    }
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
        ignore_sigpipe();
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
                    if trace_process_io() {
                        let limit = 64usize;
                        let preview = bytes
                            .iter()
                            .take(limit)
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if bytes.len() > limit {
                            eprintln!(
                                "molt_process_writer write {} bytes [{} ...]",
                                bytes.len(),
                                preview
                            );
                        } else {
                            eprintln!(
                                "molt_process_writer write {} bytes [{}]",
                                bytes.len(),
                                preview
                            );
                        }
                    }
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                    if writer.flush().is_err() {
                        break;
                    }
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
/// # Safety
/// `args_bits`, `env_bits`, and `cwd_bits` must be valid runtime-encoded objects.
/// The runtime must be initialized and the call must be allowed to enter the GIL.
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
        if let Some(env_entries) = env_entries {
            if !overlay_env {
                cmd.env_clear();
            }
            if trace_process_io() {
                let mut has_entry = false;
                let mut has_spawn = false;
                let mut has_trusted = false;
                for (key, _value) in &env_entries {
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
        if trace_process_io() {
            eprintln!(
                "molt_process_stdio stdin={stdin_mode} stdout={stdout_mode} stderr={stderr_mode}"
            );
        }

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
        let mut stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
            molt_stream_new(0)
        } else {
            0
        };
        if stderr_mode == PROCESS_STDIO_STDOUT {
            stderr_stream = 0;
        }

        let mut merged_stdout_reader = None;

        match stdin_mode {
            PROCESS_STDIO_PIPE => {
                cmd.stdin(Stdio::piped());
            }
            PROCESS_STDIO_DEVNULL => {
                cmd.stdin(Stdio::null());
            }
            val if val >= PROCESS_STDIO_FD_BASE => {
                let fd = val - PROCESS_STDIO_FD_BASE;
                let Some(stdio) = stdio_from_fd(fd) else {
                    if stdin_stream != 0 {
                        molt_stream_drop(stdin_stream);
                    }
                    if stdout_stream != 0 {
                        molt_stream_drop(stdout_stream);
                    }
                    if stderr_stream != 0 {
                        molt_stream_drop(stderr_stream);
                    }
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "invalid stdin file descriptor",
                    );
                };
                cmd.stdin(stdio);
            }
            _ => {
                cmd.stdin(Stdio::inherit());
            }
        };

        if stderr_mode == PROCESS_STDIO_STDOUT {
            if stdout_mode == PROCESS_STDIO_PIPE {
                let (reader, writer) = match os_pipe::pipe() {
                    Ok(val) => val,
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
                        return raise_os_error::<u64>(_py, err, "pipe");
                    }
                };
                let writer_err = match writer.try_clone() {
                    Ok(val) => val,
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
                        return raise_os_error::<u64>(_py, err, "pipe");
                    }
                };
                cmd.stdout(writer);
                cmd.stderr(writer_err);
                merged_stdout_reader = Some(reader);
            } else if stdout_mode == PROCESS_STDIO_DEVNULL {
                cmd.stdout(Stdio::null());
                cmd.stderr(Stdio::null());
            } else if stdout_mode >= PROCESS_STDIO_FD_BASE {
                let fd = stdout_mode - PROCESS_STDIO_FD_BASE;
                let Some(stdout_stdio) = stdio_from_fd(fd) else {
                    if stdin_stream != 0 {
                        molt_stream_drop(stdin_stream);
                    }
                    if stdout_stream != 0 {
                        molt_stream_drop(stdout_stream);
                    }
                    if stderr_stream != 0 {
                        molt_stream_drop(stderr_stream);
                    }
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "invalid stdout file descriptor",
                    );
                };
                let Some(stderr_stdio) = stdio_from_fd(fd) else {
                    if stdin_stream != 0 {
                        molt_stream_drop(stdin_stream);
                    }
                    if stdout_stream != 0 {
                        molt_stream_drop(stdout_stream);
                    }
                    if stderr_stream != 0 {
                        molt_stream_drop(stderr_stream);
                    }
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "invalid stderr file descriptor",
                    );
                };
                cmd.stdout(stdout_stdio);
                cmd.stderr(stderr_stdio);
            } else {
                cmd.stdout(Stdio::inherit());
                cmd.stderr(Stdio::inherit());
            }
        } else {
            match stdout_mode {
                PROCESS_STDIO_PIPE => {
                    cmd.stdout(Stdio::piped());
                }
                PROCESS_STDIO_DEVNULL => {
                    cmd.stdout(Stdio::null());
                }
                val if val >= PROCESS_STDIO_FD_BASE => {
                    let fd = val - PROCESS_STDIO_FD_BASE;
                    let Some(stdio) = stdio_from_fd(fd) else {
                        if stdin_stream != 0 {
                            molt_stream_drop(stdin_stream);
                        }
                        if stdout_stream != 0 {
                            molt_stream_drop(stdout_stream);
                        }
                        if stderr_stream != 0 {
                            molt_stream_drop(stderr_stream);
                        }
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "invalid stdout file descriptor",
                        );
                    };
                    cmd.stdout(stdio);
                }
                _ => {
                    cmd.stdout(Stdio::inherit());
                }
            };
            match stderr_mode {
                PROCESS_STDIO_PIPE => {
                    cmd.stderr(Stdio::piped());
                }
                PROCESS_STDIO_DEVNULL => {
                    cmd.stderr(Stdio::null());
                }
                val if val >= PROCESS_STDIO_FD_BASE => {
                    let fd = val - PROCESS_STDIO_FD_BASE;
                    let Some(stdio) = stdio_from_fd(fd) else {
                        if stdin_stream != 0 {
                            molt_stream_drop(stdin_stream);
                        }
                        if stdout_stream != 0 {
                            molt_stream_drop(stdout_stream);
                        }
                        if stderr_stream != 0 {
                            molt_stream_drop(stderr_stream);
                        }
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "invalid stderr file descriptor",
                        );
                    };
                    cmd.stderr(stdio);
                }
                _ => {
                    cmd.stderr(Stdio::inherit());
                }
            };
        }

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
            if let Some(reader) = merged_stdout_reader.take() {
                spawn_process_reader(reader, stdout_stream);
            } else if let Some(stdout) = child.stdout.take() {
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
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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
/// # Safety
/// `obj_bits` must be a valid process wait future object from this runtime.
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
extern "C" fn process_stdin_send_host_hook(ctx: *mut u8, data_ptr: *const u8, len: usize) -> i64 {
    if ctx.is_null() {
        return pending_bits_i64();
    }
    let handle = unsafe { *(ctx as *mut i64) };
    let rc = unsafe { crate::molt_process_write_host(handle, data_ptr, len as u64) };
    if rc == 0 {
        0
    } else if rc == -(libc::EWOULDBLOCK as i32) || rc == -(libc::EAGAIN as i32) {
        pending_bits_i64()
    } else {
        MoltObject::none().bits() as i64
    }
}

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `args_bits`, `env_bits`, and `cwd_bits` must be valid runtime-encoded objects.
/// The runtime must be initialized and the call must be allowed to enter the GIL.
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
        let args = match argv_from_bits_wasm(_py, args_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if args.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "args must not be empty");
        }
        let (env_entries, overlay) = match env_from_bits_wasm(_py, env_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
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
                process_stdin_send_host_hook as usize,
                process_stdin_close_host_hook as usize,
                ctx_ptr,
            );
            if stream_ptr.is_null() {
                let _ = unsafe { crate::molt_process_terminate_host(handle) };
                unsafe {
                    drop(Box::from_raw(ctx_ptr as *mut i64));
                }
                return raise_exception::<_>(_py, "RuntimeError", "stdin stream creation failed");
            }
            bits_from_ptr(stream_ptr)
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
            wait_future: Mutex::new(None),
            stdin_stream,
            stdout_stream,
            stderr_stream,
        });
        let handle_obj = Box::new(MoltProcessHandle {
            state: Arc::clone(&state),
        });
        let handle_ptr = Box::into_raw(handle_obj) as *mut u8;
        wasm_process_handles()
            .lock()
            .unwrap()
            .insert(handle, PtrSlot(handle_ptr));
        bits_from_ptr(handle_ptr)
    })
}

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `obj_bits` must be a valid process wait future object from this runtime.
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
        if rc == -(libc::EWOULDBLOCK as i32) || rc == -(libc::EAGAIN as i32) {
            return pending_bits_i64();
        }
        raise_exception::<i64>(_py, "RuntimeError", "process wait failed")
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_process_pid(proc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let proc_ptr = ptr_from_bits(proc_bits);
        if proc_ptr.is_null() {
            return MoltObject::from_int(0).bits();
        }
        let handle = &*(proc_ptr as *mut MoltProcessHandle);
        MoltObject::from_int(handle.state.handle as i64).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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
        let rc = unsafe { crate::molt_process_kill_host(handle.state.handle) };
        if rc != 0 {
            return raise_exception::<_>(_py, "OSError", "process kill failed");
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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
            MoltObject::none().bits()
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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
        let rc = unsafe { crate::molt_process_terminate_host(handle.state.handle) };
        if rc != 0 {
            return raise_exception::<_>(_py, "OSError", "process terminate failed");
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
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

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `proc_bits` must reference a live process handle from this runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_process_drop(proc_bits: u64) {
    crate::with_gil_entry!(_py, {
        let proc_ptr = ptr_from_bits(proc_bits);
        if proc_ptr.is_null() {
            return;
        }
        let handle = &*(proc_ptr as *mut MoltProcessHandle);
        wasm_process_handles()
            .lock()
            .unwrap()
            .remove(&handle.state.handle);
        release_ptr(proc_ptr);
        drop(Box::from_raw(proc_ptr as *mut MoltProcessHandle));
    })
}

#[cfg(target_arch = "wasm32")]
/// # Safety
/// `handle` must be a valid wasm process handle owned by this runtime.
#[no_mangle]
pub unsafe extern "C" fn molt_process_host_notify(handle: i64, exit_code: i32) {
    crate::with_gil_entry!(_py, {
        let entry = wasm_process_handles().lock().unwrap().get(&handle).cloned();
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
            let waiters = await_waiters_take(_py, future.0);
            for waiter in waiters {
                wake_task_ptr(_py, waiter.0);
            }
        }
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

#[cfg(target_arch = "wasm32")]
pub(crate) struct ProcessState {
    handle: i64,
    pub(crate) exit_code: AtomicI32,
    pub(crate) wait_future: Mutex<Option<PtrSlot>>,
    stdin_stream: u64,
    stdout_stream: u64,
    stderr_stream: u64,
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct ProcessTaskState {
    pub(crate) process: Arc<ProcessState>,
    pub(crate) cancelled: AtomicBool,
}

#[cfg(target_arch = "wasm32")]
unsafe impl Send for ProcessTaskState {}
#[cfg(target_arch = "wasm32")]
unsafe impl Sync for ProcessTaskState {}

#[cfg(target_arch = "wasm32")]
struct MoltProcessHandle {
    state: Arc<ProcessState>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for ProcessState {
    fn drop(&mut self) {
        if self.exit_code.load(AtomicOrdering::Acquire) == PROCESS_EXIT_PENDING {
            let _ = unsafe { crate::molt_process_terminate_host(self.handle) };
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

#[cfg(target_arch = "wasm32")]
fn wasm_process_handles() -> &'static Mutex<HashMap<i64, PtrSlot>> {
    static HANDLES: OnceLock<Mutex<HashMap<i64, PtrSlot>>> = OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
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
