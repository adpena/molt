use super::blocking::blocking_waiter_id;
use super::*;

#[cfg(molt_has_net_io)]
pub(super) fn socket_debug_fd(socket_ptr: *mut u8) -> Option<i64> {
    with_socket_mut(socket_ptr, |inner| {
        #[cfg(unix)]
        {
            inner
                .raw_fd()
                .map(|fd| fd as i64)
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))
        }
        #[cfg(windows)]
        {
            inner
                .raw_socket()
                .map(|fd| fd as i64)
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))
        }
    })
    .ok()
}

#[cfg(all(molt_has_net_io, unix))]
fn stream_debug_fd(stream: &MioTcpStream) -> i64 {
    stream.as_raw_fd() as i64
}

#[cfg(all(molt_has_net_io, windows))]
fn stream_debug_fd(stream: &MioTcpStream) -> i64 {
    stream.as_raw_socket() as i64
}

#[cfg(all(not(target_arch = "wasm32"), not(any(unix, windows))))]
fn stream_debug_fd(_stream: &MioTcpStream) -> i64 {
    -1
}

#[cfg(molt_has_net_io)]
pub(super) struct IoWaiter {
    pub(super) socket_id: usize,
    pub(super) events: u32,
}

#[cfg(molt_has_net_io)]
#[derive(Default)]
pub(super) struct WaiterList {
    order: Vec<PtrSlot>,
    index: HashMap<PtrSlot, usize>,
}

#[cfg(molt_has_net_io)]
impl WaiterList {
    fn insert(&mut self, waiter: PtrSlot) -> bool {
        if self.index.contains_key(&waiter) {
            return false;
        }
        let next = self.order.len();
        self.order.push(waiter);
        self.index.insert(waiter, next);
        true
    }

    fn remove(&mut self, waiter: PtrSlot) -> bool {
        let Some(idx) = self.index.remove(&waiter) else {
            return false;
        };
        let Some(last) = self.order.pop() else {
            return false;
        };
        if idx < self.order.len() {
            self.order[idx] = last;
            self.index.insert(last, idx);
        }
        true
    }

    fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.order.len()
    }

    pub(super) fn drain(&mut self) -> Vec<PtrSlot> {
        self.index.clear();
        std::mem::take(&mut self.order)
    }

    pub(super) fn replace_with(&mut self, order: Vec<PtrSlot>) {
        self.order = order;
        self.index.clear();
        for (idx, waiter) in self.order.iter().copied().enumerate() {
            self.index.insert(waiter, idx);
        }
    }
}

#[cfg(molt_has_net_io)]
pub(super) enum IoSource {
    Socket(PtrSlot),
    WebSocket(MioTcpStream),
}

pub(super) struct IoSocketEntry {
    pub(super) token: Token,
    pub(super) interests: Interest,
    pub(super) waiters: WaiterList,
    pub(super) blocking_waiters: BlockingWaiterList,
    pub(super) source: IoSource,
    pub(super) debug_fd: i64,
}

#[cfg(molt_has_net_io)]
struct IoRegistrationRollback {
    socket_id: usize,
    waiter_key: PtrSlot,
    token: Token,
    new_entry: bool,
    previous_interests: Option<Interest>,
}

#[cfg(molt_has_net_io)]
struct IoBlockingRegistrationRollback {
    socket_id: usize,
    waiter_id: usize,
    token: Token,
    new_entry: bool,
    previous_interests: Option<Interest>,
}

#[cfg(molt_has_net_io)]
pub(crate) struct IoPoller {
    pub(super) poll: Mutex<Poll>,
    registry: Registry,
    pub(super) events: Mutex<Events>,
    waker: Waker,
    pub(super) running: AtomicBool,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    next_token: AtomicUsize,
    pub(super) tokens: Mutex<HashMap<Token, usize>>,
    pub(super) sockets: Mutex<HashMap<usize, IoSocketEntry>>,
    pub(super) waiters: Mutex<HashMap<PtrSlot, IoWaiter>>,
    pub(super) ready: Mutex<HashMap<PtrSlot, u32>>,
}

impl IoPoller {
    pub(crate) fn new() -> Self {
        let poll = Poll::new().expect("io poller");
        let registry = poll.registry().try_clone().expect("io registry");
        let waker = Waker::new(poll.registry(), Token(0)).expect("io waker");
        Self {
            poll: Mutex::new(poll),
            registry,
            events: Mutex::new(Events::with_capacity(256)),
            waker,
            running: AtomicBool::new(true),
            worker: Mutex::new(None),
            next_token: AtomicUsize::new(1),
            tokens: Mutex::new(HashMap::new()),
            sockets: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            ready: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn start_worker(self: &Arc<Self>) {
        let poller = Arc::clone(self);
        let handle = thread::spawn(move || io_worker(poller));
        let mut guard = self.worker.lock().unwrap();
        *guard = Some(handle);
    }

    pub(crate) fn shutdown(&self) {
        if !self.running.swap(false, AtomicOrdering::SeqCst) {
            return;
        }
        let _ = self.waker.wake();
        let handle = { self.worker.lock().unwrap().take() };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    fn rollback_wait_registration(&self, rollback: IoRegistrationRollback) {
        {
            let mut waiters = self.waiters.lock().unwrap();
            waiters.remove(&rollback.waiter_key);
        }
        {
            let mut ready = self.ready.lock().unwrap();
            ready.remove(&rollback.waiter_key);
        }
        let mut sockets = self.sockets.lock().unwrap();
        if rollback.new_entry {
            sockets.remove(&rollback.socket_id);
            self.tokens.lock().unwrap().remove(&rollback.token);
            return;
        }
        if let Some(entry) = sockets.get_mut(&rollback.socket_id) {
            entry.waiters.remove(rollback.waiter_key);
            if let Some(previous) = rollback.previous_interests {
                entry.interests = previous;
            }
        }
    }

    fn rollback_blocking_registration(&self, rollback: IoBlockingRegistrationRollback) {
        let mut sockets = self.sockets.lock().unwrap();
        if rollback.new_entry {
            sockets.remove(&rollback.socket_id);
            self.tokens.lock().unwrap().remove(&rollback.token);
            return;
        }
        if let Some(entry) = sockets.get_mut(&rollback.socket_id) {
            entry.blocking_waiters.remove(rollback.waiter_id);
            if let Some(previous) = rollback.previous_interests {
                entry.interests = previous;
            }
        }
    }

    pub(crate) fn register_wait(
        &self,
        future_ptr: *mut u8,
        socket_ptr: *mut u8,
        events: u32,
    ) -> Result<(), std::io::Error> {
        if future_ptr.is_null() || socket_ptr.is_null() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "invalid io wait",
            ));
        }
        let waiter_key = PtrSlot(future_ptr);
        {
            let mut waiters = self.waiters.lock().unwrap();
            if waiters.contains_key(&waiter_key) {
                return Ok(());
            }
            waiters.insert(
                waiter_key,
                IoWaiter {
                    socket_id: socket_ptr as usize,
                    events,
                },
            );
        }
        let socket_id = socket_ptr as usize;
        let mut sockets = self.sockets.lock().unwrap();
        let mut new_entry = false;
        let token = sockets
            .get(&socket_id)
            .map(|entry| entry.token)
            .unwrap_or_else(|| {
                new_entry = true;
                let token = Token(self.next_token.fetch_add(1, AtomicOrdering::Relaxed));
                let debug_fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                sockets.insert(
                    socket_id,
                    IoSocketEntry {
                        token,
                        interests: Interest::READABLE,
                        waiters: WaiterList::default(),
                        blocking_waiters: BlockingWaiterList::default(),
                        source: IoSource::Socket(PtrSlot(socket_ptr)),
                        debug_fd,
                    },
                );
                self.tokens.lock().unwrap().insert(token, socket_id);
                token
            });
        let entry = sockets.get_mut(&socket_id).expect("socket entry");
        let previous_interests = entry.interests;
        entry.waiters.insert(waiter_key);
        let interest = interest_from_events(events);
        let needs_register = new_entry;
        let mut updated = false;
        if needs_register {
            entry.interests = interest;
            updated = true;
        } else {
            let new_interest = entry.interests | interest;
            if new_interest != entry.interests {
                entry.interests = new_interest;
                updated = true;
            }
        }
        let interests = entry.interests;
        let debug_fd = entry.debug_fd;
        let rollback = IoRegistrationRollback {
            socket_id,
            waiter_key,
            token,
            new_entry,
            previous_interests: (!new_entry).then_some(previous_interests),
        };
        drop(sockets);
        let register_result = if needs_register {
            with_socket_mut(socket_ptr, |sock| {
                let source = sock.source_mut().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                })?;
                self.registry.register(source, token, interests)
            })
        } else if updated {
            with_socket_mut(socket_ptr, |sock| {
                let source = sock.source_mut().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                })?;
                self.registry.reregister(source, token, interests)
            })
        } else {
            Ok(())
        };
        if let Err(err) = register_result {
            self.rollback_wait_registration(rollback);
            return Err(err);
        }
        let _ = self.waker.wake();
        if trace_io_poller() {
            eprintln!(
                "molt io poller: register future=0x{:x} socket=0x{:x} fd={} events={}",
                future_ptr as usize, socket_ptr as usize, debug_fd, events
            );
        }
        Ok(())
    }

    pub(crate) fn register_ws_wait(
        &self,
        future_ptr: *mut u8,
        ws_ptr: *mut u8,
        events: u32,
        stream: Option<MioTcpStream>,
    ) -> Result<(), std::io::Error> {
        if future_ptr.is_null() || ws_ptr.is_null() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "invalid io wait",
            ));
        }
        let waiter_key = PtrSlot(future_ptr);
        {
            let mut waiters = self.waiters.lock().unwrap();
            if waiters.contains_key(&waiter_key) {
                return Ok(());
            }
            waiters.insert(
                waiter_key,
                IoWaiter {
                    socket_id: ws_ptr as usize,
                    events,
                },
            );
        }
        let socket_id = ws_ptr as usize;
        let mut sockets = self.sockets.lock().unwrap();
        let mut new_entry = false;
        let token = match sockets.get(&socket_id) {
            Some(entry) => entry.token,
            None => {
                new_entry = true;
                let stream = match stream {
                    Some(stream) => stream,
                    None => {
                        drop(sockets);
                        let mut waiters = self.waiters.lock().unwrap();
                        waiters.remove(&waiter_key);
                        return Err(std::io::Error::new(
                            ErrorKind::InvalidInput,
                            "websocket not registered",
                        ));
                    }
                };
                let token = Token(self.next_token.fetch_add(1, AtomicOrdering::Relaxed));
                let debug_fd = stream_debug_fd(&stream);
                sockets.insert(
                    socket_id,
                    IoSocketEntry {
                        token,
                        interests: Interest::READABLE,
                        waiters: WaiterList::default(),
                        blocking_waiters: BlockingWaiterList::default(),
                        source: IoSource::WebSocket(stream),
                        debug_fd,
                    },
                );
                self.tokens.lock().unwrap().insert(token, socket_id);
                token
            }
        };
        let entry = sockets.get_mut(&socket_id).expect("socket entry");
        let previous_interests = entry.interests;
        entry.waiters.insert(waiter_key);
        let interest = interest_from_events(events);
        let needs_register = new_entry;
        let mut updated = false;
        if needs_register {
            entry.interests = interest;
            updated = true;
        } else {
            let new_interest = entry.interests | interest;
            if new_interest != entry.interests {
                entry.interests = new_interest;
                updated = true;
            }
        }
        let interests = entry.interests;
        let debug_fd = entry.debug_fd;
        let rollback = IoRegistrationRollback {
            socket_id,
            waiter_key,
            token,
            new_entry,
            previous_interests: (!new_entry).then_some(previous_interests),
        };
        let register_result = match &mut entry.source {
            IoSource::WebSocket(stream) => {
                if needs_register {
                    self.registry.register(stream, token, interests)
                } else if updated {
                    self.registry.reregister(stream, token, interests)
                } else {
                    Ok(())
                }
            }
            IoSource::Socket(_) => Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "websocket not pollable",
            )),
        };
        drop(sockets);
        if let Err(err) = register_result {
            self.rollback_wait_registration(rollback);
            return Err(err);
        }
        let _ = self.waker.wake();
        if trace_io_poller() {
            eprintln!(
                "molt io poller: register future=0x{:x} socket=0x{:x} fd={} events={}",
                future_ptr as usize, ws_ptr as usize, debug_fd, events
            );
        }
        Ok(())
    }

    pub(crate) fn cancel_waiter(&self, future_ptr: *mut u8) {
        if future_ptr.is_null() {
            return;
        }
        let waiter_key = PtrSlot(future_ptr);
        let mut waiters = self.waiters.lock().unwrap();
        let Some(waiter) = waiters.remove(&waiter_key) else {
            return;
        };
        let mut sockets = self.sockets.lock().unwrap();
        if let Some(entry) = sockets.get_mut(&waiter.socket_id) {
            entry.waiters.remove(waiter_key);
            if entry.waiters.is_empty() {
                let token = entry.token;
                let entry = sockets.remove(&waiter.socket_id);
                self.tokens.lock().unwrap().remove(&token);
                drop(sockets);
                let _ = self.waker.wake();
                if let Some(entry) = entry {
                    self.deregister_entry(entry);
                }
            }
        }
    }

    pub(super) fn mark_ready(&self, future_ptr: PtrSlot, ready: u32) {
        let mut ready_map = self.ready.lock().unwrap();
        ready_map
            .entry(future_ptr)
            .and_modify(|val| *val |= ready)
            .or_insert(ready);
    }

    pub(crate) fn take_ready(&self, future_ptr: *mut u8) -> Option<u32> {
        if future_ptr.is_null() {
            return None;
        }
        let mut ready_map = self.ready.lock().unwrap();
        ready_map.remove(&PtrSlot(future_ptr))
    }

    pub(super) fn socket_for_token(&self, token: Token) -> Option<usize> {
        let tokens = self.tokens.lock().unwrap();
        tokens.get(&token).copied()
    }

    fn deregister_entry(&self, mut entry: IoSocketEntry) {
        match &mut entry.source {
            IoSource::Socket(socket_ptr) => {
                let _ = with_socket_mut(socket_ptr.0, |sock| {
                    let source = sock.source_mut().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                    })?;
                    self.registry.deregister(source)
                });
            }
            IoSource::WebSocket(stream) => {
                let _ = self.registry.deregister(stream);
            }
        }
    }

    pub(crate) fn deregister_socket(&self, _py: &PyToken<'_>, socket_ptr: *mut u8) {
        if socket_ptr.is_null() {
            return;
        }
        let socket_id = socket_ptr as usize;
        let mut waiters = self.waiters.lock().unwrap();
        let mut sockets = self.sockets.lock().unwrap();
        let entry = sockets.remove(&socket_id);
        if let Some(mut entry) = entry {
            self.tokens.lock().unwrap().remove(&entry.token);
            let mut ready_futures: Vec<PtrSlot> = Vec::new();
            let entry_waiters = entry.waiters.drain();
            for waiter in entry_waiters {
                waiters.remove(&waiter);
                ready_futures.push(waiter);
            }
            for waiter in entry.blocking_waiters.drain() {
                let mut guard = waiter.ready.lock().unwrap();
                *guard = Some(IO_EVENT_ERROR);
                drop(guard);
                waiter.condvar.notify_all();
            }
            drop(waiters);
            drop(sockets);
            let _ = self.waker.wake();
            self.deregister_entry(entry);
            for future in ready_futures {
                self.mark_ready(future, IO_EVENT_ERROR);
                let _ = wake_await_waiters(_py, future.0);
            }
        }
    }

    pub(crate) fn wait_blocking(
        &self,
        socket_ptr: *mut u8,
        events: u32,
        timeout: Option<Duration>,
    ) -> Result<u32, std::io::Error> {
        if socket_ptr.is_null() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "invalid socket",
            ));
        }
        let waiter = Arc::new(BlockingWaiter {
            events,
            ready: Mutex::new(None),
            condvar: Condvar::new(),
        });
        let waiter_id = Arc::as_ptr(&waiter) as usize;
        let socket_id = socket_ptr as usize;
        let mut sockets = self.sockets.lock().unwrap();
        let mut new_entry = false;
        let token = sockets
            .get(&socket_id)
            .map(|entry| entry.token)
            .unwrap_or_else(|| {
                new_entry = true;
                let token = Token(self.next_token.fetch_add(1, AtomicOrdering::Relaxed));
                let debug_fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                sockets.insert(
                    socket_id,
                    IoSocketEntry {
                        token,
                        interests: Interest::READABLE,
                        waiters: WaiterList::default(),
                        blocking_waiters: BlockingWaiterList::default(),
                        source: IoSource::Socket(PtrSlot(socket_ptr)),
                        debug_fd,
                    },
                );
                self.tokens.lock().unwrap().insert(token, socket_id);
                token
            });
        let entry = sockets.get_mut(&socket_id).expect("socket entry");
        let previous_interests = entry.interests;
        entry.blocking_waiters.insert(Arc::clone(&waiter));
        let interest = interest_from_events(events);
        let mut updated = false;
        let needs_register = entry.waiters.is_empty() && entry.blocking_waiters.len() == 1;
        if needs_register {
            entry.interests = interest;
            updated = true;
        } else {
            let new_interest = entry.interests | interest;
            if new_interest != entry.interests {
                entry.interests = new_interest;
                updated = true;
            }
        }
        let interests = entry.interests;
        let rollback = IoBlockingRegistrationRollback {
            socket_id,
            waiter_id,
            token,
            new_entry,
            previous_interests: (!new_entry).then_some(previous_interests),
        };
        drop(sockets);
        if updated {
            let register_result = with_socket_mut(socket_ptr, |sock| {
                if needs_register {
                    let source = sock.source_mut().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                    })?;
                    match self.registry.register(source, token, interests) {
                        Ok(()) => Ok(()),
                        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                            self.registry.reregister(source, token, interests)
                        }
                        Err(err) => Err(err),
                    }
                } else {
                    let source = sock.source_mut().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                    })?;
                    self.registry.reregister(source, token, interests)
                }
            });
            if let Err(err) = register_result {
                self.rollback_blocking_registration(rollback);
                return Err(err);
            }
        }
        let _ = self.waker.wake();
        let deadline = timeout.map(|dur| Instant::now() + dur);
        let mut guard = waiter.ready.lock().unwrap();
        loop {
            if let Some(ready) = *guard {
                drop(guard);
                let mut sockets = self.sockets.lock().unwrap();
                if let Some(entry) = sockets.get_mut(&socket_id) {
                    entry.blocking_waiters.remove(waiter_id);
                    if entry.waiters.is_empty() && entry.blocking_waiters.is_empty() {
                        let token = entry.token;
                        sockets.remove(&socket_id);
                        self.tokens.lock().unwrap().remove(&token);
                        drop(sockets);
                        let _ = self.waker.wake();
                        let _ = with_socket_mut(socket_ptr, |sock| {
                            let source = sock.source_mut().ok_or_else(|| {
                                std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                            })?;
                            self.registry.deregister(source)
                        });
                    }
                }
                return Ok(ready);
            }
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let timeout = deadline - now;
                let _release = GilReleaseGuard::suspend();
                let (next, _) = waiter.condvar.wait_timeout(guard, timeout).unwrap();
                guard = next;
            } else {
                let _release = GilReleaseGuard::suspend();
                guard = waiter.condvar.wait(guard).unwrap();
            }
        }
        drop(guard);
        let mut sockets = self.sockets.lock().unwrap();
        if let Some(entry) = sockets.get_mut(&socket_id) {
            entry.blocking_waiters.remove(waiter_id);
            if entry.waiters.is_empty() && entry.blocking_waiters.is_empty() {
                let token = entry.token;
                sockets.remove(&socket_id);
                self.tokens.lock().unwrap().remove(&token);
                drop(sockets);
                let _ = self.waker.wake();
                let _ = with_socket_mut(socket_ptr, |sock| {
                    let source = sock.source_mut().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                    })?;
                    self.registry.deregister(source)
                });
            }
        }
        Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"))
    }
}

fn interest_from_events(events: u32) -> Interest {
    let mut interest = None;
    if (events & IO_EVENT_READ) != 0 {
        interest = Some(Interest::READABLE);
    }
    if (events & IO_EVENT_WRITE) != 0 {
        interest = Some(match interest {
            Some(existing) => existing | Interest::WRITABLE,
            None => Interest::WRITABLE,
        });
    }
    interest.unwrap_or(Interest::READABLE)
}

mod tests {
    use super::*;

    fn slot(addr: usize) -> PtrSlot {
        PtrSlot(addr as *mut u8)
    }

    fn socket_entry(
        token: Token,
        interests: Interest,
        waiters: WaiterList,
        blocking_waiters: BlockingWaiterList,
    ) -> IoSocketEntry {
        IoSocketEntry {
            token,
            interests,
            waiters,
            blocking_waiters,
            source: IoSource::Socket(slot(0xdead)),
            debug_fd: -1,
        }
    }

    #[test]
    fn rollback_new_wait_registration_clears_all_maps() {
        let poller = IoPoller::new();
        let waiter = slot(0x1000);
        let socket_id = 0x2000usize;
        let token = Token(11);
        let mut waiters = WaiterList::default();
        waiters.insert(waiter);
        poller.waiters.lock().unwrap().insert(
            waiter,
            IoWaiter {
                socket_id,
                events: IO_EVENT_READ,
            },
        );
        poller.sockets.lock().unwrap().insert(
            socket_id,
            socket_entry(
                token,
                Interest::READABLE,
                waiters,
                BlockingWaiterList::default(),
            ),
        );
        poller.tokens.lock().unwrap().insert(token, socket_id);
        poller.ready.lock().unwrap().insert(waiter, IO_EVENT_ERROR);

        poller.rollback_wait_registration(IoRegistrationRollback {
            socket_id,
            waiter_key: waiter,
            token,
            new_entry: true,
            previous_interests: None,
        });

        assert!(poller.waiters.lock().unwrap().is_empty());
        assert!(poller.sockets.lock().unwrap().is_empty());
        assert!(poller.tokens.lock().unwrap().is_empty());
        assert!(poller.ready.lock().unwrap().is_empty());
    }

    #[test]
    fn rollback_existing_wait_registration_restores_interest_and_waiters() {
        let poller = IoPoller::new();
        let existing = slot(0x1000);
        let waiter = slot(0x1008);
        let socket_id = 0x2000usize;
        let token = Token(12);
        let mut waiters = WaiterList::default();
        waiters.insert(existing);
        waiters.insert(waiter);
        poller.waiters.lock().unwrap().insert(
            waiter,
            IoWaiter {
                socket_id,
                events: IO_EVENT_WRITE,
            },
        );
        poller.sockets.lock().unwrap().insert(
            socket_id,
            socket_entry(
                token,
                Interest::READABLE | Interest::WRITABLE,
                waiters,
                BlockingWaiterList::default(),
            ),
        );
        poller.tokens.lock().unwrap().insert(token, socket_id);

        poller.rollback_wait_registration(IoRegistrationRollback {
            socket_id,
            waiter_key: waiter,
            token,
            new_entry: false,
            previous_interests: Some(Interest::READABLE),
        });

        assert!(!poller.waiters.lock().unwrap().contains_key(&waiter));
        assert_eq!(poller.tokens.lock().unwrap().get(&token), Some(&socket_id));
        let sockets = poller.sockets.lock().unwrap();
        let entry = sockets.get(&socket_id).expect("socket entry");
        assert_eq!(entry.interests, Interest::READABLE);
        assert_eq!(entry.waiters.len(), 1);
    }

    #[test]
    fn rollback_blocking_registration_removes_waiter_and_restores_interest() {
        let poller = IoPoller::new();
        let socket_id = 0x2000usize;
        let token = Token(13);
        let waiter = Arc::new(BlockingWaiter {
            events: IO_EVENT_WRITE,
            ready: Mutex::new(None),
            condvar: Condvar::new(),
        });
        let waiter_id = blocking_waiter_id(&waiter);
        let mut blocking_waiters = BlockingWaiterList::default();
        blocking_waiters.insert(Arc::clone(&waiter));
        poller.sockets.lock().unwrap().insert(
            socket_id,
            socket_entry(
                token,
                Interest::READABLE | Interest::WRITABLE,
                WaiterList::default(),
                blocking_waiters,
            ),
        );
        poller.tokens.lock().unwrap().insert(token, socket_id);

        poller.rollback_blocking_registration(IoBlockingRegistrationRollback {
            socket_id,
            waiter_id,
            token,
            new_entry: false,
            previous_interests: Some(Interest::READABLE),
        });

        assert_eq!(poller.tokens.lock().unwrap().get(&token), Some(&socket_id));
        let sockets = poller.sockets.lock().unwrap();
        let entry = sockets.get(&socket_id).expect("socket entry");
        assert_eq!(entry.interests, Interest::READABLE);
        assert!(entry.blocking_waiters.is_empty());
    }

    #[test]
    fn rollback_new_blocking_registration_clears_socket_and_token() {
        let poller = IoPoller::new();
        let socket_id = 0x2000usize;
        let token = Token(14);
        let waiter = Arc::new(BlockingWaiter {
            events: IO_EVENT_READ,
            ready: Mutex::new(None),
            condvar: Condvar::new(),
        });
        let waiter_id = blocking_waiter_id(&waiter);
        let mut blocking_waiters = BlockingWaiterList::default();
        blocking_waiters.insert(waiter);
        poller.sockets.lock().unwrap().insert(
            socket_id,
            socket_entry(
                token,
                Interest::READABLE,
                WaiterList::default(),
                blocking_waiters,
            ),
        );
        poller.tokens.lock().unwrap().insert(token, socket_id);

        poller.rollback_blocking_registration(IoBlockingRegistrationRollback {
            socket_id,
            waiter_id,
            token,
            new_entry: true,
            previous_interests: None,
        });

        assert!(poller.sockets.lock().unwrap().is_empty());
        assert!(poller.tokens.lock().unwrap().is_empty());
    }
}
