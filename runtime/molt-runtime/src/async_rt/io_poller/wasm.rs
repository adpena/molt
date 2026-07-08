use super::*;

#[cfg(target_arch = "wasm32")]
struct IoWaiter {
    socket_handle: i64,
    events: u32,
    is_ws: bool,
}

#[cfg(molt_has_net_io)]
struct IoSocketEntry {
    token: Token,
    interests: Interest,
    waiters: WaiterList,
    blocking_waiters: BlockingWaiterList,
    source: IoSource,
    debug_fd: i64,
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
    poll: Mutex<Poll>,
    registry: Registry,
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

#[cfg(target_arch = "wasm32")]
pub(crate) struct IoPoller {
    waiters: Mutex<HashMap<PtrSlot, IoWaiter>>,
    ready: Mutex<HashMap<PtrSlot, u32>>,
}

#[cfg(target_arch = "wasm32")]
impl IoPoller {
    pub(crate) fn new() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
            ready: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn shutdown(&self) {}

    pub(crate) fn register_wait(
        &self,
        future_ptr: *mut u8,
        socket_handle: i64,
        events: u32,
    ) -> Result<(), std::io::Error> {
        if future_ptr.is_null() || socket_handle < 0 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "invalid io wait",
            ));
        }
        let waiter_key = PtrSlot(future_ptr);
        let mut waiters = self.waiters.lock().unwrap();
        if waiters.contains_key(&waiter_key) {
            return Ok(());
        }
        waiters.insert(
            waiter_key,
            IoWaiter {
                socket_handle,
                events,
                is_ws: false,
            },
        );
        Ok(())
    }

    pub(crate) fn register_ws_wait(
        &self,
        future_ptr: *mut u8,
        ws_handle: i64,
        events: u32,
    ) -> Result<(), std::io::Error> {
        if future_ptr.is_null() || ws_handle < 0 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "invalid ws wait",
            ));
        }
        let waiter_key = PtrSlot(future_ptr);
        let mut waiters = self.waiters.lock().unwrap();
        if waiters.contains_key(&waiter_key) {
            return Ok(());
        }
        waiters.insert(
            waiter_key,
            IoWaiter {
                socket_handle: ws_handle,
                events,
                is_ws: true,
            },
        );
        Ok(())
    }

    pub(crate) fn cancel_waiter(&self, future_ptr: *mut u8) {
        if future_ptr.is_null() {
            return;
        }
        let waiter_key = PtrSlot(future_ptr);
        let mut waiters = self.waiters.lock().unwrap();
        waiters.remove(&waiter_key);
        let mut ready = self.ready.lock().unwrap();
        ready.remove(&waiter_key);
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

    pub(crate) fn poll_host(&self, _py: &PyToken<'_>) {
        let snapshot: Vec<(PtrSlot, i64, u32, bool)> = {
            let waiters = self.waiters.lock().unwrap();
            waiters
                .iter()
                .map(|(key, waiter)| (*key, waiter.socket_handle, waiter.events, waiter.is_ws))
                .collect()
        };
        if snapshot.is_empty() {
            return;
        }
        let mut ready: Vec<(PtrSlot, u32)> = Vec::new();
        for (future, handle, events, is_ws) in snapshot {
            let rc = if is_ws {
                unsafe { crate::molt_ws_poll_host(handle, events) }
            } else {
                unsafe { crate::molt_socket_poll_host(handle, events) }
            };
            if rc == 0 {
                continue;
            }
            let mask = if rc < 0 {
                IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE
            } else {
                rc as u32
            };
            ready.push((future, mask));
        }
        if ready.is_empty() {
            return;
        }
        {
            let mut waiters = self.waiters.lock().unwrap();
            for (future, _) in &ready {
                waiters.remove(future);
            }
        }
        for (future, mask) in ready {
            self.mark_ready(future, mask);
            let _ = wake_await_waiters(_py, future.0);
        }
    }
}
