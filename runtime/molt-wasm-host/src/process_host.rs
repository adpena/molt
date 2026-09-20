use super::*;

/// One owned pipe handle; reader cancellation never depends on observing EOF.
pub(super) struct HostPipeReader(std::fs::File);

impl HostPipeReader {
    #[cfg(unix)]
    pub(super) fn new(reader: impl Into<std::os::fd::OwnedFd>) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        let owned: std::os::fd::OwnedFd = reader.into();
        let file = std::fs::File::from(owned);
        let fd = file.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(file))
    }

    #[cfg(windows)]
    pub(super) fn new(
        reader: impl Into<std::os::windows::io::OwnedHandle>,
    ) -> std::io::Result<Self> {
        let owned: std::os::windows::io::OwnedHandle = reader.into();
        Ok(Self(std::fs::File::from(owned)))
    }
}

impl Read for HostPipeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE;
            use windows_sys::Win32::System::Pipes::PeekNamedPipe;
            // Anonymous child pipes are synchronous Windows handles. Peeking is
            // nonblocking; this owner is the only reader, so available bytes
            // cannot be consumed between the peek and the bounded read.
            let mut available = 0;
            let ok = unsafe {
                PeekNamedPipe(
                    self.0.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                    return Ok(0);
                }
                return Err(err);
            }
            if available == 0 {
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            let len = buf.len().min(available as usize);
            return self.0.read(&mut buf[..len]);
        }
        #[cfg(not(windows))]
        self.0.read(buf)
    }
}

/// The owner retains both cancellation and join custody. Reading remains
/// concurrent with synchronous guest stdin writes, avoiding duplex pipe stalls.
pub(super) struct HostReaderTask {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}

#[derive(Debug)]
struct HostReaderCancelled;

impl std::fmt::Display for HostReaderCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("host pipe reader cancelled by owner")
    }
}

impl std::error::Error for HostReaderCancelled {}

pub(super) fn is_reader_cancelled(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.is::<HostReaderCancelled>())
}

struct CancellablePipeReader {
    pipe: HostPipeReader,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl Read for CancellablePipeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    HostReaderCancelled,
                ));
            }
            match self.pipe.read(buf) {
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                result => return result,
            }
        }
    }
}

impl HostReaderTask {
    pub(super) fn spawn(
        name: &str,
        pipe: HostPipeReader,
        consume: impl FnOnce(Box<dyn Read + Send>) -> Result<()> + Send + 'static,
    ) -> std::io::Result<Self> {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = CancellablePipeReader {
            pipe,
            cancelled: Arc::clone(&cancelled),
        };
        let thread = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || consume(Box::new(reader)))?;
        Ok(Self {
            cancelled,
            thread: Some(thread),
        })
    }

    pub(super) fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn is_finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }

    pub(super) fn close(&mut self, deadline: Instant) -> Result<()> {
        self.cancel();
        while let Some(thread) = self.thread.as_ref() {
            if thread.is_finished() {
                let thread = self.thread.take().expect("owned reader task");
                return thread
                    .join()
                    .map_err(|_| wasmtime::Error::msg("host pipe reader panicked"))?;
            }
            if Instant::now() >= deadline {
                bail!("host pipe reader did not stop before close deadline");
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
}

impl Drop for HostReaderTask {
    fn drop(&mut self) {
        // Managers own the shared wait budget. Drop may cancel/reap an already
        // finished task, but cannot add another per-reader wait to that budget.
        if let Err(err) = self.close(Instant::now()) {
            eprintln!("host reader fallback cleanup failed: {err:#}");
        }
    }
}

/// Reap only this owned Child; never wait without a deadline or address a PID.
pub(super) fn reap_owned_child(child: &mut Child, deadline: Instant) -> Result<()> {
    loop {
        if child
            .try_wait()
            .context("query owned child exit")?
            .is_some()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "owned child {} is still running at host close deadline",
                child.id()
            );
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn stop_owned_child(child: &mut Child) -> Result<()> {
    if child
        .try_wait()
        .context("query owned child before close")?
        .is_none()
    {
        // The runtime already performed its semantic termination. The finite
        // host is the final backing-resource owner, including failed teardown.
        if let Err(err) = child.kill() {
            if child
                .try_wait()
                .context("query child after kill failure")?
                .is_none()
            {
                return Err(err).context("stop owned child during host close");
            }
        }
    }
    Ok(())
}

pub(super) fn close_owned_child(child: &mut Child, deadline: Instant) -> Result<()> {
    stop_owned_child(child)?;
    reap_owned_child(child, deadline)
}

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
    events_tx: std::sync::mpsc::Sender<ProcessEvent>,
    events_rx: std::sync::mpsc::Receiver<ProcessEvent>,
}

struct ProcessEntry {
    child: Child,
    stdin: Option<ChildStdin>,
    readers: Vec<HostReaderTask>,
    stdout_stream: Option<u64>,
    stderr_stream: Option<u64>,
    exit_code: Option<i32>,
}

enum ProcessEvent {
    Stdout(u64, Vec<u8>),
    Stderr(u64, Vec<u8>),
    StdoutClosed(u64),
    StderrClosed(u64),
    ReadError {
        handle: u64,
        stdout: bool,
        error: std::io::Error,
    },
}

impl ProcessManager {
    pub(super) fn new() -> Self {
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        Self {
            next_id: 1,
            processes: HashMap::new(),
            poll_index: Vec::new(),
            poll_positions: HashMap::new(),
            poll_cursor: 0,
            events_tx,
            events_rx,
        }
    }

    /// Runtime process_registry owns termination policy. This phase closes host
    /// handles and reaps its children, without guest callbacks. A still-live
    /// backing child is stopped on abort or incomplete runtime termination.
    /// Failed custody is retained so the owner can retry or report it.
    pub(super) fn close(&mut self) -> Result<()> {
        self.poll_index.clear();
        self.poll_positions.clear();
        self.poll_cursor = 0;
        let mut handles: Vec<_> = self.processes.keys().copied().collect();
        handles.sort_unstable();
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut result = Ok(());
        for handle in &handles {
            let entry = self
                .processes
                .get_mut(handle)
                .expect("owned process handle");
            entry.stdin.take();
            for reader in &entry.readers {
                reader.cancel();
            }
            entry.stdout_stream = None;
            entry.stderr_stream = None;
            // Stop the complete owned cohort before spending the shared reap
            // budget, so one slow child cannot delay another child's stop.
            result = preserve_application_and_cleanup_result(
                result,
                stop_owned_child(&mut entry.child)
                    .with_context(|| format!("stop process {handle}")),
            );
        }
        for handle in handles {
            let entry = self
                .processes
                .get_mut(&handle)
                .expect("owned process handle");
            let mut closed = true;
            for reader in &mut entry.readers {
                let reader_result = reader
                    .close(deadline)
                    .with_context(|| format!("close process {handle} reader"));
                if reader_result.is_err() {
                    closed = false;
                }
                result = preserve_application_and_cleanup_result(result, reader_result);
            }
            let reaped = reap_owned_child(&mut entry.child, deadline)
                .with_context(|| format!("reap process {handle}"));
            if reaped.is_err() {
                closed = false;
            }
            result = preserve_application_and_cleanup_result(result, reaped);
            if closed {
                self.processes.remove(&handle);
            }
        }
        // Disconnect old event senders and drop the queued guest stream data;
        // reader cancellation never needs a successful send to make progress.
        let (tx, rx) = std::sync::mpsc::channel();
        self.events_tx = tx;
        self.events_rx = rx;
        result.context("host process cleanup")
    }

    fn read_events(&mut self) -> Vec<ProcessEvent> {
        self.events_rx
            .try_iter()
            .take(PROCESS_POLL_BATCH * 2)
            .collect()
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

impl Drop for ProcessManager {
    fn drop(&mut self) {
        if let Err(err) = self.close() {
            // Normal owners must use explicit close to propagate this error.
            // Unwinding still stops only this manager's held child handles.
            eprintln!("process manager fallback cleanup failed: {err:#}");
        }
    }
}

fn spawn_process_reader(
    pipe: HostPipeReader,
    tx: std::sync::mpsc::Sender<ProcessEvent>,
    handle: u64,
    stdout: bool,
) -> std::io::Result<HostReaderTask> {
    HostReaderTask::spawn("molt-process-output", pipe, move |mut reader| {
        let mut buf = [0; 8192];
        let mut outcome = Ok(());
        loop {
            let event = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdout {
                        ProcessEvent::Stdout(handle, buf[..n].to_vec())
                    } else {
                        ProcessEvent::Stderr(handle, buf[..n].to_vec())
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) if is_reader_cancelled(&err) => break,
                Err(err) => {
                    let _ = tx.send(ProcessEvent::ReadError {
                        handle,
                        stdout,
                        error: std::io::Error::new(err.kind(), err.to_string()),
                    });
                    let stream = if stdout { "stdout" } else { "stderr" };
                    outcome = Err(err).with_context(|| format!("process {handle} {stream} reader"));
                    break;
                }
            };
            if tx.send(event).is_err() {
                return Ok(());
            }
        }
        // EOF remains ordered after all data even when exit was already reaped.
        let _ = tx.send(if stdout {
            ProcessEvent::StdoutClosed(handle)
        } else {
            ProcessEvent::StderrClosed(handle)
        });
        outcome
    })
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

            let exports = match runtime_exports(&mut caller) {
                Ok(exports) => exports,
                Err(_) => return -libc::EFAULT,
            };

            let stdout_stream = if stdout_mode == PROCESS_STDIO_PIPE {
                match new_stream(&mut caller, &exports, 0) {
                    Ok(bits) => Some(bits),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };
            let stderr_stream = if stderr_mode == PROCESS_STDIO_PIPE {
                match new_stream(&mut caller, &exports, 0) {
                    Ok(bits) => Some(bits),
                    Err(_) => return -libc::EFAULT,
                }
            } else {
                None
            };

            // Resolve all guest setup before acquiring a child. Once spawned,
            // register its owned handle before any further fallible operation.
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(err) => return -map_io_error(&err),
            };
            let handle = caller.data_mut().process_manager.alloc_handle(child.id());

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
                        readers: Vec::new(),
                        stdout_stream,
                        stderr_stream,
                        exit_code: None,
                    },
                );
                state.process_manager.poll_track(handle);
            }
            let stdout = if let Some(reader) = merged_stdout_reader.take() {
                HostPipeReader::new(reader).map(Some)
            } else {
                stdout.map(HostPipeReader::new).transpose()
            };
            let stdout = match stdout {
                Ok(reader) => reader,
                Err(err) => return -map_io_error(&err),
            };
            let stderr = match stderr.map(HostPipeReader::new).transpose() {
                Ok(reader) => reader,
                Err(err) => return -map_io_error(&err),
            };
            for (pipe, is_stdout) in [(stdout, true), (stderr, false)] {
                if let Some(pipe) = pipe {
                    let tx = caller.data().process_manager.events_tx.clone();
                    let reader = match spawn_process_reader(pipe, tx, handle, is_stdout) {
                        Ok(reader) => reader,
                        Err(err) => return -map_io_error(&err),
                    };
                    caller
                        .data_mut()
                        .process_manager
                        .processes
                        .get_mut(&handle)
                        .expect("spawned child registered before pipe setup")
                        .readers
                        .push(reader);
                }
            }

            if out_handle_ptr != 0 {
                if write_u64(&mut caller, &memory, out_handle_ptr, handle).is_err() {
                    return -libc::EFAULT;
                }
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
        let events = caller.data_mut().process_manager.read_events();
        let mut read_error = None;
        for event in events {
            match event {
                ProcessEvent::ReadError {
                    handle,
                    stdout,
                    error,
                } => {
                    let stream = if stdout { "stdout" } else { "stderr" };
                    eprintln!("process {handle} {stream} reader failed: {error}");
                    read_error.get_or_insert(map_io_error(&error));
                }
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
        read_error.map_or(0, |errno| -errno)
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

#[cfg(test)]
mod host_resource_tests {
    use super::*;

    #[test]
    fn host_pipe_reads_available_bytes_and_reports_eof() {
        let (pipe, mut writer) = os_pipe::pipe().unwrap();
        let mut reader = HostPipeReader::new(pipe).unwrap();
        let mut buf = [0; 8];
        assert_eq!(
            reader.read(&mut buf).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        writer.write_all(b"ready").unwrap();
        assert_eq!(reader.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], b"ready");
        drop(writer);
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn owned_reader_cancels_even_when_pipe_writer_remains_open() {
        let (pipe, _inherited_writer) = os_pipe::pipe().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut task = HostReaderTask::spawn(
            "test-owned-reader",
            HostPipeReader::new(pipe).unwrap(),
            move |mut reader| {
                let result = reader.read(&mut [0; 1]);
                tx.send(result.unwrap_err().kind()).unwrap();
                Ok(())
            },
        )
        .unwrap();
        task.close(Instant::now() + Duration::from_secs(1)).unwrap();
        assert!(task.thread.is_none());
        assert_eq!(
            rx.try_recv().unwrap(),
            std::io::ErrorKind::ConnectionAborted
        );
        task.close(Instant::now()).unwrap();
    }

    #[test]
    fn reader_panic_is_returned_by_explicit_close() {
        let (pipe, _writer) = os_pipe::pipe().unwrap();
        let mut task = HostReaderTask::spawn(
            "test-reader-panic",
            HostPipeReader::new(pipe).unwrap(),
            |_| {
                panic!("reader failure fixture");
            },
        )
        .unwrap();
        assert!(task.close(Instant::now() + Duration::from_secs(1)).is_err());
        assert!(task.thread.is_none());
    }

    #[test]
    fn terminal_reader_io_error_survives_without_event_poll() {
        let (pipe, _writer) = os_pipe::pipe().unwrap();
        let mut task = HostReaderTask::spawn(
            "test-reader-error",
            HostPipeReader::new(pipe).unwrap(),
            |_| {
                Err(
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, "reader fixture")
                        .into(),
                )
            },
        )
        .unwrap();
        let error = task
            .close(Instant::now() + Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(task.thread.is_none());
        task.close(Instant::now()).unwrap();
    }

    #[test]
    fn empty_process_manager_close_is_idempotent() {
        let mut manager = ProcessManager::new();
        manager.close().unwrap();
        manager.close().unwrap();
        assert!(manager.processes.is_empty());
        assert!(manager.read_events().is_empty());
    }
}
