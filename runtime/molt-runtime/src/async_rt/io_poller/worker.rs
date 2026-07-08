use super::*;

pub(super) fn io_worker(poller: Arc<IoPoller>) {
    loop {
        if !poller.running.load(AtomicOrdering::Acquire) {
            break;
        }
        let mut events = poller.events.lock().unwrap();
        let _ = poller
            .poll
            .lock()
            .unwrap()
            .poll(&mut events, Some(Duration::from_millis(250)));
        if !poller.running.load(AtomicOrdering::Acquire) {
            break;
        }
        let mut ready_futures: Vec<(PtrSlot, u32, usize, i64)> = Vec::new();
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
                for waiter in entry.waiters.drain() {
                    if let Some(info) = waiters.get(&waiter) {
                        if (info.events & ready_mask) != 0 {
                            if trace_io_poller() {
                                let fd = entry.debug_fd;
                                eprintln!(
                                    "molt io poller: event socket=0x{:x} fd={} future=0x{:x} ready_mask={} interest={}",
                                    socket_id, fd, waiter.0 as usize, ready_mask, info.events
                                );
                            }
                            ready_futures.push((waiter, ready_mask, socket_id, entry.debug_fd));
                            waiters.remove(&waiter);
                        } else {
                            remaining.push(waiter);
                        }
                    }
                }
                entry.waiters.replace_with(remaining);
                if !entry.blocking_waiters.is_empty() {
                    for waiter in entry.blocking_waiters.drain_ready(ready_mask) {
                        let mut guard = waiter.ready.lock().unwrap();
                        *guard = Some(ready_mask);
                        drop(guard);
                        waiter.condvar.notify_all();
                    }
                }
            }
        }
        drop(events);
        if !ready_futures.is_empty() {
            // Record readiness before taking the GIL so polling threads can observe
            // ready masks even if wake propagation is temporarily delayed.
            for (future, mask, _, _) in &ready_futures {
                poller.mark_ready(*future, *mask);
            }
            let gil = GilGuard::new();
            let py = gil.token();
            for (future, mask, socket_id, debug_fd) in ready_futures {
                let waiter_count = wake_await_waiters(&py, future.0);
                if trace_io_poller() {
                    eprintln!(
                        "molt io poller: ready future=0x{:x} socket=0x{:x} fd={} mask={} waiters={}",
                        future.0 as usize, socket_id, debug_fd, mask, waiter_count
                    );
                }
            }
        }
    }
}
