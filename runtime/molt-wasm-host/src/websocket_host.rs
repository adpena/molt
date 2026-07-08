use super::*;

impl WebSocketManager {
    pub(super) fn new() -> Self {
        Self {
            next_id: 1,
            sockets: HashMap::new(),
        }
    }

    fn insert(&mut self, socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.sockets.insert(
            id,
            WebSocketEntry {
                socket,
                queue: VecDeque::new(),
                closed: false,
            },
        );
        id
    }

    fn remove(&mut self, id: u64) -> Option<WebSocketEntry> {
        self.sockets.remove(&id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut WebSocketEntry> {
        self.sockets.get_mut(&id)
    }
}

pub(super) struct WebSocketManager {
    next_id: u64,
    sockets: HashMap<u64, WebSocketEntry>,
}

struct WebSocketEntry {
    socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
    queue: VecDeque<Vec<u8>>,
    closed: bool,
}

fn ws_get_mut(state: &mut HostState, handle: i64) -> Result<&mut WebSocketEntry, i32> {
    if handle <= 0 {
        return Err(libc::EBADF);
    }
    state.ws_manager.get_mut(handle as u64).ok_or(libc::EBADF)
}

fn ws_set_nonblocking(
    ws: &mut tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
) -> std::io::Result<()> {
    match ws.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_nonblocking(true)?;
        }
        MaybeTlsStream::Rustls(stream) => {
            stream.get_ref().set_nonblocking(true)?;
        }
        _ => {}
    }
    Ok(())
}

fn map_ws_error(err: &tungstenite::Error) -> i32 {
    match err {
        tungstenite::Error::Io(io_err) => map_io_error(io_err),
        tungstenite::Error::Url(_) => libc::EINVAL,
        tungstenite::Error::Http(_) => libc::ECONNREFUSED,
        tungstenite::Error::Tls(_) => libc::EIO,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => libc::EPIPE,
        _ => libc::EIO,
    }
}

fn ws_drain_incoming(entry: &mut WebSocketEntry) -> Result<(), i32> {
    if entry.closed {
        return Ok(());
    }
    loop {
        match entry.socket.read() {
            Ok(Message::Binary(bytes)) => {
                entry.queue.push_back(bytes.to_vec());
            }
            Ok(Message::Text(text)) => {
                entry.queue.push_back(text.to_string().into_bytes());
            }
            Ok(Message::Ping(payload)) => {
                let _ = entry.socket.send(Message::Pong(payload));
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) => {
                entry.closed = true;
                break;
            }
            Err(tungstenite::Error::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {
                break;
            }
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                entry.closed = true;
                break;
            }
            Err(err) => {
                entry.closed = true;
                return Err(map_ws_error(&err));
            }
        }
        if entry.queue.len() >= 64 {
            break;
        }
    }
    Ok(())
}

fn poll_ws_stream(stream: &TcpStream, events: u32) -> Result<u32, i32> {
    let mut poll_events: i16 = 0;
    if (events & IO_EVENT_READ) != 0 {
        poll_events |= HOST_POLLIN;
    }
    if (events & IO_EVENT_WRITE) != 0 {
        poll_events |= HOST_POLLOUT;
    }
    if poll_events == 0 {
        poll_events |= HOST_POLLIN;
    }
    #[cfg(unix)]
    {
        let fd = stream.as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & HOST_POLLERR) != 0
            || (revents & HOST_POLLHUP) != 0
            || (revents & HOST_POLLNVAL) != 0
        {
            ready |= IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return Ok(ready);
        }
        if (revents & HOST_POLLIN) != 0 {
            ready |= IO_EVENT_READ;
        }
        if (revents & HOST_POLLOUT) != 0 {
            ready |= IO_EVENT_WRITE;
        }
        Ok(ready)
    }
    #[cfg(windows)]
    {
        let fd = stream.as_raw_socket() as usize;
        let mut pfd = winsock::WSAPOLLFD {
            fd,
            events: poll_events,
            revents: 0,
        };
        let rc = unsafe { winsock::WSAPoll(&mut pfd, 1, 0) };
        if rc < 0 {
            return Err(map_io_error(&std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Ok(0);
        }
        let revents = pfd.revents;
        let mut ready = 0u32;
        if (revents & HOST_POLLERR) != 0
            || (revents & HOST_POLLHUP) != 0
            || (revents & HOST_POLLNVAL) != 0
        {
            ready |= IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return Ok(ready);
        }
        if (revents & HOST_POLLIN) != 0 {
            ready |= IO_EVENT_READ;
        }
        if (revents & HOST_POLLOUT) != 0 {
            ready |= IO_EVENT_WRITE;
        }
        Ok(ready)
    }
}

pub(super) fn define_ws_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
    let ws_connect = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, url_ptr: i32, url_len: i64, out_handle: i32| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if out_handle == 0 {
                return -libc::EFAULT;
            }
            if url_len < 0 || url_len > i64::from(i32::MAX) {
                return -libc::EINVAL;
            }
            let url_len = url_len as i32;
            let url_bytes = match read_bytes(&mut caller, &memory, url_ptr, url_len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let url_str = match String::from_utf8(url_bytes) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            let url = match Url::parse(&url_str) {
                Ok(val) => val,
                Err(_) => return -libc::EINVAL,
            };
            if url.scheme() != "ws" && url.scheme() != "wss" {
                return -libc::EINVAL;
            }
            let (mut socket, _) = match connect(url.as_str()) {
                Ok(val) => val,
                Err(err) => return -map_ws_error(&err),
            };
            if let Err(err) = ws_set_nonblocking(&mut socket) {
                return -map_io_error(&err);
            }
            let handle = {
                let state = caller.data_mut();
                state.ws_manager.insert(socket)
            };
            if write_u64(&mut caller, &memory, out_handle, handle).is_err() {
                caller.data_mut().ws_manager.remove(handle);
                return -libc::EFAULT;
            }
            0
        },
    );
    let ws_send = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, data_ptr: i32, len: i64| -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if len < 0 || len > i64::from(i32::MAX) {
                return -libc::EINVAL;
            }
            let len = len as i32;
            let payload = match read_bytes(&mut caller, &memory, data_ptr, len) {
                Ok(buf) => buf,
                Err(_) => return -libc::EFAULT,
            };
            let entry = match ws_get_mut(caller.data_mut(), handle) {
                Ok(entry) => entry,
                Err(errno) => return -errno,
            };
            if entry.closed {
                return -libc::EPIPE;
            }
            match entry.socket.send(Message::Binary(payload.into())) {
                Ok(_) => 0,
                Err(tungstenite::Error::Io(err))
                    if err.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    -libc::EWOULDBLOCK
                }
                Err(err) => {
                    entry.closed = true;
                    -map_ws_error(&err)
                }
            }
        },
    );
    let ws_recv = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         handle: i64,
         buf_ptr: i32,
         buf_cap: i32,
         out_len: i32|
         -> i32 {
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::EFAULT,
            };
            if out_len == 0 {
                return -libc::EFAULT;
            }
            let cap = if buf_cap < 0 {
                return -libc::EINVAL;
            } else {
                buf_cap as usize
            };

            let (pending_bytes, needed_len, closed) = {
                let mut pending_bytes: Option<Vec<u8>> = None;
                let mut needed_len: Option<usize> = None;
                let entry = match ws_get_mut(caller.data_mut(), handle) {
                    Ok(entry) => entry,
                    Err(errno) => return -errno,
                };
                if entry.queue.is_empty()
                    && !entry.closed
                    && let Err(errno) = ws_drain_incoming(entry)
                {
                    return -errno;
                }
                if let Some(front) = entry.queue.front() {
                    if front.len() > cap {
                        needed_len = Some(front.len());
                    } else {
                        pending_bytes = entry.queue.pop_front();
                    }
                }
                (pending_bytes, needed_len, entry.closed)
            };

            if let Some(len) = needed_len {
                let _ = write_u32(&mut caller, &memory, out_len, len as u32);
                return -libc::ENOMEM;
            }
            if let Some(bytes) = pending_bytes {
                if write_bytes(&mut caller, &memory, buf_ptr, &bytes).is_err() {
                    return -libc::EFAULT;
                }
                let _ = write_u32(&mut caller, &memory, out_len, bytes.len() as u32);
                return 0;
            }
            let _ = write_u32(&mut caller, &memory, out_len, 0);
            if closed { 0 } else { -libc::EWOULDBLOCK }
        },
    );
    let ws_poll = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64, events: i32| -> i32 {
            let entry = match ws_get_mut(caller.data_mut(), handle) {
                Ok(entry) => entry,
                Err(errno) => return -errno,
            };
            if entry.closed {
                return (IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE) as i32;
            }
            let events = events as u32;
            let mut ready = 0u32;
            if (events & IO_EVENT_READ) != 0 {
                if entry.queue.is_empty()
                    && let Err(errno) = ws_drain_incoming(entry)
                {
                    return -errno;
                }
                if !entry.queue.is_empty() {
                    ready |= IO_EVENT_READ;
                }
            }
            if (events & IO_EVENT_WRITE) != 0 {
                let stream_ref = match entry.socket.get_ref() {
                    MaybeTlsStream::Plain(stream) => stream,
                    MaybeTlsStream::Rustls(stream) => stream.get_ref(),
                    _ => return -libc::EIO,
                };
                let poll_ready = match poll_ws_stream(stream_ref, IO_EVENT_WRITE) {
                    Ok(mask) => mask,
                    Err(errno) => return -errno,
                };
                if (poll_ready & IO_EVENT_ERROR) != 0 {
                    return (IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE) as i32;
                }
                if (poll_ready & IO_EVENT_WRITE) != 0 {
                    ready |= IO_EVENT_WRITE;
                }
            }
            ready as i32
        },
    );
    let ws_close = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>, handle: i64| -> i32 {
            let entry = match caller.data_mut().ws_manager.remove(handle as u64) {
                Some(entry) => entry,
                None => return -libc::EBADF,
            };
            if entry.closed {
                return 0;
            }
            let mut socket = entry.socket;
            let _ = socket.close(None);
            0
        },
    );
    linker.define(&mut *store, "env", "molt_ws_connect_host", ws_connect)?;
    linker.define(&mut *store, "env", "molt_ws_poll_host", ws_poll)?;
    linker.define(&mut *store, "env", "molt_ws_send_host", ws_send)?;
    linker.define(&mut *store, "env", "molt_ws_recv_host", ws_recv)?;
    linker.define(&mut *store, "env", "molt_ws_close_host", ws_close)?;
    Ok(())
}
