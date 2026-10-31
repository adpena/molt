// === FILE: runtime/molt-runtime/src/builtins/subprocess_ext.rs ===
use crate::BuiltinExc;
//
// High-level subprocess helpers: run, check_output, check_call, and the
// symbolic PIPE / STDOUT / DEVNULL constants.
//
// The low-level spawn/wait/kill machinery lives in async_rt/process.rs.
// These helpers operate synchronously (blocking) via std::process::Command
// on native targets and raise NotImplementedError on WASM (where the host's
// process_spawn hook is async-only).
//
// Capability gate: "process" or "process.exec"
//
// ABI contract:
//   All functions follow the NaN-boxed u64 calling convention.
//   Errors propagate via the runtime exception mechanism (raise_exception /
//   raise_os_error) – callers must check exception_pending() before
//   interpreting the return value.
//
// Return conventions:
//   molt_subprocess_run   → tuple(returncode:int, stdout:bytes, stderr:bytes)
//   molt_subprocess_check_output → bytes  (stdout)
//   molt_subprocess_check_call   → int    (returncode, always 0 on success)
//   molt_subprocess_pipe / stdout / devnull → int constants

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

use crate::*;

// ---------------------------------------------------------------------------
// stdio mode constants (mirrors process.rs / subprocess.py)
// ---------------------------------------------------------------------------

/// PIPE = -1   (capture into bytes)
const SUBPROCESS_PIPE: i32 = -1;
/// STDOUT = -2  (merge stderr → stdout; only valid for stderr arg)
const SUBPROCESS_STDOUT: i32 = -2;
/// DEVNULL = -3  (redirect to /dev/null)
const SUBPROCESS_DEVNULL: i32 = -3;

// ---------------------------------------------------------------------------
// Constant-returning intrinsics (used by the stdlib subprocess.py shim)
// ---------------------------------------------------------------------------

/// `subprocess.PIPE` → int (-1)
#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_pipe_const() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_int(SUBPROCESS_PIPE as i64).bits() })
}

/// `subprocess.STDOUT` → int (-2)
#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_stdout_const() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_int(SUBPROCESS_STDOUT as i64).bits()
    })
}

/// `subprocess.DEVNULL` → int (-3)
#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_devnull_const() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_int(SUBPROCESS_DEVNULL as i64).bits()
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Decode a runtime list/tuple of strings to a Vec<String> argv.
#[cfg(not(target_arch = "wasm32"))]
fn argv_from_obj(_py: &PyToken<'_>, args_bits: u64) -> Result<Vec<std::ffi::OsString>, String> {
    let obj = obj_from_bits(args_bits);
    // A plain string → invoke via shell.
    if let Some(s) = string_obj_to_owned(obj) {
        return Ok(vec![s.into()]);
    }
    let Some(ptr) = obj.as_ptr() else {
        return Err("args must be a string or sequence".to_string());
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Err("args must be a string or sequence".to_string());
    }
    let Some(elems) = (unsafe {
        crate::object::seq_access::snapshot(
            _py,
            ptr,
            "subprocess argument snapshot allocation failed",
        )
    }) else {
        return Err("subprocess argument snapshot allocation failed".to_string());
    };
    if elems.is_empty() {
        return Err("args must not be empty".to_string());
    }
    let mut out: Vec<std::ffi::OsString> = Vec::with_capacity(elems.len());
    for &bits in elems.iter() {
        if let Some(s) = string_obj_to_owned(obj_from_bits(bits)) {
            out.push(s.into());
            continue;
        }
        // POSIX argv entries are filesystem bytes; callers that apply
        // os.fsencode (os.system's FSConverter) pass them through unchanged.
        #[cfg(unix)]
        if let Some(arg) = argv_bytes(bits) {
            out.push(arg);
            continue;
        }
        return Err("all args elements must be str".to_string());
    }
    Ok(out)
}

#[cfg(unix)]
fn argv_bytes(bits: u64) -> Option<std::ffi::OsString> {
    use std::os::unix::ffi::OsStringExt;
    let ptr = obj_from_bits(bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
        return None;
    }
    let data = unsafe { std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr)) };
    Some(std::ffi::OsString::from_vec(data.to_vec()))
}

/// Build a `std::process::Command` from the common run/call arguments.
///
/// args_bits      – list/tuple of str, or single str
/// cwd_bits       – str or None
/// env_bits       – dict[str,str] or None (None → inherit)
#[cfg(not(target_arch = "wasm32"))]
fn build_command(
    _py: &PyToken<'_>,
    args_bits: u64,
    cwd_bits: u64,
    env_bits: u64,
) -> Result<std::process::Command, u64> {
    let argv = argv_from_obj(_py, args_bits)
        .map_err(|msg| raise_exception::<u64>(_py, BuiltinExc::TypeError, &msg))?;
    if argv.is_empty() {
        return Err(raise_exception::<u64>(
            _py,
            BuiltinExc::ValueError,
            "args must not be empty",
        ));
    }

    let mut cmd = std::process::Command::new(&argv[0]);
    if argv.len() > 1 {
        cmd.args(&argv[1..]);
    }

    // cwd
    if !obj_from_bits(cwd_bits).is_none() {
        let path = path_from_bits(_py, cwd_bits)?;
        cmd.current_dir(path);
    }

    // env
    if !obj_from_bits(env_bits).is_none() {
        match extract_env_dict(_py, env_bits) {
            Ok(entries) => {
                cmd.env_clear();
                for (k, v) in entries {
                    cmd.env(k, v);
                }
            }
            Err(msg) => return Err(raise_exception::<u64>(_py, BuiltinExc::TypeError, &msg)),
        }
    }

    crate::async_rt::process::self_spawn::admit_child(&mut cmd)
        .map_err(|refusal| refusal.raise(_py, &argv[0]))?;
    Ok(cmd)
}

/// Extract a dict[str,str] from runtime bits.
#[cfg(not(target_arch = "wasm32"))]
fn extract_env_dict(_py: &PyToken<'_>, env_bits: u64) -> Result<Vec<(String, String)>, String> {
    let obj = obj_from_bits(env_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("env must be a dict or None".to_string());
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return Err("env must be a dict".to_string());
        }
        let mut out = Vec::with_capacity(crate::dict_len(ptr));
        let mut cursor = 0;
        while let Some(row) = crate::dict_next_entry(ptr, &mut cursor) {
            let k = string_obj_to_owned(obj_from_bits(row.key))
                .ok_or_else(|| "env keys must be str".to_string())?;
            let v = string_obj_to_owned(obj_from_bits(row.value))
                .ok_or_else(|| "env values must be str".to_string())?;
            out.push((k, v));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// molt_subprocess_run
// ---------------------------------------------------------------------------
//
// Signature (Python side):
//   subprocess_run(args, capture_output, timeout, check, input, cwd, env)
//     → tuple(returncode, stdout_bytes, stderr_bytes)
//
// Parameters
//   args_bits          – list[str] or str
//   capture_output_bits – bool (if True: stdout=PIPE, stderr=PIPE)
//   timeout_bits        – float (seconds) or None
//   check_bits          – bool (raise CalledProcessError on non-zero rc)
//   input_bits          – bytes or None  (sent to stdin)
//   cwd_bits            – str or None
//   env_bits            – dict[str,str] or None

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn molt_subprocess_run(
    args_bits: u64,
    capture_output_bits: u64,
    timeout_bits: u64,
    check_bits: u64,
    input_bits: u64,
    cwd_bits: u64,
    env_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_process_capability::<u64>(_py, crate::OperationId::ProcessExec).is_err() {
            return MoltObject::none().bits();
        }

        let capture_output = is_truthy(_py, obj_from_bits(capture_output_bits));
        let check = is_truthy(_py, obj_from_bits(check_bits));

        // Optional timeout in seconds.
        let timeout_secs: Option<f64> = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            to_f64(obj_from_bits(timeout_bits))
        };

        // Optional stdin input bytes.
        let input_data: Option<Vec<u8>> = if obj_from_bits(input_bits).is_none() {
            None
        } else {
            match extract_bytes(_py, input_bits) {
                Ok(b) => Some(b),
                Err(bits) => return bits,
            }
        };

        let mut cmd = match build_command(_py, args_bits, cwd_bits, env_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };

        // stdin
        if input_data.is_some() {
            cmd.stdin(std::process::Stdio::piped());
        } else {
            cmd.stdin(std::process::Stdio::null());
        }

        // stdout / stderr
        if capture_output {
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
        } else {
            cmd.stdout(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => return raise_os_error::<u64>(_py, err, "subprocess_run"),
        };

        // Write stdin if provided.
        if let Some(data) = input_data
            && let Some(mut stdin) = child.stdin.take()
        {
            use std::io::Write;
            let _ = stdin.write_all(&data);
        }

        // Collect output with optional timeout.
        let result = if let Some(secs) = timeout_secs {
            run_with_timeout(child, secs, capture_output)
        } else {
            run_blocking(child, capture_output)
        };

        let (returncode, stdout_bytes, stderr_bytes) = match result {
            Ok(v) => v,
            Err(err) => return raise_os_error::<u64>(_py, err, "subprocess_run"),
        };

        // Raise CalledProcessError if check=True and returncode != 0.
        if check && returncode != 0 {
            let (output, stderr) = if capture_output {
                (Some(stdout_bytes.as_slice()), Some(stderr_bytes.as_slice()))
            } else {
                (None, None)
            };
            return raise_called_process_error(_py, returncode, args_bits, output, stderr);
        }

        // Build return tuple (returncode, stdout_bytes, stderr_bytes).
        let rc_bits = MoltObject::from_int(returncode as i64).bits();
        let stdout_bits = bytes_to_bits(_py, &stdout_bytes);
        let stderr_bits = bytes_to_bits(_py, &stderr_bytes);

        let elems = [rc_bits, stdout_bits, stderr_bits];
        let tup = alloc_tuple(_py, &elems);
        dec_ref_bits(_py, stdout_bits);
        dec_ref_bits(_py, stderr_bits);

        if tup.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(tup).bits()
    })
}

/// Raise `subprocess.CalledProcessError(returncode, cmd, output, stderr)` with
/// the owner's class, so its own `__init__` sets the fields and `__str__`.
#[cfg(not(target_arch = "wasm32"))]
fn raise_called_process_error(
    py: &PyToken<'_>,
    returncode: i32,
    cmd_bits: u64,
    output: Option<&[u8]>,
    stderr: Option<&[u8]>,
) -> u64 {
    use crate::builtins::exceptions::ExceptionValue;
    let optional_bytes = |bytes: Option<&[u8]>| match bytes {
        Some(bytes) => ExceptionValue::adopt(py, bytes_to_bits(py, bytes)),
        None => ExceptionValue::adopt(py, MoltObject::none().bits()),
    };
    let output = optional_bytes(output);
    let stderr = optional_bytes(stderr);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    crate::builtins::exceptions::raise_module_exception_with_args(
        py,
        crate::ModuleExc::SubprocessCalledProcessError,
        &[
            MoltObject::from_int(returncode as i64).bits(),
            cmd_bits,
            output.bits(),
            stderr.bits(),
        ],
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn run_blocking(
    child: std::process::Child,
    capture: bool,
) -> std::io::Result<(i32, Vec<u8>, Vec<u8>)> {
    let output = child.wait_with_output()?;
    let rc = returncode(output.status);
    let stdout = if capture { output.stdout } else { Vec::new() };
    let stderr = if capture { output.stderr } else { Vec::new() };
    Ok((rc, stdout, stderr))
}

/// `subprocess` returncode: the exit code, or `-N` when signal N ended the
/// child (os.system turns `-N` back into the raw wait status N).
#[cfg(not(target_arch = "wasm32"))]
fn returncode(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return -signal;
        }
    }
    status.code().unwrap_or(-1)
}

#[cfg(not(target_arch = "wasm32"))]
fn run_with_timeout(
    child: std::process::Child,
    secs: f64,
    capture: bool,
) -> std::io::Result<(i32, Vec<u8>, Vec<u8>)> {
    // Run in a worker thread so we can enforce a wall-clock deadline.
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = run_blocking(child, capture);
        let _ = tx.send(result);
    });
    let timeout = std::time::Duration::from_secs_f64(secs.max(0.0));
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "subprocess timed out",
        )),
    }
}

/// WASM stub for molt_subprocess_run.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn molt_subprocess_run(
    _args_bits: u64,
    _capture_output_bits: u64,
    _timeout_bits: u64,
    _check_bits: u64,
    _input_bits: u64,
    _cwd_bits: u64,
    _env_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            BuiltinExc::NotImplementedError,
            "subprocess.run is not available on this platform",
        )
    })
}

// ---------------------------------------------------------------------------
// molt_subprocess_check_output
// ---------------------------------------------------------------------------
//
// subprocess.check_output(args, timeout=None, input=None, cwd=None, env=None)
//   → bytes
//
// Raises CalledProcessError on non-zero exit.

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_check_output(
    args_bits: u64,
    timeout_bits: u64,
    input_bits: u64,
    cwd_bits: u64,
    env_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_process_capability::<u64>(_py, crate::OperationId::ProcessExec).is_err() {
            return MoltObject::none().bits();
        }

        let timeout_secs: Option<f64> = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            to_f64(obj_from_bits(timeout_bits))
        };

        let input_data: Option<Vec<u8>> = if obj_from_bits(input_bits).is_none() {
            None
        } else {
            match extract_bytes(_py, input_bits) {
                Ok(b) => Some(b),
                Err(bits) => return bits,
            }
        };

        let mut cmd = match build_command(_py, args_bits, cwd_bits, env_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };

        if input_data.is_some() {
            cmd.stdin(std::process::Stdio::piped());
        } else {
            cmd.stdin(std::process::Stdio::null());
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => return raise_os_error::<u64>(_py, err, "check_output"),
        };

        if let Some(data) = input_data
            && let Some(mut stdin) = child.stdin.take()
        {
            use std::io::Write;
            let _ = stdin.write_all(&data);
        }

        let result = if let Some(secs) = timeout_secs {
            run_with_timeout(child, secs, true)
        } else {
            run_blocking(child, true)
        };

        let (returncode, stdout_bytes, _) = match result {
            Ok(v) => v,
            Err(err) => return raise_os_error::<u64>(_py, err, "check_output"),
        };

        if returncode != 0 {
            return raise_called_process_error(
                _py,
                returncode,
                args_bits,
                Some(stdout_bytes.as_slice()),
                None,
            );
        }

        bytes_to_bits(_py, &stdout_bytes)
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_check_output(
    _args_bits: u64,
    _timeout_bits: u64,
    _input_bits: u64,
    _cwd_bits: u64,
    _env_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            BuiltinExc::NotImplementedError,
            "subprocess.check_output is not available on this platform",
        )
    })
}

// ---------------------------------------------------------------------------
// molt_subprocess_check_call
// ---------------------------------------------------------------------------
//
// subprocess.check_call(args, timeout=None, cwd=None, env=None) → int
//
// Returns 0 on success; raises CalledProcessError on non-zero exit.

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_check_call(
    args_bits: u64,
    timeout_bits: u64,
    cwd_bits: u64,
    env_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_process_capability::<u64>(_py, crate::OperationId::ProcessExec).is_err() {
            return MoltObject::none().bits();
        }

        let timeout_secs: Option<f64> = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            to_f64(obj_from_bits(timeout_bits))
        };

        let mut cmd = match build_command(_py, args_bits, cwd_bits, env_bits) {
            Ok(c) => c,
            Err(bits) => return bits,
        };
        cmd.stdin(std::process::Stdio::inherit());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => return raise_os_error::<u64>(_py, err, "check_call"),
        };

        let result = if let Some(secs) = timeout_secs {
            run_with_timeout(child, secs, false)
        } else {
            run_blocking(child, false)
        };

        let (returncode, _, _) = match result {
            Ok(v) => v,
            Err(err) => return raise_os_error::<u64>(_py, err, "check_call"),
        };

        if returncode != 0 {
            return raise_called_process_error(_py, returncode, args_bits, None, None);
        }

        MoltObject::from_int(0).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_check_call(
    _args_bits: u64,
    _timeout_bits: u64,
    _cwd_bits: u64,
    _env_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            BuiltinExc::NotImplementedError,
            "subprocess.check_call is not available on this platform",
        )
    })
}

// ---------------------------------------------------------------------------
// Internal byte helpers
// ---------------------------------------------------------------------------

/// Extract bytes/bytearray content from runtime bits → Vec<u8>.
fn extract_bytes(_py: &PyToken<'_>, bits: u64) -> Result<Vec<u8>, u64> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_BYTES {
            let len = unsafe { bytes_len(ptr) };
            let data = unsafe { std::slice::from_raw_parts(bytes_data(ptr), len) };
            return Ok(data.to_vec());
        }
        if type_id == TYPE_ID_BYTEARRAY {
            let vec_ref = unsafe { bytearray_vec_ref(ptr) };
            return Ok(vec_ref.clone());
        }
    }
    Err(raise_exception::<u64>(
        _py,
        BuiltinExc::TypeError,
        "input must be bytes or bytearray",
    ))
}

/// Allocate a runtime bytes object from a Rust slice.
fn bytes_to_bits(_py: &PyToken<'_>, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}
