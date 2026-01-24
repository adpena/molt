use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use mio::{Events, Interest, Poll, Token, Waker};

use crate::{
    dec_ref_bits, header_from_obj_ptr, inc_ref_bits, io_wait_poll_fn_addr, monotonic_now_secs,
    molt_future_new, obj_from_bits, pending_bits_i64, ptr_from_bits, raise_exception,
    resolve_obj_ptr, runtime_state, to_f64, to_i64, GilGuard, MoltHeader, MoltObject, PtrSlot,
    PyToken,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{raise_os_error, IO_EVENT_ERROR, IO_EVENT_READ, IO_EVENT_WRITE};
use super::{await_waiters_take, wake_task_ptr};
use super::sockets::{socket_ptr_from_bits_or_fd, socket_ref_inc, with_socket_mut};
use crate::require_net_capability;

#[cfg(not(target_arch = "wasm32"))]
struct IoWaiter {
    socket_id: usize,
    events: u32,
}

#[cfg(not(target_arch = "wasm32"))]
struct IoSocketEntry {
    token: Token,
    interests: Interest,
    waiters: Vec<PtrSlot>,
    blocking_waiters: Vec<Arc<BlockingWaiter>>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct IoPoller {
    poll: Mutex<Poll>,
    events: Mutex<Events>,
    waker: Waker,
    running: AtomicBool,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    next_token: AtomicUsize,
    tokens: Mutex<HashMap<Token, usize>>,
    sockets: Mutex<HashMap<usize, IoSocketEntry>>,
    waiters: Mutex<HashMap<PtrSlot, IoWaiter>>,
    ready: Mutex<HashMap<PtrSlot, u32>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct BlockingWaiter {
    events: u32,
    ready: Mutex<Option<u32>>,
    condvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
impl IoPoller {
    pub(crate) fn new() -> Self {
        let poll = Poll::new().expect("io poller");
        let waker = Waker::new(poll.registry(), Token(0)).expect("io waker");
        Self {
            poll: Mutex::new(poll),
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
        let token = sockets
            .get(&socket_id)
            .map(|entry| entry.token)
            .unwrap_or_else(|| {
                let token = Token(self.next_token.fetch_add(1, AtomicOrdering::Relaxed));
                sockets.insert(
                    socket_id,
                    IoSocketEntry {
                        token,
                        interests: Interest::READABLE,
                        waiters: Vec::new(),
                        blocking_waiters: Vec::new(),
                    },
                );
                self.tokens.lock().unwrap().insert(token, socket_id);
                token
            });
        let entry = sockets.get_mut(&socket_id).expect("socket entry");
        if !entry.waiters.contains(&waiter_key) {
            entry.waiters.push(waiter_key);
        }
        let interest = interest_from_events(events);
        let needs_register = entry.waiters.len() == 1;
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
        drop(sockets);
        if needs_register {
            with_socket_mut(socket_ptr, |sock| {
                let source = sock.source_mut().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                })?;
                self.poll
                    .lock()
                    .unwrap()
                    .registry()
                    .register(source, token, interests)
            })?;
        } else if updated {
            with_socket_mut(socket_ptr, |sock| {
                let source = sock.source_mut().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                })?;
                self.poll
                    .lock()
                    .unwrap()
                    .registry()
                    .reregister(source, token, interests)
            })?;
        }
        let _ = self.waker.wake();
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
            if let Some(pos) = entry.waiters.iter().position(|val| *val == waiter_key) {
                entry.waiters.swap_remove(pos);
            }
            if entry.waiters.is_empty() {
                let token = entry.token;
                sockets.remove(&waiter.socket_id);
                self.tokens.lock().unwrap().remove(&token);
                drop(sockets);
                let _ = with_socket_mut(waiter.socket_id as *mut u8, |sock| {
                    let source = sock.source_mut().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                    })?;
                    self.poll.lock().unwrap().registry().deregister(source)
                });
            }
        }
    }

    fn mark_ready(&self, future_ptr: PtrSlot, ready: u32) {
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

    fn socket_for_token(&self, token: Token) -> Option<usize> {
        let tokens = self.tokens.lock().unwrap();
        tokens.get(&token).copied()
    }

    pub(crate) fn deregister_socket(&self, _py: &PyToken<'_>, socket_ptr: *mut u8) {
        if socket_ptr.is_null() {
            return;
        }
        let socket_id = socket_ptr as usize;
        let mut waiters = self.waiters.lock().unwrap();
        let mut sockets = self.sockets.lock().unwrap();
        let entry = sockets.remove(&socket_id);
        if let Some(entry) = entry {
            self.tokens.lock().unwrap().remove(&entry.token);
            let mut ready_futures: Vec<PtrSlot> = Vec::new();
            for waiter in entry.waiters {
                waiters.remove(&waiter);
                ready_futures.push(waiter);
            }
            for waiter in entry.blocking_waiters {
                let mut guard = waiter.ready.lock().unwrap();
                *guard = Some(IO_EVENT_ERROR);
                drop(guard);
                waiter.condvar.notify_all();
            }
            drop(waiters);
            drop(sockets);
            let _ = with_socket_mut(socket_ptr, |sock| {
                let source = sock.source_mut().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                })?;
                self.poll.lock().unwrap().registry().deregister(source)
            });
            for future in ready_futures {
                self.mark_ready(future, IO_EVENT_ERROR);
                let tasks = await_waiters_take(_py, future.0);
                for waiter in tasks {
                    wake_task_ptr(_py, waiter.0);
                }
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
        let token = sockets
            .get(&socket_id)
            .map(|entry| entry.token)
            .unwrap_or_else(|| {
                let token = Token(self.next_token.fetch_add(1, AtomicOrdering::Relaxed));
                sockets.insert(
                    socket_id,
                    IoSocketEntry {
                        token,
                        interests: Interest::READABLE,
                        waiters: Vec::new(),
                        blocking_waiters: Vec::new(),
                    },
                );
                self.tokens.lock().unwrap().insert(token, socket_id);
                token
            });
        let entry = sockets.get_mut(&socket_id).expect("socket entry");
        entry.blocking_waiters.push(Arc::clone(&waiter));
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
        drop(sockets);
        if updated {
            with_socket_mut(socket_ptr, |sock| {
                if needs_register {
                    {
                        let source = sock.source_mut().ok_or_else(|| {
                            std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                        })?;
                        self.poll
                            .lock()
                            .unwrap()
                            .registry()
                            .register(source, token, interests)
                    }
                } else {
                    {
                        let source = sock.source_mut().ok_or_else(|| {
                            std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                        })?;
                        self.poll
                            .lock()
                            .unwrap()
                            .registry()
                            .reregister(source, token, interests)
                    }
                }
            })?;
        }
        let _ = self.waker.wake();
        let deadline = timeout.map(|dur| Instant::now() + dur);
        let mut guard = waiter.ready.lock().unwrap();
        loop {
            if let Some(ready) = *guard {
                return Ok(ready);
            }
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let timeout = deadline - now;
                let (next, _) = waiter.condvar.wait_timeout(guard, timeout).unwrap();
                guard = next;
            } else {
                guard = waiter.condvar.wait(guard).unwrap();
            }
        }
        drop(guard);
        let mut sockets = self.sockets.lock().unwrap();
        if let Some(entry) = sockets.get_mut(&socket_id) {
            entry
                .blocking_waiters
                .retain(|candidate| Arc::as_ptr(candidate) as usize != waiter_id);
            if entry.waiters.is_empty() && entry.blocking_waiters.is_empty() {
                let token = entry.token;
                sockets.remove(&socket_id);
                self.tokens.lock().unwrap().remove(&token);
                drop(sockets);
                let _ = with_socket_mut(socket_ptr, |sock| {
                    let source = sock.source_mut().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidInput, "socket not pollable")
                    })?;
                    self.poll.lock().unwrap().registry().deregister(source)
                });
            }
        }
        Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn io_worker(poller: Arc<IoPoller>) {
    loop {
        if !poller.running.load(AtomicOrdering::Acquire) {
            break;
        }
        let mut events = poller.events.lock().unwrap();
        let _ = poller
            .poll
            .lock()
            .unwrap()
            .poll(&mut *events, Some(Duration::from_millis(250)));
        if !poller.running.load(AtomicOrdering::Acquire) {
            break;
        }
        let mut ready_futures: Vec<(PtrSlot, u32)> = Vec::new();
        {
            let mut waiters = poller.waiters.lock().unwrap();
            let mut sockets = poller.sockets.lock().unwrap();
            for event in events.iter() {
                if event.token() == Token(0) {
                    continue;
                }
                let Some(socket_id) = poller.socket_for_token(event.token()) else {
                    continue;
                };
                let Some(entry) = sockets.get_mut(&socket_id) else {
                    continue;
                };
                let mut ready_mask = 0;
                if event.is_readable() {
                    ready_mask |= IO_EVENT_READ;
                }
                if event.is_writable() {
                    ready_mask |= IO_EVENT_WRITE;
                }
                if event.is_error() || event.is_read_closed() || event.is_write_closed() {
                    ready_mask |= IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
                }
                if ready_mask == 0 {
                    continue;
                }
                let mut remaining: Vec<PtrSlot> = Vec::with_capacity(entry.waiters.len());
                for waiter in entry.waiters.drain(..) {
                    if let Some(info) = waiters.get(&waiter) {
                        if (info.events & ready_mask) != 0 {
                            ready_futures.push((waiter, ready_mask));
                            waiters.remove(&waiter);
                        } else {
                            remaining.push(waiter);
                        }
                    }
                }
                entry.waiters = remaining;
                if !entry.blocking_waiters.is_empty() {
                    let mut remaining_blocking: Vec<Arc<BlockingWaiter>> =
                        Vec::with_capacity(entry.blocking_waiters.len());
                    for waiter in entry.blocking_waiters.drain(..) {
                        if (waiter.events & ready_mask) != 0 {
                            let mut guard = waiter.ready.lock().unwrap();
                            *guard = Some(ready_mask);
                            drop(guard);
                            waiter.condvar.notify_all();
                        } else {
                            remaining_blocking.push(waiter);
                        }
                    }
                    entry.blocking_waiters = remaining_blocking;
                }
            }
        }
        drop(events);
        if !ready_futures.is_empty() {
            let gil = GilGuard::new();
            let py = gil.token();
            for (future, mask) in ready_futures {
                poller.mark_ready(future, mask);
                let waiters = await_waiters_take(&py, future.0);
                for waiter in waiters {
                    wake_task_ptr(&py, waiter.0);
                }
            }
        }
    }
}



#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_io_wait(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
    let obj_ptr = ptr_from_bits(obj_bits);
    if obj_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let header = header_from_obj_ptr(obj_ptr);
    let payload_bytes = (*header)
        .size
        .saturating_sub(std::mem::size_of::<MoltHeader>());
    let payload_len = payload_bytes / std::mem::size_of::<u64>();
    if payload_len < 2 {
        return raise_exception::<i64>(_py, "TypeError", "io wait payload too small");
    }
    let payload_ptr = obj_ptr as *mut u64;
    let socket_bits = *payload_ptr;
    let events_bits = *payload_ptr.add(1);
    let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
    if socket_ptr.is_null() {
        return raise_exception::<i64>(_py, "TypeError", "invalid socket");
    }
    let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
    if events == 0 {
        return raise_exception::<i64>(_py, "ValueError", "events must be non-zero");
    }
    if (*header).state == 0 {
        let mut timeout: Option<f64> = None;
        if payload_len >= 3 {
            let timeout_bits = *payload_ptr.add(2);
            let timeout_obj = obj_from_bits(timeout_bits);
            if !timeout_obj.is_none() {
                if let Some(val) = to_f64(timeout_obj) {
                    if !val.is_finite() || val < 0.0 {
                        return raise_exception::<i64>(_py,
                            "ValueError",
                            "timeout must be non-negative",
                        );
                    }
                    timeout = Some(val);
                } else {
                    return raise_exception::<i64>(_py, "TypeError", "timeout must be float or None");
                }
            }
        }
        if let Some(val) = timeout {
            if val == 0.0 {
                return raise_exception::<i64>(_py, "TimeoutError", "timed out");
            }
            let deadline = monotonic_now_secs(_py) + val;
            let deadline_bits = MoltObject::from_float(deadline).bits();
            if payload_len >= 3 {
                dec_ref_bits(_py, *payload_ptr.add(2));
                *payload_ptr.add(2) = deadline_bits;
                inc_ref_bits(_py, deadline_bits);
            }
        }
        if let Err(err) = runtime_state(_py)
            .io_poller()
            .register_wait(obj_ptr, socket_ptr, events)
        {
            return raise_os_error::<i64>(_py, err, "io_wait");
        }
        (*header).state = 1;
        return pending_bits_i64();
    }
    if let Some(mask) = runtime_state(_py).io_poller().take_ready(obj_ptr) {
        let res_bits = MoltObject::from_int(mask as i64).bits();
        return res_bits as i64;
    }
    if payload_len >= 3 {
        let deadline_obj = obj_from_bits(*payload_ptr.add(2));
        if let Some(deadline) = to_f64(deadline_obj) {
            if deadline.is_finite() && monotonic_now_secs(_py) >= deadline {
                runtime_state(_py).io_poller().cancel_waiter(obj_ptr);
                return raise_exception::<i64>(_py, "TimeoutError", "timed out");
            }
        }
    }
    pending_bits_i64()

    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_io_wait_new(socket_bits: u64, events_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
    if require_net_capability::<u64>(_py, &["net", "net.poll"]).is_err() {
        return MoltObject::none().bits();
    }
    let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
    if socket_ptr.is_null() {
        return raise_exception::<_>(_py, "TypeError", "invalid socket");
    }
    let events = match to_i64(obj_from_bits(events_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>(_py, "TypeError", "events must be int"),
    };
    if events == 0 {
        return raise_exception::<_>(_py, "ValueError", "events must be non-zero");
    }
    let obj_bits = molt_future_new(
        io_wait_poll_fn_addr(),
        (3 * std::mem::size_of::<u64>()) as u64,
    );
    let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
        return MoltObject::none().bits();
    };
    unsafe {
        let payload_ptr = obj_ptr as *mut u64;
        *payload_ptr = socket_bits;
        *payload_ptr.add(1) = events_bits;
        *payload_ptr.add(2) = timeout_bits;
        inc_ref_bits(_py, events_bits);
        inc_ref_bits(_py, timeout_bits);
    }
    socket_ref_inc(socket_ptr);
    obj_bits

    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_io_wait_new(
    _socket_bits: u64,
    _events_bits: u64,
    _timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
    return raise_exception::<_>(_py, "RuntimeError", "io wait unsupported on wasm");

    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_io_wait(_obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
    // TODO(wasm-parity, owner:runtime, milestone:RT2, priority:P0, status:missing): wire io_wait to wasm host I/O readiness once wasm sockets land.
    raise_exception::<i64>(_py, "RuntimeError", "io wait unsupported on wasm")

    })
}
