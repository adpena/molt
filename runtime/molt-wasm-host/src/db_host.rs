use super::process_host::{HostPipeReader, HostReaderTask, close_owned_child, is_reader_cancelled};
use super::*;

fn db_cancel_track(state: &mut HostState, req_id: u64) {
    indexed_track(
        &mut state.db_cancel_index,
        &mut state.db_cancel_positions,
        req_id,
    );
}

fn db_cancel_untrack(state: &mut HostState, req_id: u64) {
    indexed_untrack(
        &mut state.db_cancel_index,
        &mut state.db_cancel_positions,
        &mut state.db_cancel_cursor,
        req_id,
    );
}

fn write_frame(mut writer: impl Write, payload: &[u8]) -> Result<()> {
    let len = payload.len();
    if len > u32::MAX as usize {
        bail!("frame too large: {len}");
    }
    let header = (len as u32).to_le_bytes();
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    Ok(())
}

#[derive(Deserialize)]
struct WorkerEnvelope {
    request_id: Option<u64>,
    status: Option<String>,
    codec: Option<String>,
    payload_b64: Option<String>,
    error: Option<String>,
    metrics: Option<JsonValue>,
}

struct WorkerResponse {
    request_id: u64,
    status: String,
    codec: String,
    payload: Vec<u8>,
    error: Option<String>,
    metrics: Option<JsonValue>,
}

pub(super) struct PendingDbRequest {
    stream_bits: u64,
    token_id: u64,
    cancel_sent: bool,
}

enum WorkerError {
    Unavailable(wasmtime::Error),
    SendFailed(wasmtime::Error),
}

enum WorkerMessage {
    Response(WorkerResponse),
    Error(wasmtime::Error),
}

fn decode_worker_frame(frame: &[u8]) -> Result<WorkerResponse> {
    let envelope: WorkerEnvelope = serde_json::from_slice(frame)?;
    let request_id = envelope.request_id.unwrap_or(0);
    let status = envelope
        .status
        .unwrap_or_else(|| "InternalError".to_string());
    let codec = envelope.codec.unwrap_or_else(|| "raw".to_string());
    let payload = match envelope.payload_b64 {
        Some(encoded) => STANDARD.decode(encoded)?,
        None => Vec::new(),
    };
    Ok(WorkerResponse {
        request_id,
        status,
        codec,
        payload,
        error: envelope.error,
        metrics: envelope.metrics,
    })
}

fn read_frame(mut reader: impl Read) -> Result<Option<Vec<u8>>> {
    let mut header = [0; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let size = u32::from_le_bytes(header) as usize;
    if size > MAX_DB_FRAME_SIZE {
        bail!("worker frame too large: {size}");
    }
    let mut payload = vec![0; size];
    reader.read_exact(&mut payload)?;
    Ok(Some(payload))
}

fn spawn_db_reader(
    pipe: HostPipeReader,
) -> std::io::Result<(HostReaderTask, std::sync::mpsc::Receiver<WorkerMessage>)> {
    let (tx, responses) = std::sync::mpsc::channel();
    let task = HostReaderTask::spawn("molt-db-output", pipe, move |reader| {
        let mut reader = std::io::BufReader::new(reader);
        loop {
            let response = read_frame(&mut reader)
                .and_then(|frame| frame.map(|frame| decode_worker_frame(&frame)).transpose());
            match response {
                Ok(Some(response)) => {
                    if tx.send(WorkerMessage::Response(response)).is_err() {
                        return Ok(());
                    }
                }
                Ok(None) => return Ok(()),
                Err(err)
                    if err
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(is_reader_cancelled) =>
                {
                    return Ok(());
                }
                Err(err) => {
                    // Guest reporting owns a diagnostic copy. The reader task
                    // retains the original typed terminal error for close.
                    let _ = tx.send(WorkerMessage::Error(wasmtime::Error::msg(format!(
                        "{err:#}"
                    ))));
                    return Err(err).context("database response reader");
                }
            }
        }
    })?;
    Ok((task, responses))
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    struct FragmentedReader(std::io::Cursor<Vec<u8>>);

    impl Read for FragmentedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let size = buffer.len().min(2);
            self.0.read(&mut buffer[..size])
        }
    }

    #[test]
    fn worker_frame_survives_fragmented_header_and_payload() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, b"first").unwrap();
        write_frame(&mut bytes, b"second").unwrap();
        let mut reader = FragmentedReader(std::io::Cursor::new(bytes));
        assert_eq!(read_frame(&mut reader).unwrap().unwrap(), b"first");
        assert_eq!(read_frame(&mut reader).unwrap().unwrap(), b"second");
        assert!(read_frame(&mut reader).unwrap().is_none());
    }

    #[test]
    fn worker_rejects_oversized_frame_before_reading_payload() {
        let bytes = ((MAX_DB_FRAME_SIZE + 1) as u32).to_le_bytes();
        assert!(
            read_frame(&bytes[..])
                .unwrap_err()
                .to_string()
                .contains("frame too large")
        );
    }

    #[test]
    fn worker_rejects_truncated_header_and_payload() {
        assert!(read_frame(&[1_u8, 0][..]).is_err());
        assert!(read_frame(&[3_u8, 0, 0, 0, b'x'][..]).is_err());
    }

    #[test]
    fn worker_rejects_invalid_response_envelope() {
        assert!(decode_worker_frame(b"not json").is_err());
        assert!(decode_worker_frame(br#"{"request_id":1,"payload_b64":"!invalid!"}"#).is_err());
    }

    fn finish_reader_input(task: &HostReaderTask) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !task.is_finished() {
            assert!(
                Instant::now() < deadline,
                "reader did not consume finite fixture"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn database_reader_retains_invalid_frame_error_without_guest_poll() {
        let (pipe, mut writer) = os_pipe::pipe().unwrap();
        write_frame(&mut writer, b"invalid json").unwrap();
        drop(writer);
        let (mut task, _unpolled_responses) =
            spawn_db_reader(HostPipeReader::new(pipe).unwrap()).unwrap();
        finish_reader_input(&task);
        let error = task.close(Instant::now()).unwrap_err();
        assert!(error.downcast_ref::<serde_json::Error>().is_some());
    }

    #[test]
    fn database_reader_retains_truncation_without_guest_poll() {
        let (pipe, mut writer) = os_pipe::pipe().unwrap();
        writer.write_all(&[3, 0, 0, 0, b'x']).unwrap();
        drop(writer);
        let (mut task, _unpolled_responses) =
            spawn_db_reader(HostPipeReader::new(pipe).unwrap()).unwrap();
        finish_reader_input(&task);
        let error = task.close(Instant::now()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn database_reader_delivers_completed_frames_before_channel_eof() {
        let (pipe, mut writer) = os_pipe::pipe().unwrap();
        for request_id in [1, 2] {
            let payload =
                serde_json::to_vec(&serde_json::json!({"request_id":request_id,"status":"Ok"}))
                    .unwrap();
            write_frame(&mut writer, &payload).unwrap();
        }
        drop(writer);
        let (mut task, responses) = spawn_db_reader(HostPipeReader::new(pipe).unwrap()).unwrap();
        finish_reader_input(&task);
        task.close(Instant::now()).unwrap();
        for expected in [1, 2] {
            let WorkerMessage::Response(response) = responses.try_recv().unwrap() else {
                panic!("completed frame replaced by failure");
            };
            assert_eq!(response.request_id, expected);
        }
        assert!(matches!(
            responses.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn database_reader_owner_cancellation_is_not_a_terminal_failure() {
        let (pipe, _writer) = os_pipe::pipe().unwrap();
        let (mut task, responses) = spawn_db_reader(HostPipeReader::new(pipe).unwrap()).unwrap();
        task.close(Instant::now() + Duration::from_secs(1)).unwrap();
        assert!(matches!(
            responses.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }
}

fn map_worker_status(status: &str) -> &'static str {
    match status {
        "Ok" => "ok",
        "InvalidInput" => "invalid_input",
        "Busy" => "busy",
        "Timeout" => "timeout",
        "Cancelled" => "cancelled",
        "InternalError" => "internal_error",
        _ => "internal_error",
    }
}

pub(super) struct DbWorker {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: Option<HostReaderTask>,
    responses: std::sync::mpsc::Receiver<WorkerMessage>,
    next_id: u64,
    closed: bool,
}

impl DbWorker {
    fn new() -> Result<Self> {
        let cmd = resolve_worker_cmd()?;
        let mut command = Command::new(&cmd[0]);
        command.args(&cmd[1..]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command.envs(env::vars());
        let (_, responses) = std::sync::mpsc::channel();
        let mut worker = Self {
            child: command.spawn().context("spawn molt-worker")?,
            stdin: None,
            reader: None,
            responses,
            next_id: 1,
            closed: false,
        };
        let setup = (|| -> Result<()> {
            worker.stdin = Some(worker.child.stdin.take().context("missing worker stdin")?);
            let stdout = worker
                .child
                .stdout
                .take()
                .context("missing worker stdout")?;
            let pipe = HostPipeReader::new(stdout).context("configure worker output")?;
            let (reader, responses) =
                spawn_db_reader(pipe).context("start database output reader")?;
            worker.reader = Some(reader);
            worker.responses = responses;
            Ok(())
        })();
        if let Err(err) = setup {
            return match worker.close() {
                Ok(()) => Err(err),
                Err(cleanup) => {
                    Err(err.context(format!("worker setup cleanup also failed: {cleanup:#}")))
                }
            };
        }
        Ok(worker)
    }

    pub(super) fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_millis(250);
        if let Some(reader) = &self.reader {
            reader.cancel();
        }
        let mut result =
            close_owned_child(&mut self.child, deadline).context("close database child");
        if let Some(reader) = &mut self.reader {
            result = preserve_application_and_cleanup_result(
                result,
                reader.close(deadline).context("close database reader"),
            );
        }
        let (_, responses) = std::sync::mpsc::channel();
        self.responses = responses;
        result.context("close database worker")?;
        self.reader.take();
        self.closed = true;
        Ok(())
    }

    fn poll_response(&mut self) -> Result<Option<WorkerResponse>> {
        match self.responses.try_recv() {
            Ok(WorkerMessage::Response(response)) => Ok(Some(response)),
            Ok(WorkerMessage::Error(err)) => Err(err),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                bail!("database worker output disconnected")
            }
        }
    }

    fn send_request(&mut self, entry: &str, payload: &[u8], timeout_ms: u64) -> Result<u64> {
        let request_id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let payload_b64 = STANDARD.encode(payload);
        let msg = serde_json::json!({
            "request_id": request_id,
            "entry": entry,
            "timeout_ms": timeout_ms,
            "codec": "msgpack",
            "payload_b64": payload_b64,
        });
        let bytes = serde_json::to_vec(&msg)?;
        let stdin = self
            .stdin
            .as_mut()
            .context("database worker input is closed")?;
        write_frame(stdin, &bytes)?;
        Ok(request_id)
    }
}

impl Drop for DbWorker {
    fn drop(&mut self) {
        if let Err(err) = self.close() {
            eprintln!("database worker fallback cleanup failed: {err:#}");
        }
    }
}

fn send_worker_cancel(stdin: &mut ChildStdin, target_id: u64) -> Result<()> {
    let cancel_payload = serde_json::json!({ "request_id": target_id });
    let cancel_bytes = serde_json::to_vec(&cancel_payload)?;
    let payload_b64 = STANDARD.encode(cancel_bytes);
    let msg = serde_json::json!({
        "request_id": 0,
        "entry": "__cancel__",
        "timeout_ms": 0,
        "codec": "json",
        "payload_b64": payload_b64,
    });
    let bytes = serde_json::to_vec(&msg)?;
    write_frame(stdin, &bytes)?;
    Ok(())
}

fn deliver_worker_response(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    response: WorkerResponse,
) {
    let status = map_worker_status(&response.status);
    if status != "ok" {
        let message = response
            .error
            .clone()
            .unwrap_or_else(|| response.status.clone());
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            None,
            Some(&message),
            response.metrics.as_ref(),
        );
        let _ = exports
            .stream_close
            .call(caller, &[Val::I64(stream_bits as i64)], &mut []);
        return;
    }

    if response.codec == "arrow_ipc" {
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            None,
            None,
            response.metrics.as_ref(),
        );
        if !response.payload.is_empty() {
            let _ = send_stream_frame(caller, exports, memory, stream_bits, &response.payload);
        }
    } else {
        let _ = send_stream_header(
            caller,
            exports,
            memory,
            stream_bits,
            status,
            response.codec.as_str(),
            Some(&response.payload),
            None,
            response.metrics.as_ref(),
        );
    }
    let _ = exports
        .stream_close
        .call(caller, &[Val::I64(stream_bits as i64)], &mut []);
}

fn fail_pending_requests(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    pending: Vec<PendingDbRequest>,
    message: &str,
) {
    for entry in pending {
        let _ = send_stream_error(caller, exports, memory, entry.stream_bits, message);
    }
}

fn drain_db_pending(state: &mut HostState) -> Vec<PendingDbRequest> {
    state.db_cancel_index.clear();
    state.db_cancel_positions.clear();
    state.db_cancel_cursor = 0;
    std::mem::take(&mut state.db_pending)
        .into_values()
        .collect::<Vec<_>>()
}

fn handle_db_host_poll(mut caller: Caller<'_, HostState>) -> i32 {
    let memory = match ensure_memory(&mut caller) {
        Ok(mem) => mem,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    let exports = match runtime_exports(&mut caller) {
        Ok(exports) => exports,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };

    let mut deliveries = Vec::new();
    let mut failures: Option<(Vec<PendingDbRequest>, String)> = None;
    let mut drop_worker = false;
    {
        let state = caller.data_mut();
        // Child exit can precede the reader draining its output. Retirement
        // follows ordered channel EOF/error, never a racing try_wait result.
        if state.db_worker.is_some() {
            for _ in 0..128 {
                let message = match state.db_worker.as_mut() {
                    Some(worker) => worker.poll_response(),
                    None => break,
                };
                match message {
                    Ok(Some(resp)) => {
                        if let Some(pending) = state.db_pending.remove(&resp.request_id) {
                            db_cancel_untrack(state, resp.request_id);
                            deliveries.push((pending, resp));
                        }
                    }
                    Err(err) => {
                        let pending = drain_db_pending(state);
                        failures = Some((pending, format!("db host error: {err}")));
                        drop_worker = true;
                        break;
                    }
                    Ok(None) => break,
                }
            }
        }
    }
    // Successful responses preceding an error retain their delivery even when
    // the same poll retires the failed worker and fails its remaining requests.
    for (pending, response) in deliveries {
        deliver_worker_response(
            &mut caller,
            &exports,
            &memory,
            pending.stream_bits,
            response,
        );
    }
    if drop_worker {
        if let Some(worker) = caller.data_mut().db_worker.as_mut()
            && let Err(err) = worker.close()
        {
            let message = format!("database worker cleanup failed: {err:#}");
            if let Some((pending, original)) = failures {
                fail_pending_requests(
                    &mut caller,
                    &exports,
                    &memory,
                    pending,
                    &format!("{original}; {message}"),
                );
            }
            eprintln!("{message}");
            return 7;
        }
        caller.data_mut().db_worker = None;
    }

    if let Some((pending, message)) = failures {
        fail_pending_requests(&mut caller, &exports, &memory, pending, &message);
        return 0;
    }

    let now = Instant::now();
    let should_check = {
        let state = caller.data();
        state
            .last_cancel_check
            .map(|last| now.duration_since(last) >= Duration::from_millis(CANCEL_POLL_MS))
            .unwrap_or(true)
    };
    if should_check {
        let cancel_func = exports.cancel_is_cancelled;
        if let Some(cancel_func) = cancel_func {
            let candidate_ids = {
                let state = caller.data_mut();
                let budget = state.db_cancel_index.len().min(CANCEL_POLL_BATCH);
                indexed_next_batch(&state.db_cancel_index, &mut state.db_cancel_cursor, budget)
            };
            let candidates = {
                let state = caller.data_mut();
                let mut stale_ids = Vec::new();
                let mut batch = Vec::with_capacity(candidate_ids.len());
                for req_id in candidate_ids {
                    if let Some(pending) = state.db_pending.get(&req_id)
                        && pending.token_id != 0
                        && !pending.cancel_sent
                    {
                        batch.push((req_id, pending.token_id));
                    } else {
                        stale_ids.push(req_id);
                    }
                }
                for req_id in stale_ids {
                    db_cancel_untrack(state, req_id);
                }
                batch
            };
            let mut cancel_ids = Vec::new();
            for (req_id, token_id) in candidates {
                let boxed = box_int(token_id);
                if let Ok(bits) = call_i64(&cancel_func, &mut caller, &[Val::I64(boxed as i64)]) {
                    let bits = bits as u64;
                    if is_bool_bits(bits) && unbox_bool(bits) {
                        cancel_ids.push(req_id);
                    }
                }
            }
            if !cancel_ids.is_empty() {
                let state = caller.data_mut();
                for req_id in cancel_ids {
                    if let Some(worker_stdin) = state
                        .db_worker
                        .as_mut()
                        .and_then(|worker| worker.stdin.as_mut())
                    {
                        let mut stop_polling_token = false;
                        if let Some(pending) = state.db_pending.get_mut(&req_id)
                            && pending.token_id != 0
                            && !pending.cancel_sent
                            && send_worker_cancel(worker_stdin, req_id).is_ok()
                        {
                            pending.cancel_sent = true;
                            stop_polling_token = true;
                        }
                        if stop_polling_token || !state.db_pending.contains_key(&req_id) {
                            db_cancel_untrack(state, req_id);
                        }
                    }
                }
            }
        }
        caller.data_mut().last_cancel_check = Some(now);
    }

    0
}

fn ptr_from_i64(ptr: i64) -> Result<usize, i32> {
    let ptr_u64 = u64::try_from(ptr).map_err(|_| 1)?;
    usize::try_from(ptr_u64).map_err(|_| 1)
}

fn handle_db_host(
    mut caller: Caller<'_, HostState>,
    entry: &str,
    req_ptr: usize,
    len_bits: i64,
    out_ptr: usize,
    token_bits: i64,
) -> i32 {
    let len_bits_u64 = match u64::try_from(len_bits) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    let len = match usize::try_from(len_bits_u64) {
        Ok(val) => val,
        Err(_) => return 1,
    };
    if out_ptr == 0 {
        return 2;
    }
    if req_ptr == 0 && len != 0 {
        return 1;
    }
    let memory = match ensure_memory(&mut caller) {
        Ok(mem) => mem,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    let mut payload = vec![0u8; len];
    if len > 0 && memory.read(&mut caller, req_ptr, &mut payload).is_err() {
        return 1;
    }

    let exports = match runtime_exports(&mut caller) {
        Ok(exports) => exports,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };

    let stream_bits = match call_i64(&exports.stream_new, &mut caller, &[Val::I64(0)]) {
        Ok(bits) => bits as u64,
        Err(err) => {
            eprintln!("{err}");
            return 7;
        }
    };
    if memory
        .write(&mut caller, out_ptr, &stream_bits.to_le_bytes())
        .is_err()
    {
        return 2;
    }

    let timeout_ms = resolve_timeout_ms();
    let token_id = u64::try_from(token_bits).unwrap_or(0);
    let request_id = 'worker: {
        let state = caller.data_mut();
        let mut need_spawn = state.db_worker.is_none();
        if let Some(worker) = state.db_worker.as_mut() {
            match worker.child.try_wait() {
                Ok(Some(_)) => need_spawn = true,
                Ok(None) => {}
                Err(_) => need_spawn = true,
            }
        }
        if need_spawn {
            if !state.db_pending.is_empty() {
                break 'worker Err(WorkerError::Unavailable(wasmtime::Error::msg(
                    "database worker exited with responses still pending; poll before replacing it",
                )));
            }
            if let Some(worker) = state.db_worker.as_mut()
                && let Err(err) = worker.close()
            {
                break 'worker Err(WorkerError::Unavailable(err));
            }
            match DbWorker::new() {
                Ok(worker) => state.db_worker = Some(worker),
                Err(err) => break 'worker Err(WorkerError::Unavailable(err)),
            }
        }
        let worker = state
            .db_worker
            .as_mut()
            .expect("db_worker should be initialized");
        match worker.send_request(entry, &payload, timeout_ms) {
            Ok(id) => {
                state.db_pending.insert(
                    id,
                    PendingDbRequest {
                        stream_bits,
                        token_id,
                        cancel_sent: false,
                    },
                );
                if token_id != 0 {
                    db_cancel_track(state, id);
                }
                Ok(id)
            }
            Err(err) => Err(WorkerError::SendFailed(err)),
        }
    };
    match request_id {
        Ok(_) => 0,
        Err(WorkerError::Unavailable(err)) => {
            eprintln!("{err}");
            db_host_unavailable(&mut caller, &memory, out_ptr)
        }
        Err(WorkerError::SendFailed(err)) => {
            let _ = send_stream_error(
                &mut caller,
                &exports,
                &memory,
                stream_bits,
                &format!("db host send failed: {err}"),
            );
            0
        }
    }
}

pub(super) fn define_db_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
    let query = Func::wrap(
        &mut *store,
        |caller: Caller<'_, HostState>, req_ptr: i64, len: i64, out_ptr: i64, token: i64| {
            let req_ptr = match ptr_from_i64(req_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            let out_ptr = match ptr_from_i64(out_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            handle_db_host(caller, "db_query", req_ptr, len, out_ptr, token)
        },
    );
    let exec = Func::wrap(
        &mut *store,
        |caller: Caller<'_, HostState>, req_ptr: i64, len: i64, out_ptr: i64, token: i64| {
            let req_ptr = match ptr_from_i64(req_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            let out_ptr = match ptr_from_i64(out_ptr) {
                Ok(ptr) => ptr,
                Err(code) => return code,
            };
            handle_db_host(caller, "db_exec", req_ptr, len, out_ptr, token)
        },
    );
    let poll = Func::wrap(&mut *store, |caller: Caller<'_, HostState>| {
        handle_db_host_poll(caller)
    });
    linker.define(&mut *store, "env", "molt_db_query_host", query)?;
    linker.define(&mut *store, "env", "molt_db_exec_host", exec)?;
    linker.define(&mut *store, "env", "molt_db_host_poll", poll)?;
    Ok(())
}
