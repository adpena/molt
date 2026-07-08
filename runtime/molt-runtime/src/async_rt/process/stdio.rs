use super::super::generators_async::asyncio_clear_pending_exception;
use crate::*;
#[cfg(not(target_arch = "wasm32"))]
use std::process::{Command, Stdio};

pub(super) const PROCESS_STDIO_INHERIT: i32 = 0;
pub(super) const PROCESS_STDIO_PIPE: i32 = 1;
pub(super) const PROCESS_STDIO_DEVNULL: i32 = 2;
pub(super) const PROCESS_STDIO_STDOUT: i32 = -2;
pub(super) const PROCESS_STDIO_FD_BASE: i32 = 1 << 30;

#[cfg(not(target_arch = "wasm32"))]
const PROCESS_PIPE_MAX_QUEUED_BYTES_ENV: &str = "MOLT_PROCESS_PIPE_MAX_QUEUED_BYTES";

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct NativeProcessStdio {
    pub(super) stdin_stream: u64,
    pub(super) stdout_stream: u64,
    pub(super) stderr_stream: u64,
    pub(super) merged_stdout_reader: Option<os_pipe::PipeReader>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeProcessStdio {
    fn from_modes(stdin_mode: i32, stdout_mode: i32, stderr_mode: i32) -> Self {
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
        let stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
            new_process_pipe_stream()
        } else {
            0
        };
        Self {
            stdin_stream,
            stdout_stream,
            stderr_stream,
            merged_stdout_reader: None,
        }
    }

    pub(super) fn drop_streams(&mut self) {
        drop_stream(&mut self.stdin_stream);
        drop_stream(&mut self.stdout_stream);
        drop_stream(&mut self.stderr_stream);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn drop_stream(stream_bits: &mut u64) {
    if *stream_bits == 0 {
        return;
    }
    unsafe {
        molt_stream_drop(*stream_bits);
    }
    *stream_bits = 0;
}

#[cfg(not(target_arch = "wasm32"))]
fn process_pipe_max_queued_bytes() -> usize {
    std::env::var(PROCESS_PIPE_MAX_QUEUED_BYTES_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(super::super::channels::default_stream_max_queued_bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn new_process_pipe_stream() -> u64 {
    super::super::channels::stream_new_with_byte_budget(0, process_pipe_max_queued_bytes())
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn stdio_from_fd(fd: i32) -> Option<Stdio> {
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

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
enum NativeStdioSlot {
    Stdin,
    Stdout,
    Stderr,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeStdioSlot {
    fn name(self) -> &'static str {
        match self {
            Self::Stdin => "stdin",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn set_command_stdio(cmd: &mut Command, slot: NativeStdioSlot, stdio: Stdio) {
    match slot {
        NativeStdioSlot::Stdin => {
            cmd.stdin(stdio);
        }
        NativeStdioSlot::Stdout => {
            cmd.stdout(stdio);
        }
        NativeStdioSlot::Stderr => {
            cmd.stderr(stdio);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn invalid_fd_error(_py: &PyToken<'_>, slot: NativeStdioSlot) -> u64 {
    raise_exception::<u64>(
        _py,
        "ValueError",
        &format!("invalid {} file descriptor", slot.name()),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_unmerged_stdio(
    _py: &PyToken<'_>,
    cmd: &mut Command,
    slot: NativeStdioSlot,
    mode: i32,
) -> Result<(), u64> {
    match mode {
        PROCESS_STDIO_PIPE => {
            set_command_stdio(cmd, slot, Stdio::piped());
        }
        PROCESS_STDIO_DEVNULL => {
            set_command_stdio(cmd, slot, Stdio::null());
        }
        val if val >= PROCESS_STDIO_FD_BASE => {
            let fd = val - PROCESS_STDIO_FD_BASE;
            let Some(stdio) = stdio_from_fd(fd) else {
                return Err(invalid_fd_error(_py, slot));
            };
            set_command_stdio(cmd, slot, stdio);
        }
        _ => {
            set_command_stdio(cmd, slot, Stdio::inherit());
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_merged_stdout_stderr(
    _py: &PyToken<'_>,
    cmd: &mut Command,
    plan: &mut NativeProcessStdio,
    stdout_mode: i32,
) -> Result<(), u64> {
    match stdout_mode {
        PROCESS_STDIO_PIPE => {
            let (reader, writer) =
                os_pipe::pipe().map_err(|err| raise_os_error::<u64>(_py, err, "pipe"))?;
            let writer_err = writer
                .try_clone()
                .map_err(|err| raise_os_error::<u64>(_py, err, "pipe"))?;
            cmd.stdout(writer);
            cmd.stderr(writer_err);
            plan.merged_stdout_reader = Some(reader);
        }
        PROCESS_STDIO_DEVNULL => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
        val if val >= PROCESS_STDIO_FD_BASE => {
            let fd = val - PROCESS_STDIO_FD_BASE;
            let Some(stdout_stdio) = stdio_from_fd(fd) else {
                return Err(invalid_fd_error(_py, NativeStdioSlot::Stdout));
            };
            let Some(stderr_stdio) = stdio_from_fd(fd) else {
                return Err(invalid_fd_error(_py, NativeStdioSlot::Stderr));
            };
            cmd.stdout(stdout_stdio);
            cmd.stderr(stderr_stdio);
        }
        _ => {
            cmd.stdout(Stdio::inherit());
            cmd.stderr(Stdio::inherit());
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn configure_native_stdio(
    _py: &PyToken<'_>,
    cmd: &mut Command,
    stdin_mode: i32,
    stdout_mode: i32,
    stderr_mode: i32,
) -> Result<NativeProcessStdio, u64> {
    let mut plan = NativeProcessStdio::from_modes(stdin_mode, stdout_mode, stderr_mode);
    let result = (|| {
        apply_unmerged_stdio(_py, cmd, NativeStdioSlot::Stdin, stdin_mode)?;
        if stderr_mode == PROCESS_STDIO_STDOUT {
            apply_merged_stdout_stderr(_py, cmd, &mut plan, stdout_mode)?;
        } else {
            apply_unmerged_stdio(_py, cmd, NativeStdioSlot::Stdout, stdout_mode)?;
            apply_unmerged_stdio(_py, cmd, NativeStdioSlot::Stderr, stderr_mode)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(plan),
        Err(err) => {
            plan.drop_streams();
            Err(err)
        }
    }
}

pub(super) fn process_stdio_mode(_py: &PyToken<'_>, bits: u64, name: &str) -> i32 {
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
