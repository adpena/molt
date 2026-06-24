use super::generators_async::asyncio_clear_pending_exception;
use super::process_task_state;
use super::wake_await_waiters;
use crate::*;

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::ffi::OsStr;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::process::Stdio;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(target_arch = "wasm32")]
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, JoinHandle};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

// --- Process ---

const PROCESS_STDIO_INHERIT: i32 = 0;
const PROCESS_STDIO_PIPE: i32 = 1;
const PROCESS_STDIO_DEVNULL: i32 = 2;
const PROCESS_STDIO_STDOUT: i32 = -2;
const PROCESS_STDIO_FD_BASE: i32 = 1 << 30;
#[cfg(not(target_arch = "wasm32"))]
const PROCESS_PIPE_MAX_QUEUED_BYTES_ENV: &str = "MOLT_PROCESS_PIPE_MAX_QUEUED_BYTES";
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
fn process_pipe_max_queued_bytes() -> usize {
    std::env::var(PROCESS_PIPE_MAX_QUEUED_BYTES_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(super::channels::default_stream_max_queued_bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn new_process_pipe_stream() -> u64 {
    super::channels::stream_new_with_byte_budget(0, process_pipe_max_queued_bytes())
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

const CHILD_RESOURCE_ENV_KEYS: &[&str] = &[
    "MOLT_RESOURCE_MAX_MEMORY",
    "MOLT_RESOURCE_MAX_DURATION_MS",
    "MOLT_RESOURCE_MAX_ALLOCATIONS",
    "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
    // Per-operation result caps are raw-byte integers like the keys above, so a
    // spawned child inherits the tighter of (parent, child-requested) for each.
    // The `MOLT_MEMORY_LIMIT` human-size alias is intentionally absent: a child
    // resolves it into `max_memory` at its own init, and the numeric min-merge
    // here only handles raw integers.
    "MOLT_RESOURCE_MAX_OPERATION_RESULT",
    "MOLT_RESOURCE_MAX_POW_RESULT",
    "MOLT_RESOURCE_MAX_REPEAT_RESULT",
    "MOLT_RESOURCE_MAX_SHIFT_RESULT",
    "MOLT_RESOURCE_MAX_STRING_RESULT",
];

fn parse_resource_limit(raw: &str) -> Option<u128> {
    raw.trim().parse::<u128>().ok()
}

fn active_parent_resource_limit(key: &str) -> Option<u128> {
    std::env::var(key)
        .ok()
        .and_then(|raw| parse_resource_limit(&raw))
}

#[cfg(target_arch = "wasm32")]
fn env_entry_value<'a>(entries: Option<&'a [(String, String)]>, key: &str) -> Option<&'a str> {
    entries?
        .iter()
        .rev()
        .find(|(entry_key, _)| entry_key == key)
        .map(|(_, value)| value.as_str())
}

#[cfg(not(target_arch = "wasm32"))]
fn env_entry_os_value<'a>(
    entries: Option<&'a [(std::ffi::OsString, std::ffi::OsString)]>,
    key: &str,
) -> Option<&'a OsStr> {
    entries?
        .iter()
        .rev()
        .find(|(entry_key, _)| entry_key == key)
        .map(|(_, value)| value.as_os_str())
}

fn enforced_child_resource_env_value(key: &str, requested: Option<&str>) -> Option<String> {
    let parent_limit = active_parent_resource_limit(key)?;
    let selected = match requested.and_then(parse_resource_limit) {
        Some(child_limit) if child_limit < parent_limit => child_limit,
        _ => parent_limit,
    };
    Some(selected.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn enforced_child_resource_env_os_value(key: &str, requested: Option<&OsStr>) -> Option<String> {
    enforced_child_resource_env_value(key, requested.and_then(OsStr::to_str))
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_child_resource_env(
    cmd: &mut std::process::Command,
    env_entries: Option<&[(std::ffi::OsString, std::ffi::OsString)]>,
) {
    for key in CHILD_RESOURCE_ENV_KEYS {
        if let Some(value) =
            enforced_child_resource_env_os_value(key, env_entry_os_value(env_entries, key))
        {
            cmd.env(key, value);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn enforce_child_resource_env_entries(
    entries: &mut Option<Vec<(String, String)>>,
    overlay: &mut bool,
) {
    let mut enforced = Vec::new();
    for key in CHILD_RESOURCE_ENV_KEYS {
        if let Some(value) =
            enforced_child_resource_env_value(key, env_entry_value(entries.as_deref(), key))
        {
            enforced.push(((*key).to_string(), value));
        }
    }
    if enforced.is_empty() {
        return;
    }
    let entries = entries.get_or_insert_with(|| {
        *overlay = true;
        Vec::new()
    });
    for (key, value) in enforced {
        entries.retain(|(entry_key, _)| entry_key != &key);
        entries.push((key, value));
    }
}

#[cfg(unix)]
fn parse_child_rlimit_bytes_env(name: &str) -> Option<Option<u64>> {
    let raw = std::env::var(name).ok()?;
    let value = raw.trim().parse::<u64>().ok()?;
    if value == 0 {
        Some(None)
    } else {
        Some(Some(value))
    }
}

#[cfg(unix)]
fn parse_child_rlimit_gb_env(name: &str) -> Option<Option<u64>> {
    let raw = std::env::var(name).ok()?;
    let value = raw.trim().parse::<f64>().ok()?;
    if value == 0.0 {
        return Some(None);
    }
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let bytes = value * 1024.0 * 1024.0 * 1024.0;
    if bytes > u64::MAX as f64 {
        return None;
    }
    Some(Some(bytes as u64))
}

#[cfg(unix)]
fn child_memory_rlimit_bytes() -> Option<u64> {
    for candidate in [
        parse_child_rlimit_bytes_env("MOLT_CHILD_RLIMIT_BYTES"),
        parse_child_rlimit_gb_env("MOLT_CHILD_RLIMIT_GB"),
    ] {
        if let Some(None) = candidate {
            return None;
        }
    }

    let mut limit = active_parent_resource_limit("MOLT_RESOURCE_MAX_MEMORY")
        .and_then(|value| u64::try_from(value).ok());
    for candidate in [
        parse_child_rlimit_bytes_env("MOLT_CHILD_RLIMIT_BYTES"),
        parse_child_rlimit_gb_env("MOLT_CHILD_RLIMIT_GB"),
    ] {
        if let Some(Some(value)) = candidate {
            limit = Some(limit.map_or(value, |current| current.min(value)));
        }
    }
    limit.filter(|value| *value > 0)
}

#[cfg(unix)]
fn apply_child_memory_rlimit(cmd: &mut std::process::Command) {
    let Some(limit_bytes) = child_memory_rlimit_bytes() else {
        return;
    };
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(move || {
            let hard_limit = limit_bytes.min(libc::rlim_t::MAX as u64) as libc::rlim_t;
            let limit = libc::rlimit {
                rlim_cur: hard_limit,
                rlim_max: hard_limit,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(unix)]
fn configure_unix_owned_process_group(
    cmd: &mut std::process::Command,
    start_new_session: bool,
    process_group: Option<i64>,
) -> bool {
    use std::os::unix::process::CommandExt;

    let setpgid_target = match (start_new_session, process_group) {
        (true, None | Some(0)) => None,
        (true, Some(pgid)) => Some(pgid),
        (false, Some(pgid)) => Some(pgid),
        (false, None) => Some(0),
    };
    let owns_group = start_new_session || setpgid_target == Some(0);
    if start_new_session || setpgid_target.is_some() {
        unsafe {
            cmd.pre_exec(move || {
                if start_new_session && libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(pgid) = setpgid_target
                    && libc::setpgid(0, pgid as libc::pid_t) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    owns_group
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod child_resource_tests {
    use super::*;

    fn with_env<R>(updates: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        // Use the single process-wide test mutex shared with the resource-env
        // tests (resource.rs, ops_sys.rs). These suites mutate the SAME
        // MOLT_RESOURCE_MAX_* env vars; a private mutex here would let them race
        // and clobber each other's env across module boundaries.
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = updates
            .iter()
            .map(|(key, _)| (*key, std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for (key, value) in updates {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        let result = f();
        for (key, value) in saved {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        result
    }

    #[test]
    fn child_resource_env_inherits_parent_when_child_omits_limit() {
        with_env(
            &[
                ("MOLT_RESOURCE_MAX_MEMORY", Some("4096")),
                ("MOLT_CHILD_RLIMIT_BYTES", None),
                ("MOLT_CHILD_RLIMIT_GB", None),
            ],
            || {
                assert_eq!(
                    enforced_child_resource_env_value("MOLT_RESOURCE_MAX_MEMORY", None),
                    Some("4096".to_string())
                );
            },
        );
    }

    #[test]
    fn child_resource_env_can_tighten_but_not_widen_parent_limit() {
        with_env(&[("MOLT_RESOURCE_MAX_MEMORY", Some("4096"))], || {
            assert_eq!(
                enforced_child_resource_env_value("MOLT_RESOURCE_MAX_MEMORY", Some("8192")),
                Some("4096".to_string())
            );
            assert_eq!(
                enforced_child_resource_env_value("MOLT_RESOURCE_MAX_MEMORY", Some("1024")),
                Some("1024".to_string())
            );
            assert_eq!(
                enforced_child_resource_env_value("MOLT_RESOURCE_MAX_MEMORY", Some("invalid")),
                Some("4096".to_string())
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn child_memory_rlimit_uses_tightest_runtime_and_shared_limit() {
        with_env(
            &[
                ("MOLT_RESOURCE_MAX_MEMORY", Some("8192")),
                ("MOLT_CHILD_RLIMIT_BYTES", Some("4096")),
                ("MOLT_CHILD_RLIMIT_GB", None),
            ],
            || {
                assert_eq!(child_memory_rlimit_bytes(), Some(4096));
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn child_memory_rlimit_zero_shared_limit_disables_os_limit() {
        with_env(
            &[
                ("MOLT_RESOURCE_MAX_MEMORY", Some("4096")),
                ("MOLT_CHILD_RLIMIT_BYTES", None),
                ("MOLT_CHILD_RLIMIT_GB", Some("0")),
            ],
            || {
                assert_eq!(child_memory_rlimit_bytes(), None);
            },
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn ignore_sigpipe() {
    static IGNORE: OnceLock<()> = OnceLock::new();
    IGNORE.get_or_init(|| {
        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
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
        let duped = unsafe { libc::dup(fd as libc::c_int) };
        if duped < 0 {
            return None;
        }
        let handle = unsafe { libc::get_osfhandle(duped as libc::c_int) };
        if handle == -1 {
            unsafe {
                libc::close(duped as libc::c_int);
            }
            return None;
        }
        let file = unsafe { std::fs::File::from_raw_handle(handle as *mut _) };
        Some(Stdio::from(file))
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
#[unsafe(no_mangle)]
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
    crate::with_gil_entry_nopanic!(_py, {
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
            let Some(fileno_name_bits) = attr_name_bits_from_bytes(_py, b"fileno") else {
                return MoltObject::none().bits();
            };
            let missing = missing_bits(_py);
            let fileno_bits = molt_getattr_builtin(value_bits, fileno_name_bits, missing);
            dec_ref_bits(_py, fileno_name_bits);
            if exception_pending(_py) {
                unsafe { asyncio_clear_pending_exception(_py) };
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "unsupported subprocess stdio option",
                );
            }
            if fileno_bits == missing {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "unsupported subprocess stdio option",
                );
            }
            if !is_truthy(_py, obj_from_bits(molt_is_callable(fileno_bits))) {
                if !obj_from_bits(fileno_bits).is_none() {
                    dec_ref_bits(_py, fileno_bits);
                }
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "unsupported subprocess stdio option",
                );
            }
            let out_bits = unsafe { call_callable0(_py, fileno_bits) };
            if !obj_from_bits(fileno_bits).is_none() {
                dec_ref_bits(_py, fileno_bits);
            }
            if exception_pending(_py) {
                unsafe { asyncio_clear_pending_exception(_py) };
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "unsupported subprocess stdio option",
                );
            }
            fd = to_i64(obj_from_bits(out_bits));
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
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
                    if !super::channels::stream_enqueue_bytes_blocking(stream, bytes) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        super::channels::stream_close_local(stream);
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
                    super::channels::stream_release_queued_bytes(stream, bytes.len());
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_process_spawn(
    args_bits: u64,
    env_bits: u64,
    cwd_bits: u64,
    stdin_bits: u64,
    stdout_bits: u64,
    stderr_bits: u64,
) -> u64 {
    unsafe {
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

            let stdin_stream = if stdin_mode == PROCESS_STDIO_PIPE {
                new_process_pipe_stream()
            } else {
                0
            };
            let stdout_stream = if stdout_mode == PROCESS_STDIO_PIPE {
                new_process_pipe_stream()
            } else {
                0
            };
            let mut stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
                new_process_pipe_stream()
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

            if stdin_stream != 0
                && let Some(stdin) = child.stdin.take()
            {
                spawn_process_writer(stdin, stdin_stream);
            }
            if stdout_stream != 0 {
                if let Some(reader) = merged_stdout_reader.take() {
                    spawn_process_reader(reader, stdout_stream);
                } else if let Some(stdout) = child.stdout.take() {
                    spawn_process_reader(stdout, stdout_stream);
                }
            }
            if stderr_stream != 0
                && let Some(stderr) = child.stderr.take()
            {
                spawn_process_reader(stderr, stderr_stream);
            }

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
                stdin_stream,
                stdout_stream,
                stderr_stream,
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
    unsafe {
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

            let stdin_stream = if stdin_mode == PROCESS_STDIO_PIPE {
                new_process_pipe_stream()
            } else {
                0
            };
            let stdout_stream = if stdout_mode == PROCESS_STDIO_PIPE {
                new_process_pipe_stream()
            } else {
                0
            };
            let mut stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
                new_process_pipe_stream()
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

            if stdin_stream != 0
                && let Some(stdin) = child.stdin.take()
            {
                spawn_process_writer(stdin, stdin_stream);
            }
            if stdout_stream != 0 {
                if let Some(reader) = merged_stdout_reader.take() {
                    spawn_process_reader(reader, stdout_stream);
                } else if let Some(stdout) = child.stdout.take() {
                    spawn_process_reader(stdout, stdout_stream);
                }
            }
            if stderr_stream != 0
                && let Some(stderr) = child.stderr.take()
            {
                spawn_process_reader(stderr, stderr_stream);
            }

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
                stdin_stream,
                stdout_stream,
                stderr_stream,
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
}

#[cfg(target_arch = "wasm32")]
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
    // On WASM, start_new_session and process_group are not applicable.
    // Delegate to the base spawn implementation.
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

#[cfg(target_arch = "wasm32")]
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
        enforce_child_resource_env_entries(&mut env_entries, &mut overlay);
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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
            MoltObject::from_int(handle.state.handle as i64).bits()
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
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

#[cfg(target_arch = "wasm32")]
pub(crate) struct ProcessState {
    handle: i64,
    pub(crate) exit_code: AtomicI32,
    streams_released: AtomicBool,
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
        self.request_terminate_for_teardown();
        self.release_owned_streams();
    }
}

#[cfg(target_arch = "wasm32")]
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
            super::channels::stream_close_local(stream);
        }
        unsafe {
            molt_stream_drop(stream_bits);
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct ProcessRegistry {
    handles: Mutex<HashMap<i64, PtrSlot>>,
}

#[cfg(target_arch = "wasm32")]
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
