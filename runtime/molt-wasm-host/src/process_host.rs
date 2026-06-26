use super::*;

const PROCESS_POLL_BATCH: usize = 128;
const PROCESS_STDIO_PIPE: i32 = 1;
const PROCESS_STDIO_DEVNULL: i32 = 2;
const PROCESS_STDIO_STDOUT_REDIRECT: i32 = -2;
const PROCESS_STDIO_FD_BASE: i32 = 1 << 30;
const PROCESS_STDIO_STDOUT: i32 = 1;
const PROCESS_STDIO_STDERR: i32 = 2;

pub(super) struct ProcessManager {
    next_id: u64,
    processes: HashMap<u64, ProcessEntry>,
    poll_index: Vec<u64>,
    poll_positions: HashMap<u64, usize>,
    poll_cursor: usize,
    events_tx: mpsc::Sender<ProcessEvent>,
    events_rx: mpsc::Receiver<ProcessEvent>,
}

struct ProcessEntry {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_stream: Option<u64>,
    stderr_stream: Option<u64>,
    exit_code: Option<i32>,
}

enum ProcessEvent {
    Stdout(u64, Vec<u8>),
    Stderr(u64, Vec<u8>),
    StdoutClosed(u64),
    StderrClosed(u64),
}

enum ProcessStreamKind {
    Stdout,
    Stderr,
}

impl ProcessManager {
    pub(super) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            next_id: 1,
            processes: HashMap::new(),
            poll_index: Vec::new(),
            poll_positions: HashMap::new(),
            poll_cursor: 0,
            events_tx: tx,
            events_rx: rx,
        }
    }

    fn alloc_handle(&mut self, pid: u32) -> u64 {
        let handle = if pid != 0 { pid as u64 } else { self.next_id };
        if pid == 0 {
            self.next_id = self.next_id.saturating_add(1);
        }
        handle
    }

    fn poll_track(&mut self, handle: u64) {
        indexed_track(&mut self.poll_index, &mut self.poll_positions, handle);
    }

    fn poll_untrack(&mut self, handle: u64) {
        indexed_untrack(
            &mut self.poll_index,
            &mut self.poll_positions,
            &mut self.poll_cursor,
            handle,
        );
    }

    fn poll_batch_handles(&mut self, max_batch: usize) -> Vec<u64> {
        indexed_next_batch(&self.poll_index, &mut self.poll_cursor, max_batch)
    }
}

fn decode_string_list(buf: &[u8]) -> Result<Vec<String>> {
    if buf.len() < 4 {
        bail!("string list buffer too small");
    }
    let count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let mut out = Vec::with_capacity(count);
    let mut offset = 4;
    for _ in 0..count {
        if offset + 4 > buf.len() {
            bail!("string list truncated");
        }
        let len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;
        let end = offset + len;
        if end > buf.len() {
            bail!("string list truncated");
        }
        let value = std::str::from_utf8(&buf[offset..end])?.to_string();
        out.push(value);
        offset = end;
    }
    Ok(out)
}

fn decode_env(buf: &[u8]) -> Result<(u8, Vec<(String, String)>)> {
    if buf.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mode = buf[0];
    if buf.len() < 5 {
        bail!("env buffer too small");
    }
    let count = u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    let mut offset = 5;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if offset + 4 > buf.len() {
            bail!("env buffer truncated");
        }
        let key_len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;
        let key_end = offset + key_len;
        if key_end > buf.len() {
            bail!("env buffer truncated");
        }
        let key = std::str::from_utf8(&buf[offset..key_end])?.to_string();
        offset = key_end;
        if offset + 4 > buf.len() {
            bail!("env buffer truncated");
        }
        let val_len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;
        let val_end = offset + val_len;
        if val_end > buf.len() {
            bail!("env buffer truncated");
        }
        let value = std::str::from_utf8(&buf[offset..val_end])?.to_string();
        offset = val_end;
        out.push((key, value));
    }
    Ok((mode, out))
}

fn stdio_from_fd(fd: i32) -> Option<Stdio> {
    if fd < 0 {
        return None;
    }
    #[cfg(unix)]
    {
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
        let handle = unsafe { libc::get_osfhandle(fd as libc::c_int) };
        if handle == -1 {
            return None;
        }
        let process = unsafe { GetCurrentProcess() };
        let mut duplicated: HANDLE = std::ptr::null_mut();
        let ok = unsafe {
            DuplicateHandle(
                process,
                handle as HANDLE,
                process,
                &mut duplicated,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            return None;
        }
        let file = unsafe { std::fs::File::from_raw_handle(duplicated as *mut _) };
        Some(Stdio::from(file))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fd;
        None
    }
}

fn spawn_process_reader<R: Read + Send + 'static>(
    mut reader: R,
    tx: mpsc::Sender<ProcessEvent>,
    handle: u64,
    kind: ProcessStreamKind,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(match kind {
                        ProcessStreamKind::Stdout => ProcessEvent::StdoutClosed(handle),
                        ProcessStreamKind::Stderr => ProcessEvent::StderrClosed(handle),
                    });
                    break;
                }
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    let _ = tx.send(match kind {
                        ProcessStreamKind::Stdout => ProcessEvent::Stdout(handle, data),
                        ProcessStreamKind::Stderr => ProcessEvent::Stderr(handle, data),
                    });
                }
                Err(_) => {
                    let _ = tx.send(match kind {
                        ProcessStreamKind::Stdout => ProcessEvent::StdoutClosed(handle),
                        ProcessStreamKind::Stderr => ProcessEvent::StderrClosed(handle),
                    });
                    break;
                }
            }
        }
    });
}

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

pub(super) fn define_process_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
    let process_spawn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         args_ptr: i32,
         args_len: i32,
         env_ptr: i32,
         env_len: i32,
         cwd_ptr: i32,
         cwd_len: i32,
         stdin_mode: i32,
         stdout_mode: i32,
         stderr_mode: i32,
         out_handle_ptr: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let args_buf = match read_bytes(&mut caller, &memory, args_ptr, args_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let args = match decode_string_list(&args_buf) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            if args.is_empty() {
                return -libc::EINVAL;
            }
            let env_mode;
            let env_entries;
            if env_ptr != 0 && env_len > 0 {
                let env_buf = match read_bytes(&mut caller, &memory, env_ptr, env_len) {
                    Ok(buf) => buf,
                    Err(_) => return -libc::EFAULT,
                };
                match decode_env(&env_buf) {
                    Ok((mode, entries)) => {
                        env_mode = mode;
                        env_entries = entries;
                    }
                    Err(_) => return -libc::EINVAL,
                }
            } else {
                env_mode = 0;
                env_entries = Vec::new();
            }
            let cwd = if cwd_ptr != 0 && cwd_len > 0 {
                let cwd_buf = match read_bytes(&mut caller, &memory, cwd_ptr, cwd_len) {
                    Ok(buf) => buf,
                    Err(_) => return -libc::EFAULT,
                };
                match String::from_utf8(cwd_buf) {
                    Ok(val) => Some(val),
                    Err(_) => return -libc::EINVAL,
                }
            } else {
                None
            };

            let mut cmd = Command::new(&args[0]);
            if args.len() > 1 {
                cmd.args(&args[1..]);
            }
            match env_mode {
                1 => {
                    cmd.env_clear();
                    for (key, value) in env_entries {
                        cmd.env(key, value);
                    }
                }
                2 => {
                    for (key, value) in env_entries {
                        cmd.env(key, value);
                    }
                }
                _ => {}
            }
            if let Some(cwd) = cwd {
                cmd.current_dir(cwd);
            }
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
                        return -libc::EBADF;
                    };
                    cmd.stdin(stdio);
                }
                _ => {
                    cmd.stdin(Stdio::inherit());
                }
            }

            let mut merged_stdout_reader: Option<os_pipe::PipeReader> = None;
            if stderr_mode == PROCESS_STDIO_STDOUT_REDIRECT {
                if stdout_mode == PROCESS_STDIO_PIPE {
                    let (reader, writer) = match os_pipe::pipe() {
                        Ok(val) => val,
                        Err(err) => return -map_io_error(&err),
                    };
                    let writer_err = match writer.try_clone() {
                        Ok(val) => val,
                        Err(err) => return -map_io_error(&err),
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
                        return -libc::EBADF;
                    };
                    let Some(stderr_stdio) = stdio_from_fd(fd) else {
                        return -libc::EBADF;
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
                            return -libc::EBADF;
                        };
                        cmd.stdout(stdio);
                    }
                    _ => {
                        cmd.stdout(Stdio::inherit());
                    }
                }
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
                            return -libc::EBADF;
                        };
                        cmd.stderr(stdio);
                    }
                    _ => {
                        cmd.stderr(Stdio::inherit());
                    }
                }
            }

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(err) => return -map_io_error(&err),
            };
            let pid = child.id();
            let handle = {
                let state = caller.data_mut();
                state.process_manager.alloc_handle(pid)
            };

            let exports = match runtime_exports(&mut caller) {
                Ok(exports) => exports,
                Err(_) => return -libc::EFAULT,
            };

            let stdout_stream = if stdout_mode == PROCESS_STDIO_PIPE {
                match call_i64(&exports.stream_new, &mut caller, &[Val::I64(0)]) {
                    Ok(bits) => Some(bits as u64),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
                match call_i64(&exports.stream_new, &mut caller, &[Val::I64(0)]) {
                    Ok(bits) => Some(bits as u64),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let stdin = child.stdin.take();
            {
                let state = caller.data_mut();
                state.process_manager.processes.insert(
                    handle,
                    ProcessEntry {
                        child,
                        stdin,
                        stdout_stream,
                        stderr_stream,
                        exit_code: None,
                    },
                );
                state.process_manager.poll_track(handle);
            }
            if let Some(reader) = merged_stdout_reader.take() {
                let tx = caller.data().process_manager.events_tx.clone();
                spawn_process_reader(reader, tx, handle, ProcessStreamKind::Stdout);
            } else if let Some(stdout) = stdout {
                let tx = caller.data().process_manager.events_tx.clone();
                spawn_process_reader(stdout, tx, handle, ProcessStreamKind::Stdout);
            }
            if let Some(stderr) = stderr {
                let tx = caller.data().process_manager.events_tx.clone();
                spawn_process_reader(stderr, tx, handle, ProcessStreamKind::Stderr);
            }

            if out_handle_ptr != 0 {
                let _ = write_u64(&mut caller, &memory, out_handle_ptr, handle);
            }
            0
        },
    );

    let process_wait = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, _timeout_ms: i64, out_code: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let mut stop_polling = false;
            let code = {
                let entry = match caller
                    .data_mut()
                    .process_manager
                    .processes
                    .get_mut(&(handle as u64))
                {
                    Some(entry) => entry,
                    None => return -libc::EBADF,
                };
                if entry.exit_code.is_none() {
                    match entry.child.try_wait() {
                        Ok(Some(status)) => {
                            entry.exit_code = Some(exit_code_from_status(status));
                            stop_polling = true;
                        }
                        Ok(None) => {}
                        Err(err) => return -map_io_error(&err),
                    }
                }
                if entry.exit_code.is_some() {
                    stop_polling = true;
                }
                entry.exit_code
            };
            if stop_polling {
                caller
                    .data_mut()
                    .process_manager
                    .poll_untrack(handle as u64);
            }
            let Some(code) = code else {
                return -libc::EWOULDBLOCK;
            };
            if out_code != 0 {
                let _ = write_bytes(&mut caller, &memory, out_code, &code.to_le_bytes());
            }
            if let Some(func) = caller
                .get_export("molt_process_host_notify")
                .and_then(Extern::into_func)
            {
                let _ = func.call(&mut caller, &[Val::I64(handle), Val::I32(code)], &mut []);
            }
            0
        },
    );

    let process_kill = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller
                .data_mut()
                .process_manager
                .processes
                .get_mut(&(handle as u64))
            {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            match entry.child.kill() {
                Ok(_) => 0,
                Err(err) => -map_io_error(&err),
            }
        },
    );

    let process_terminate = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller
                .data_mut()
                .process_manager
                .processes
                .get_mut(&(handle as u64))
            {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            #[cfg(unix)]
            {
                let pid = entry.child.id() as i32;
                let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
                if rc != 0 {
                    return -map_io_error(&std::io::Error::last_os_error());
                }
                0
            }
            #[cfg(not(unix))]
            {
                match entry.child.kill() {
                    Ok(_) => 0,
                    Err(err) => -map_io_error(&err),
                }
            }
        },
    );

    let process_write = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, data_ptr: i32, len: i64| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let len_i32 = i32::try_from(len).unwrap_or(0);
            if len_i32 <= 0 {
                return 0;
            }
            let buf = match read_bytes(&mut caller, &memory, data_ptr, len_i32) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let entry = match caller
                .data_mut()
                .process_manager
                .processes
                .get_mut(&(handle as u64))
            {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            let Some(stdin) = entry.stdin.as_mut() else {
                return -libc::EPIPE;
            };
            if let Err(err) = stdin.write_all(&buf) {
                return -map_io_error(&err);
            }
            if let Err(err) = stdin.flush() {
                return -map_io_error(&err);
            }
            0
        },
    );

    let process_close_stdin = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller
                .data_mut()
                .process_manager
                .processes
                .get_mut(&(handle as u64))
            {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            entry.stdin = None;
            0
        },
    );

    let process_stdio = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, which: i32, out_stream: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            let stream_bits = match caller
                .data()
                .process_manager
                .processes
                .get(&(handle as u64))
            {
                Some(entry) => match which {
                    PROCESS_STDIO_STDOUT => entry.stdout_stream,
                    PROCESS_STDIO_STDERR => entry.stderr_stream,
                    _ => None,
                },
                None => return -libc::EBADF,
            };
            let Some(bits) = stream_bits else {
                return -libc::EINVAL;
            };
            if out_stream != 0 {
                let _ = write_u64(&mut caller, &memory, out_stream, bits);
            }
            0
        },
    );

    let process_poll = Func::wrap(&mut *store, |mut caller: Caller<'_, HostState>| -> i32 {
        let memory = match ensure_memory(&mut caller) {
            Ok(mem) => mem,
            Err(_) => return -libc::EFAULT,
        };
        let exports = match runtime_exports(&mut caller) {
            Ok(exports) => exports,
            Err(_) => return -libc::EFAULT,
        };
        let mut events = Vec::new();
        while let Ok(event) = caller.data_mut().process_manager.events_rx.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                ProcessEvent::Stdout(handle, data) => {
                    let stream_bits = caller
                        .data()
                        .process_manager
                        .processes
                        .get(&handle)
                        .and_then(|entry| entry.stdout_stream);
                    if let Some(stream_bits) = stream_bits {
                        let _ =
                            send_stream_frame(&mut caller, &exports, &memory, stream_bits, &data);
                    }
                }
                ProcessEvent::Stderr(handle, data) => {
                    let stream_bits = caller
                        .data()
                        .process_manager
                        .processes
                        .get(&handle)
                        .and_then(|entry| entry.stderr_stream);
                    if let Some(stream_bits) = stream_bits {
                        let _ =
                            send_stream_frame(&mut caller, &exports, &memory, stream_bits, &data);
                    }
                }
                ProcessEvent::StdoutClosed(handle) => {
                    let stream_bits = caller
                        .data()
                        .process_manager
                        .processes
                        .get(&handle)
                        .and_then(|entry| entry.stdout_stream);
                    if let Some(stream_bits) = stream_bits {
                        let _ = exports.stream_close.call(
                            &mut caller,
                            &[Val::I64(stream_bits as i64)],
                            &mut [],
                        );
                    }
                }
                ProcessEvent::StderrClosed(handle) => {
                    let stream_bits = caller
                        .data()
                        .process_manager
                        .processes
                        .get(&handle)
                        .and_then(|entry| entry.stderr_stream);
                    if let Some(stream_bits) = stream_bits {
                        let _ = exports.stream_close.call(
                            &mut caller,
                            &[Val::I64(stream_bits as i64)],
                            &mut [],
                        );
                    }
                }
            }
        }
        let mut exited = Vec::new();
        {
            let state = caller.data_mut();
            let budget = state
                .process_manager
                .poll_index
                .len()
                .min(PROCESS_POLL_BATCH);
            let handles = state.process_manager.poll_batch_handles(budget);
            for handle in handles {
                let mut stop_polling = false;
                let mut exit_code = None;
                if let Some(entry) = state.process_manager.processes.get_mut(&handle) {
                    if entry.exit_code.is_none() {
                        if let Ok(Some(status)) = entry.child.try_wait() {
                            let code = exit_code_from_status(status);
                            entry.exit_code = Some(code);
                            exit_code = Some(code);
                            stop_polling = true;
                        }
                    } else {
                        stop_polling = true;
                    }
                } else {
                    stop_polling = true;
                }
                if let Some(code) = exit_code {
                    exited.push((handle, code));
                }
                if stop_polling {
                    state.process_manager.poll_untrack(handle);
                }
            }
        }
        if !exited.is_empty()
            && let Some(func) = caller
                .get_export("molt_process_host_notify")
                .and_then(Extern::into_func)
        {
            for (handle, code) in exited {
                let _ = func.call(
                    &mut caller,
                    &[Val::I64(handle as i64), Val::I32(code)],
                    &mut [],
                );
            }
        }
        0
    });

    linker.define(&mut *store, "env", "molt_process_spawn_host", process_spawn)?;
    linker.define(&mut *store, "env", "molt_process_wait_host", process_wait)?;
    linker.define(&mut *store, "env", "molt_process_kill_host", process_kill)?;
    linker.define(
        &mut *store,
        "env",
        "molt_process_terminate_host",
        process_terminate,
    )?;
    linker.define(&mut *store, "env", "molt_process_write_host", process_write)?;
    linker.define(
        &mut *store,
        "env",
        "molt_process_close_stdin_host",
        process_close_stdin,
    )?;
    linker.define(&mut *store, "env", "molt_process_stdio_host", process_stdio)?;
    linker.define(&mut *store, "env", "molt_process_host_poll", process_poll)?;
    Ok(())
}
