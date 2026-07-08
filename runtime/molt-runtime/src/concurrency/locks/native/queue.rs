//! Native queue synchronization primitives.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::{
    GilReleaseGuard, MoltObject, PyToken, dec_ref_bits, exception_pending, inc_ref_bits, is_truthy,
    obj_from_bits, opaque_handle_bits, ptr_from_bits, raise_exception, release_ptr, to_f64,
};

#[cfg(not(target_arch = "wasm32"))]
struct MoltQueue {
    state: Mutex<QueueState>,
    not_empty: Condvar,
    not_full: Condvar,
    all_tasks_done: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
struct QueueState {
    kind: QueueKind,
    items: VecDeque<u64>,
    maxsize: i64,
    unfinished_tasks: u64,
    is_shutdown: bool,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum QueueKind {
    Fifo,
    Lifo,
    Priority,
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltQueue {
    fn new(maxsize: i64, kind: QueueKind) -> Self {
        Self {
            state: Mutex::new(QueueState {
                kind,
                items: VecDeque::new(),
                maxsize,
                unfinished_tasks: 0,
                is_shutdown: false,
            }),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
            all_tasks_done: Condvar::new(),
        }
    }

    fn qsize(&self) -> i64 {
        self.state.lock().unwrap().items.len() as i64
    }

    fn empty(&self) -> bool {
        self.state.lock().unwrap().items.is_empty()
    }

    fn full(&self) -> bool {
        let guard = self.state.lock().unwrap();
        guard.maxsize > 0 && (guard.items.len() as i64) >= guard.maxsize
    }

    fn is_shutdown(&self) -> bool {
        self.state.lock().unwrap().is_shutdown
    }

    fn kind(&self) -> QueueKind {
        self.state.lock().unwrap().kind
    }

    fn pop_item_locked(guard: &mut QueueState) -> Option<u64> {
        match guard.kind {
            QueueKind::Fifo | QueueKind::Priority => guard.items.pop_front(),
            QueueKind::Lifo => guard.items.pop_back(),
        }
    }

    fn priority_insert_locked(
        _py: &PyToken<'_>,
        guard: &mut QueueState,
        item_bits: u64,
    ) -> Result<(), u64> {
        let mut insert_index = guard.items.len();
        for (idx, existing_bits) in guard.items.iter().copied().enumerate() {
            let lt_bits = crate::molt_lt(item_bits, existing_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            if is_truthy(_py, obj_from_bits(lt_bits)) {
                insert_index = idx;
                break;
            }
        }
        guard.items.insert(insert_index, item_bits);
        Ok(())
    }

    fn put(&self, item_bits: u64, blocking: bool, timeout: Option<Duration>) -> bool {
        let mut guard = self.state.lock().unwrap();
        if guard.kind == QueueKind::Priority {
            return false;
        }
        if guard.is_shutdown {
            return false;
        }
        if guard.maxsize <= 0 || (guard.items.len() as i64) < guard.maxsize {
            guard.items.push_back(item_bits);
            guard.unfinished_tasks = guard.unfinished_tasks.saturating_add(1);
            self.not_empty.notify_one();
            return true;
        }
        if !blocking {
            return false;
        }
        match timeout {
            Some(wait) if wait == Duration::ZERO => false,
            Some(wait) => {
                let start = Instant::now();
                let mut remaining = wait;
                loop {
                    let (next, timed) = self.not_full.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if guard.is_shutdown {
                        return false;
                    }
                    if guard.maxsize <= 0 || (guard.items.len() as i64) < guard.maxsize {
                        guard.items.push_back(item_bits);
                        guard.unfinished_tasks = guard.unfinished_tasks.saturating_add(1);
                        self.not_empty.notify_one();
                        return true;
                    }
                    if timed.timed_out() {
                        return false;
                    }
                    let elapsed = start.elapsed();
                    if elapsed >= wait {
                        return false;
                    }
                    remaining = wait.saturating_sub(elapsed);
                }
            }
            None => loop {
                guard = self.not_full.wait(guard).unwrap();
                if guard.is_shutdown {
                    return false;
                }
                if guard.maxsize <= 0 || (guard.items.len() as i64) < guard.maxsize {
                    guard.items.push_back(item_bits);
                    guard.unfinished_tasks = guard.unfinished_tasks.saturating_add(1);
                    self.not_empty.notify_one();
                    return true;
                }
            },
        }
    }

    fn try_put_priority(&self, _py: &PyToken<'_>, item_bits: u64) -> Result<bool, u64> {
        let mut guard = self.state.lock().unwrap();
        if guard.is_shutdown {
            return Ok(false);
        }
        if guard.maxsize > 0 && (guard.items.len() as i64) >= guard.maxsize {
            return Ok(false);
        }
        Self::priority_insert_locked(_py, &mut guard, item_bits)?;
        guard.unfinished_tasks = guard.unfinished_tasks.saturating_add(1);
        self.not_empty.notify_one();
        Ok(true)
    }

    fn wait_not_full(&self, timeout: Option<Duration>) -> bool {
        let mut guard = self.state.lock().unwrap();
        if guard.is_shutdown {
            return false;
        }
        if guard.maxsize <= 0 || (guard.items.len() as i64) < guard.maxsize {
            return true;
        }
        match timeout {
            Some(wait) if wait == Duration::ZERO => false,
            Some(wait) => {
                let start = Instant::now();
                let mut remaining = wait;
                loop {
                    let (next, timed) = self.not_full.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if guard.is_shutdown {
                        return false;
                    }
                    if guard.maxsize <= 0 || (guard.items.len() as i64) < guard.maxsize {
                        return true;
                    }
                    if timed.timed_out() {
                        return false;
                    }
                    let elapsed = start.elapsed();
                    if elapsed >= wait {
                        return false;
                    }
                    remaining = wait.saturating_sub(elapsed);
                }
            }
            None => loop {
                guard = self.not_full.wait(guard).unwrap();
                if guard.is_shutdown {
                    return false;
                }
                if guard.maxsize <= 0 || (guard.items.len() as i64) < guard.maxsize {
                    return true;
                }
            },
        }
    }

    fn put_priority(
        &self,
        _py: &PyToken<'_>,
        item_bits: u64,
        blocking: bool,
        timeout: Option<Duration>,
    ) -> Result<bool, u64> {
        let start = Instant::now();
        loop {
            if self.try_put_priority(_py, item_bits)? {
                return Ok(true);
            }
            if !blocking {
                return Ok(false);
            }
            let wait_for = match timeout {
                Some(total) => {
                    let elapsed = start.elapsed();
                    if elapsed >= total {
                        return Ok(false);
                    }
                    Some(total.saturating_sub(elapsed))
                }
                None => None,
            };
            let ready = {
                let _release = GilReleaseGuard::new();
                self.wait_not_full(wait_for)
            };
            if !ready {
                return Ok(false);
            }
        }
    }

    fn get(&self, blocking: bool, timeout: Option<Duration>) -> Option<u64> {
        let mut guard = self.state.lock().unwrap();
        if let Some(item_bits) = Self::pop_item_locked(&mut guard) {
            if guard.maxsize > 0 {
                self.not_full.notify_one();
            }
            return Some(item_bits);
        }
        if guard.is_shutdown {
            return None;
        }
        if !blocking {
            return None;
        }
        match timeout {
            Some(wait) if wait == Duration::ZERO => None,
            Some(wait) => {
                let start = Instant::now();
                let mut remaining = wait;
                loop {
                    let (next, timed) = self.not_empty.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if let Some(item_bits) = Self::pop_item_locked(&mut guard) {
                        if guard.maxsize > 0 {
                            self.not_full.notify_one();
                        }
                        return Some(item_bits);
                    }
                    if guard.is_shutdown {
                        return None;
                    }
                    if timed.timed_out() {
                        return None;
                    }
                    let elapsed = start.elapsed();
                    if elapsed >= wait {
                        return None;
                    }
                    remaining = wait.saturating_sub(elapsed);
                }
            }
            None => loop {
                guard = self.not_empty.wait(guard).unwrap();
                if let Some(item_bits) = Self::pop_item_locked(&mut guard) {
                    if guard.maxsize > 0 {
                        self.not_full.notify_one();
                    }
                    return Some(item_bits);
                }
                if guard.is_shutdown {
                    return None;
                }
            },
        }
    }

    fn shutdown(&self, _py: &PyToken<'_>, immediate: bool) {
        let mut guard = self.state.lock().unwrap();
        guard.is_shutdown = true;
        if immediate {
            let mut drained = 0u64;
            while let Some(bits) = Self::pop_item_locked(&mut guard) {
                dec_ref_bits(_py, bits);
                drained = drained.saturating_add(1);
            }
            guard.unfinished_tasks = guard.unfinished_tasks.saturating_sub(drained);
            self.all_tasks_done.notify_all();
        }
        self.not_empty.notify_all();
        self.not_full.notify_all();
    }

    fn task_done(&self) -> bool {
        let mut guard = self.state.lock().unwrap();
        if guard.unfinished_tasks == 0 {
            return false;
        }
        guard.unfinished_tasks = guard.unfinished_tasks.saturating_sub(1);
        if guard.unfinished_tasks == 0 {
            self.all_tasks_done.notify_all();
        }
        true
    }

    fn join(&self) {
        let mut guard = self.state.lock().unwrap();
        while guard.unfinished_tasks > 0 {
            guard = self.all_tasks_done.wait(guard).unwrap();
        }
    }

    fn drop_items(&self, _py: &PyToken<'_>) {
        let mut guard = self.state.lock().unwrap();
        while let Some(bits) = guard.items.pop_front() {
            dec_ref_bits(_py, bits);
        }
        guard.unfinished_tasks = 0;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn queue_from_bits(bits: u64) -> Option<Arc<MoltQueue>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltQueue);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_queue_timeout(
    _py: &PyToken<'_>,
    blocking: bool,
    timeout_bits: u64,
    op_name: &str,
) -> Result<Option<Duration>, u64> {
    let timeout_obj = obj_from_bits(timeout_bits);
    if timeout_obj.is_none() {
        return Ok(None);
    }
    if !blocking {
        let msg = format!("can't specify a timeout for a non-blocking {op_name}");
        return Err(raise_exception::<_>(_py, "ValueError", &msg));
    }
    let Some(timeout) = to_f64(timeout_obj) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "timeout value must be a float",
        ));
    };
    if !timeout.is_finite() || timeout < 0.0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "'timeout' must be a non-negative number",
        ));
    }
    Ok(Some(Duration::from_secs_f64(timeout)))
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_new(maxsize_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(maxsize) = crate::to_i64(obj_from_bits(maxsize_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxsize must be an integer");
        };
        let queue = Arc::new(MoltQueue::new(maxsize, QueueKind::Fifo));
        let raw = Arc::into_raw(queue) as *mut u8;
        opaque_handle_bits(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_lifo_new(maxsize_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(maxsize) = crate::to_i64(obj_from_bits(maxsize_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxsize must be an integer");
        };
        let queue = Arc::new(MoltQueue::new(maxsize, QueueKind::Lifo));
        let raw = Arc::into_raw(queue) as *mut u8;
        opaque_handle_bits(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_priority_new(maxsize_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(maxsize) = crate::to_i64(obj_from_bits(maxsize_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxsize must be an integer");
        };
        let queue = Arc::new(MoltQueue::new(maxsize, QueueKind::Priority));
        let raw = Arc::into_raw(queue) as *mut u8;
        opaque_handle_bits(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_qsize(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        MoltObject::from_int(queue.qsize()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_empty(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        MoltObject::from_bool(queue.empty()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_full(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        MoltObject::from_bool(queue.full()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_put(
    handle_bits: u64,
    item_bits: u64,
    blocking_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        let blocking = is_truthy(_py, obj_from_bits(blocking_bits));
        let timeout = match parse_queue_timeout(_py, blocking, timeout_bits, "put") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        inc_ref_bits(_py, item_bits);
        let pushed = if queue.kind() == QueueKind::Priority {
            match queue.put_priority(_py, item_bits, blocking, timeout) {
                Ok(value) => value,
                Err(err_bits) => {
                    dec_ref_bits(_py, item_bits);
                    return err_bits;
                }
            }
        } else {
            {
                let _release = GilReleaseGuard::new();
                queue.put(item_bits, blocking, timeout)
            }
        };
        if !pushed {
            dec_ref_bits(_py, item_bits);
        }
        MoltObject::from_bool(pushed).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_get(
    handle_bits: u64,
    blocking_bits: u64,
    timeout_bits: u64,
    sentinel_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        let blocking = is_truthy(_py, obj_from_bits(blocking_bits));
        let timeout = match parse_queue_timeout(_py, blocking, timeout_bits, "get") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = {
            let _release = GilReleaseGuard::new();
            queue.get(blocking, timeout)
        };
        match out {
            Some(bits) => bits,
            None => {
                inc_ref_bits(_py, sentinel_bits);
                sentinel_bits
            }
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_shutdown(handle_bits: u64, immediate_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        let immediate = is_truthy(_py, obj_from_bits(immediate_bits));
        queue.shutdown(_py, immediate);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_is_shutdown(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        MoltObject::from_bool(queue.is_shutdown()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_task_done(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        MoltObject::from_bool(queue.task_done()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_join(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(queue) = queue_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid queue handle");
        };
        {
            let _release = GilReleaseGuard::new();
            queue.join();
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_queue_drop(handle_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let ptr = ptr_from_bits(handle_bits);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            release_ptr(ptr);
            let queue = Arc::from_raw(ptr as *const MoltQueue);
            queue.drop_items(_py);
            MoltObject::none().bits()
        })
    }
}
