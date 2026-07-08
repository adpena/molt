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

fn read_frame(mut reader: impl Read) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header) as usize;
    if size > MAX_DB_FRAME_SIZE {
        bail!("worker frame too large: {size}");
    }
    let mut payload = vec![0u8; size];
    reader.read_exact(&mut payload)?;
    Ok(payload)
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

enum WorkerMessage {
    Response(WorkerResponse),
    Error(anyhow::Error),
}

enum WorkerError {
    Unavailable(anyhow::Error),
    SendFailed(anyhow::Error),
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
    stdin: Arc<Mutex<ChildStdin>>,
    responses: mpsc::Receiver<WorkerMessage>,
    next_id: u64,
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
        let mut child = command.spawn().context("spawn molt-worker")?;
        let stdin = child.stdin.take().context("missing worker stdin")?;
        let stdout = child.stdout.take().context("missing worker stdout")?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let frame = match read_frame(&mut reader) {
                    Ok(frame) => frame,
                    Err(err) => {
                        let _ = tx.send(WorkerMessage::Error(err));
                        break;
                    }
                };
                let response = match decode_worker_frame(&frame) {
                    Ok(resp) => WorkerMessage::Response(resp),
                    Err(err) => WorkerMessage::Error(err),
                };
                if tx.send(response).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            responses: rx,
            next_id: 1,
        })
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
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| anyhow::anyhow!("stdin lock poisoned"))?;
        write_frame(&mut *stdin, &bytes)?;
        Ok(request_id)
    }
}

fn send_worker_cancel(stdin: &Arc<Mutex<ChildStdin>>, target_id: u64) -> Result<()> {
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
    let mut guard = stdin
        .lock()
        .map_err(|_| anyhow::anyhow!("stdin lock poisoned"))?;
    write_frame(&mut *guard, &bytes)?;
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
        let worker_status = match state.db_worker.as_mut() {
            Some(worker) => worker.child.try_wait(),
            None => return 0,
        };
        match worker_status {
            Ok(Some(_)) | Err(_) => {
                let pending = drain_db_pending(state);
                failures = Some((pending, "db host worker exited".to_string()));
                drop_worker = true;
            }
            Ok(None) => {}
        }
        if failures.is_none() {
            loop {
                let message = match state.db_worker.as_mut() {
                    Some(worker) => worker.responses.try_recv(),
                    None => Err(mpsc::TryRecvError::Disconnected),
                };
                match message {
                    Ok(WorkerMessage::Response(resp)) => {
                        if let Some(pending) = state.db_pending.remove(&resp.request_id) {
                            db_cancel_untrack(state, resp.request_id);
                            deliveries.push((pending, resp));
                        }
                    }
                    Ok(WorkerMessage::Error(err)) => {
                        let pending = drain_db_pending(state);
                        failures = Some((pending, format!("db host error: {err}")));
                        drop_worker = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let pending = drain_db_pending(state);
                        failures = Some((pending, "db host disconnected".to_string()));
                        drop_worker = true;
                        break;
                    }
                }
            }
        }
    }
    if drop_worker {
        caller.data_mut().db_worker = None;
    }

    if let Some((pending, message)) = failures {
        fail_pending_requests(&mut caller, &exports, &memory, pending, &message);
        return 0;
    }

    for (pending, response) in deliveries {
        deliver_worker_response(
            &mut caller,
            &exports,
            &memory,
            pending.stream_bits,
            response,
        );
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
                let worker_stdin = state.db_worker.as_ref().map(|worker| worker.stdin.clone());
                if let Some(worker_stdin) = worker_stdin {
                    for req_id in cancel_ids {
                        let mut stop_polling_token = false;
                        if let Some(pending) = state.db_pending.get_mut(&req_id)
                            && pending.token_id != 0
                            && !pending.cancel_sent
                            && send_worker_cancel(&worker_stdin, req_id).is_ok()
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
