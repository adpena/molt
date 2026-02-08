use crate::{MoltObject, PyToken};

#[cfg(not(target_arch = "wasm32"))]
use super::current_thread_id;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use crate::{
    attr_name_bits_from_bytes, bits_from_ptr, call_callable0, call_callable1, dec_ref_bits,
    exception_pending, inc_ref_bits, is_truthy, missing_bits, molt_getattr_builtin,
    molt_is_callable, monotonic_now_secs, obj_from_bits, ptr_from_bits, raise_exception,
    release_ptr, to_f64, GilReleaseGuard,
};

#[cfg(not(target_arch = "wasm32"))]
struct MoltLock {
    state: Mutex<LockState>,
    cvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct LockState {
    locked: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltLock {
    fn new() -> Self {
        Self {
            state: Mutex::new(LockState { locked: false }),
            cvar: Condvar::new(),
        }
    }

    fn try_acquire(&self) -> bool {
        let mut guard = self.state.lock().unwrap();
        if guard.locked {
            return false;
        }
        guard.locked = true;
        true
    }

    fn acquire(&self, timeout: Option<Duration>) -> bool {
        let mut guard = self.state.lock().unwrap();
        if !guard.locked {
            guard.locked = true;
            return true;
        }
        match timeout {
            Some(wait) if wait == Duration::ZERO => false,
            Some(wait) => {
                let start = Instant::now();
                let mut remaining = wait;
                loop {
                    let (next, res) = self.cvar.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if !guard.locked {
                        guard.locked = true;
                        return true;
                    }
                    if res.timed_out() {
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
                guard = self.cvar.wait(guard).unwrap();
                if !guard.locked {
                    guard.locked = true;
                    return true;
                }
            },
        }
    }

    fn release(&self) -> bool {
        let mut guard = self.state.lock().unwrap();
        if !guard.locked {
            return false;
        }
        guard.locked = false;
        self.cvar.notify_one();
        true
    }

    fn locked(&self) -> bool {
        self.state.lock().unwrap().locked
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltRLock {
    state: Mutex<RLockState>,
    cvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct RLockState {
    locked: bool,
    owner: u64,
    count: u64,
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltCondition {
    state: Mutex<ConditionState>,
    cvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct ConditionState {
    waiters: u64,
    notify_seq: u64,
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltEvent {
    state: Mutex<bool>,
    cvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltSemaphore {
    state: Mutex<SemaphoreState>,
    cvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct SemaphoreState {
    value: i64,
    max_value: i64,
    bounded: bool,
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltBarrier {
    parties: u64,
    default_timeout: Option<Duration>,
    state: Mutex<BarrierState>,
    cvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct BarrierState {
    waiting: u64,
    generation: u64,
    broken: bool,
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltLocal {
    state: Mutex<HashMap<u64, u64>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltRLock {
    fn new() -> Self {
        Self {
            state: Mutex::new(RLockState {
                locked: false,
                owner: 0,
                count: 0,
            }),
            cvar: Condvar::new(),
        }
    }

    fn try_acquire(&self) -> bool {
        let tid = current_thread_id();
        let mut guard = self.state.lock().unwrap();
        if !guard.locked {
            guard.locked = true;
            guard.owner = tid;
            guard.count = 1;
            return true;
        }
        if guard.owner == tid {
            guard.count = guard.count.saturating_add(1);
            return true;
        }
        false
    }

    fn acquire(&self, timeout: Option<Duration>) -> bool {
        let tid = current_thread_id();
        let mut guard = self.state.lock().unwrap();
        if !guard.locked {
            guard.locked = true;
            guard.owner = tid;
            guard.count = 1;
            return true;
        }
        if guard.owner == tid {
            guard.count = guard.count.saturating_add(1);
            return true;
        }
        match timeout {
            Some(wait) if wait == Duration::ZERO => false,
            Some(wait) => {
                let start = Instant::now();
                let mut remaining = wait;
                loop {
                    let (next, res) = self.cvar.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if !guard.locked {
                        guard.locked = true;
                        guard.owner = tid;
                        guard.count = 1;
                        return true;
                    }
                    if res.timed_out() {
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
                guard = self.cvar.wait(guard).unwrap();
                if !guard.locked {
                    guard.locked = true;
                    guard.owner = tid;
                    guard.count = 1;
                    return true;
                }
            },
        }
    }

    fn release(&self) -> bool {
        let tid = current_thread_id();
        let mut guard = self.state.lock().unwrap();
        if !guard.locked || guard.owner != tid {
            return false;
        }
        guard.count = guard.count.saturating_sub(1);
        if guard.count == 0 {
            guard.locked = false;
            guard.owner = 0;
            self.cvar.notify_one();
        }
        true
    }

    fn locked(&self) -> bool {
        self.state.lock().unwrap().locked
    }

    fn is_owned(&self) -> bool {
        let tid = current_thread_id();
        let guard = self.state.lock().unwrap();
        guard.locked && guard.owner == tid
    }

    fn release_save(&self) -> Option<u64> {
        let tid = current_thread_id();
        let mut guard = self.state.lock().unwrap();
        if !guard.locked || guard.owner != tid {
            return None;
        }
        let saved = guard.count;
        guard.locked = false;
        guard.owner = 0;
        guard.count = 0;
        self.cvar.notify_one();
        Some(saved)
    }

    fn acquire_restore(&self, count: u64) {
        let tid = current_thread_id();
        let mut guard = self.state.lock().unwrap();
        while guard.locked && guard.owner != tid {
            guard = self.cvar.wait(guard).unwrap();
        }
        if !guard.locked {
            guard.locked = true;
            guard.owner = tid;
            guard.count = count.max(1);
            return;
        }
        guard.count = guard.count.saturating_add(count.max(1));
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltCondition {
    fn new() -> Self {
        Self {
            state: Mutex::new(ConditionState {
                waiters: 0,
                notify_seq: 0,
            }),
            cvar: Condvar::new(),
        }
    }

    fn wait(&self, timeout: Option<Duration>) -> bool {
        let mut guard = self.state.lock().unwrap();
        let expected_seq = guard.notify_seq;
        guard.waiters = guard.waiters.saturating_add(1);
        let result = match timeout {
            Some(wait) if wait == Duration::ZERO => false,
            Some(wait) => {
                let start = Instant::now();
                let mut remaining = wait;
                loop {
                    let (next, timed) = self.cvar.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if guard.notify_seq != expected_seq {
                        break true;
                    }
                    if timed.timed_out() {
                        break false;
                    }
                    let elapsed = start.elapsed();
                    if elapsed >= wait {
                        break false;
                    }
                    remaining = wait.saturating_sub(elapsed);
                }
            }
            None => loop {
                guard = self.cvar.wait(guard).unwrap();
                if guard.notify_seq != expected_seq {
                    break true;
                }
            },
        };
        guard.waiters = guard.waiters.saturating_sub(1);
        result
    }

    fn notify(&self, n: u64) {
        if n == 0 {
            return;
        }
        let mut guard = self.state.lock().unwrap();
        guard.notify_seq = guard.notify_seq.saturating_add(n);
        if n == 1 {
            self.cvar.notify_one();
        } else {
            self.cvar.notify_all();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltEvent {
    fn new() -> Self {
        Self {
            state: Mutex::new(false),
            cvar: Condvar::new(),
        }
    }

    fn is_set(&self) -> bool {
        *self.state.lock().unwrap()
    }

    fn set(&self) {
        let mut guard = self.state.lock().unwrap();
        *guard = true;
        self.cvar.notify_all();
    }

    fn clear(&self) {
        let mut guard = self.state.lock().unwrap();
        *guard = false;
    }

    fn wait(&self, timeout: Option<Duration>) -> bool {
        let mut guard = self.state.lock().unwrap();
        if *guard {
            return true;
        }
        match timeout {
            Some(wait) if wait == Duration::ZERO => false,
            Some(wait) => {
                let start = Instant::now();
                let mut remaining = wait;
                loop {
                    let (next, timed) = self.cvar.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if *guard {
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
                guard = self.cvar.wait(guard).unwrap();
                if *guard {
                    return true;
                }
            },
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltSemaphore {
    fn new(value: i64, bounded: bool) -> Self {
        Self {
            state: Mutex::new(SemaphoreState {
                value,
                max_value: value,
                bounded,
            }),
            cvar: Condvar::new(),
        }
    }

    fn acquire(&self, blocking: bool, timeout: Option<Duration>) -> bool {
        let mut guard = self.state.lock().unwrap();
        if guard.value > 0 {
            guard.value -= 1;
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
                    let (next, timed) = self.cvar.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if guard.value > 0 {
                        guard.value -= 1;
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
                guard = self.cvar.wait(guard).unwrap();
                if guard.value > 0 {
                    guard.value -= 1;
                    return true;
                }
            },
        }
    }

    fn release(&self, n: i64) -> Result<(), &'static str> {
        let mut guard = self.state.lock().unwrap();
        if guard.bounded && guard.value.saturating_add(n) > guard.max_value {
            return Err("Semaphore released too many times");
        }
        guard.value = guard.value.saturating_add(n);
        if n == 1 {
            self.cvar.notify_one();
        } else {
            self.cvar.notify_all();
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltBarrier {
    fn new(parties: u64, timeout: Option<Duration>) -> Self {
        Self {
            parties,
            default_timeout: timeout,
            state: Mutex::new(BarrierState {
                waiting: 0,
                generation: 0,
                broken: false,
            }),
            cvar: Condvar::new(),
        }
    }

    fn wait(&self, timeout: Option<Duration>) -> Result<u64, ()> {
        let mut guard = self.state.lock().unwrap();
        if guard.broken {
            return Err(());
        }
        let generation = guard.generation;
        let index = guard.waiting;
        guard.waiting = guard.waiting.saturating_add(1);
        if guard.waiting == self.parties {
            guard.waiting = 0;
            guard.generation = guard.generation.saturating_add(1);
            self.cvar.notify_all();
            return Ok(index);
        }
        let effective_timeout = timeout.or(self.default_timeout);
        let timed_out = match effective_timeout {
            None => loop {
                guard = self.cvar.wait(guard).unwrap();
                if guard.broken {
                    return Err(());
                }
                if guard.generation != generation {
                    return Ok(index);
                }
            },
            Some(wait) if wait == Duration::ZERO => true,
            Some(wait) => {
                let start = Instant::now();
                let mut remaining = wait;
                loop {
                    let (next, timed) = self.cvar.wait_timeout(guard, remaining).unwrap();
                    guard = next;
                    if guard.broken {
                        return Err(());
                    }
                    if guard.generation != generation {
                        return Ok(index);
                    }
                    if timed.timed_out() {
                        break true;
                    }
                    let elapsed = start.elapsed();
                    if elapsed >= wait {
                        break true;
                    }
                    remaining = wait.saturating_sub(elapsed);
                }
            }
        };
        if timed_out {
            guard.broken = true;
            guard.waiting = 0;
            guard.generation = guard.generation.saturating_add(1);
            self.cvar.notify_all();
            return Err(());
        }
        Ok(index)
    }

    fn abort(&self) {
        let mut guard = self.state.lock().unwrap();
        guard.broken = true;
        guard.waiting = 0;
        guard.generation = guard.generation.saturating_add(1);
        self.cvar.notify_all();
    }

    fn reset(&self) {
        let mut guard = self.state.lock().unwrap();
        if guard.waiting > 0 {
            guard.broken = true;
            guard.waiting = 0;
            guard.generation = guard.generation.saturating_add(1);
            self.cvar.notify_all();
        }
        guard.broken = false;
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltLocal {
    fn new() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn lock_from_bits(bits: u64) -> Option<Arc<MoltLock>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltLock);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn rlock_from_bits(bits: u64) -> Option<Arc<MoltRLock>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltRLock);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn condition_from_bits(bits: u64) -> Option<Arc<MoltCondition>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltCondition);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn event_from_bits(bits: u64) -> Option<Arc<MoltEvent>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltEvent);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn semaphore_from_bits(bits: u64) -> Option<Arc<MoltSemaphore>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltSemaphore);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn barrier_from_bits(bits: u64) -> Option<Arc<MoltBarrier>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltBarrier);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn local_from_bits(bits: u64) -> Option<Arc<MoltLocal>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let arc = Arc::from_raw(ptr as *const MoltLocal);
        let cloned = arc.clone();
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_timeout(
    _py: &PyToken<'_>,
    timeout_bits: u64,
    blocking: bool,
) -> Result<Option<Duration>, u64> {
    let timeout_obj = obj_from_bits(timeout_bits);
    if timeout_obj.is_none() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "'NoneType' object cannot be interpreted as an integer or float",
        ));
    }
    let Some(timeout) = to_f64(timeout_obj) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "timeout value must be a float",
        ));
    };
    if !timeout.is_finite() {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "timeout value must be a non-negative number",
        ));
    }
    if !blocking {
        if timeout != -1.0 {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "can't specify a timeout for a non-blocking call",
            ));
        }
        return Ok(None);
    }
    if timeout < 0.0 && timeout != -1.0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "timeout value must be a non-negative number",
        ));
    }
    if timeout < 0.0 {
        return Ok(None);
    }
    Ok(Some(Duration::from_secs_f64(timeout)))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_optional_timeout(_py: &PyToken<'_>, timeout_bits: u64) -> Result<Option<Duration>, u64> {
    let timeout_obj = obj_from_bits(timeout_bits);
    if timeout_obj.is_none() {
        return Ok(None);
    }
    let Some(timeout) = to_f64(timeout_obj) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "timeout value must be a float",
        ));
    };
    if !timeout.is_finite() {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "timeout value must be a non-negative number",
        ));
    }
    if timeout < 0.0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "timeout value must be a non-negative number",
        ));
    }
    Ok(Some(Duration::from_secs_f64(timeout)))
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let lock = Arc::new(MoltLock::new());
        let raw = Arc::into_raw(lock) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_acquire(
    handle_bits: u64,
    blocking_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = lock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid lock handle");
        };
        let blocking = is_truthy(_py, obj_from_bits(blocking_bits));
        let timeout = match parse_timeout(_py, timeout_bits, blocking) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let acquired = if !blocking || matches!(timeout, Some(t) if t == Duration::ZERO) {
            lock.try_acquire()
        } else if lock.try_acquire() {
            true
        } else {
            let _release = GilReleaseGuard::new();
            lock.acquire(timeout)
        };
        MoltObject::from_bool(acquired).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_release(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = lock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid lock handle");
        };
        if !lock.release() {
            return raise_exception::<_>(_py, "RuntimeError", "release unlocked lock");
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_locked(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = lock_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(lock.locked()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Arc::from_raw(ptr as *const MoltLock);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let lock = Arc::new(MoltRLock::new());
        let raw = Arc::into_raw(lock) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_acquire(
    handle_bits: u64,
    blocking_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid rlock handle");
        };
        let blocking = is_truthy(_py, obj_from_bits(blocking_bits));
        let timeout = match parse_timeout(_py, timeout_bits, blocking) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let acquired = if !blocking || matches!(timeout, Some(t) if t == Duration::ZERO) {
            lock.try_acquire()
        } else if lock.try_acquire() {
            true
        } else {
            let _release = GilReleaseGuard::new();
            lock.acquire(timeout)
        };
        MoltObject::from_bool(acquired).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_release(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid rlock handle");
        };
        if !lock.release() {
            return raise_exception::<_>(_py, "RuntimeError", "cannot release un-acquired lock");
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_locked(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(lock.locked()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_is_owned(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(lock.is_owned()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_release_save(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid rlock handle");
        };
        match lock.release_save() {
            Some(saved) => MoltObject::from_int(saved as i64).bits(),
            None => raise_exception::<_>(_py, "RuntimeError", "cannot release un-acquired lock"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_acquire_restore(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid rlock handle");
        };
        let Some(count) = crate::to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "count must be an integer");
        };
        if count < 0 {
            return raise_exception::<_>(_py, "ValueError", "count must be >= 0");
        }
        lock.acquire_restore(count as u64);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Arc::from_raw(ptr as *const MoltRLock);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let condition = Arc::new(MoltCondition::new());
        let raw = Arc::into_raw(condition) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_wait(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(condition) = condition_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid condition handle");
        };
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let ok = {
            let _release = GilReleaseGuard::new();
            condition.wait(timeout)
        };
        MoltObject::from_bool(ok).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_wait_for(
    condition_bits: u64,
    predicate_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !is_truthy(_py, obj_from_bits(molt_is_callable(predicate_bits))) {
            return raise_exception::<_>(_py, "TypeError", "predicate must be callable");
        }
        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            let Some(value) = to_f64(obj_from_bits(timeout_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
            };
            Some(value)
        };
        let Some(wait_name_bits) = attr_name_bits_from_bytes(_py, b"wait") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let wait_bits = molt_getattr_builtin(condition_bits, wait_name_bits, missing);
        dec_ref_bits(_py, wait_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if wait_bits == missing {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "condition wait method is unavailable",
            );
        }
        if !is_truthy(_py, obj_from_bits(molt_is_callable(wait_bits))) {
            if obj_from_bits(wait_bits).as_ptr().is_some() {
                dec_ref_bits(_py, wait_bits);
            }
            return raise_exception::<_>(_py, "TypeError", "condition.wait must be callable");
        }
        let mut waittime = timeout;
        let mut deadline: Option<f64> = None;
        loop {
            let predicate_out = call_callable0(_py, predicate_bits);
            if exception_pending(_py) {
                if obj_from_bits(wait_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, wait_bits);
                }
                return MoltObject::none().bits();
            }
            let ok = is_truthy(_py, obj_from_bits(predicate_out));
            if obj_from_bits(predicate_out).as_ptr().is_some() {
                dec_ref_bits(_py, predicate_out);
            }
            if ok {
                if obj_from_bits(wait_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, wait_bits);
                }
                return MoltObject::from_bool(true).bits();
            }
            if let Some(current_wait) = waittime {
                if let Some(endtime) = deadline {
                    let remaining = endtime - monotonic_now_secs(_py);
                    waittime = Some(remaining);
                    if remaining <= 0.0 {
                        if obj_from_bits(wait_bits).as_ptr().is_some() {
                            dec_ref_bits(_py, wait_bits);
                        }
                        return MoltObject::from_bool(false).bits();
                    }
                } else {
                    deadline = Some(monotonic_now_secs(_py) + current_wait);
                }
            }
            let wait_arg = if let Some(current_wait) = waittime {
                MoltObject::from_float(current_wait).bits()
            } else {
                MoltObject::none().bits()
            };
            let wait_out = call_callable1(_py, wait_bits, wait_arg);
            if exception_pending(_py) {
                if obj_from_bits(wait_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, wait_bits);
                }
                return MoltObject::none().bits();
            }
            if obj_from_bits(wait_out).as_ptr().is_some() {
                dec_ref_bits(_py, wait_out);
            }
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_notify(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(condition) = condition_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid condition handle");
        };
        let Some(count) = crate::to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "notify count must be an integer");
        };
        if count <= 0 {
            return MoltObject::none().bits();
        }
        condition.notify(count as u64);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Arc::from_raw(ptr as *const MoltCondition);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_event_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let event = Arc::new(MoltEvent::new());
        let raw = Arc::into_raw(event) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_event_set(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(event) = event_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid event handle");
        };
        event.set();
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_event_clear(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(event) = event_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid event handle");
        };
        event.clear();
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_event_is_set(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(event) = event_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(event.is_set()).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_event_wait(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(event) = event_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid event handle");
        };
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let ok = {
            let _release = GilReleaseGuard::new();
            event.wait(timeout)
        };
        MoltObject::from_bool(ok).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_event_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Arc::from_raw(ptr as *const MoltEvent);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_semaphore_new(value_bits: u64, bounded_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = crate::to_i64(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "semaphore initial value must be an integer",
            );
        };
        if value < 0 {
            return raise_exception::<_>(_py, "ValueError", "semaphore initial value must be >= 0");
        }
        let bounded = is_truthy(_py, obj_from_bits(bounded_bits));
        let sem = Arc::new(MoltSemaphore::new(value, bounded));
        let raw = Arc::into_raw(sem) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_semaphore_acquire(
    handle_bits: u64,
    blocking_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sem) = semaphore_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid semaphore handle");
        };
        let blocking = is_truthy(_py, obj_from_bits(blocking_bits));
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let ok = {
            let _release = GilReleaseGuard::new();
            sem.acquire(blocking, timeout)
        };
        MoltObject::from_bool(ok).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_semaphore_release(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sem) = semaphore_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid semaphore handle");
        };
        let Some(count) = crate::to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "release count must be an integer");
        };
        if count < 1 {
            return raise_exception::<_>(_py, "ValueError", "semaphore release count must be >= 1");
        }
        if let Err(msg) = sem.release(count) {
            return raise_exception::<_>(_py, "ValueError", msg);
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_semaphore_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Arc::from_raw(ptr as *const MoltSemaphore);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_new(parties_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(parties) = crate::to_i64(obj_from_bits(parties_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "barrier parties must be an integer");
        };
        if parties <= 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "barrier parties must be greater than zero",
            );
        }
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let barrier = Arc::new(MoltBarrier::new(parties as u64, timeout));
        let raw = Arc::into_raw(barrier) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_wait(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = {
            let _release = GilReleaseGuard::new();
            barrier.wait(timeout)
        };
        match out {
            Ok(index) => MoltObject::from_int(index as i64).bits(),
            Err(()) => raise_exception::<_>(_py, "RuntimeError", "broken barrier"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_abort(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        barrier.abort();
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_reset(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        barrier.reset();
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_parties(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        MoltObject::from_int(barrier.parties as i64).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_n_waiting(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        let waiting = barrier.state.lock().unwrap().waiting;
        MoltObject::from_int(waiting as i64).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_broken(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        let broken = barrier.state.lock().unwrap().broken;
        MoltObject::from_bool(broken).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Arc::from_raw(ptr as *const MoltBarrier);
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_local_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let local = Arc::new(MoltLocal::new());
        let raw = Arc::into_raw(local) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_local_get_dict(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(local) = local_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid local handle");
        };
        let tid = current_thread_id();
        let mut guard = local.state.lock().unwrap();
        if let Some(bits) = guard.get(&tid).copied() {
            inc_ref_bits(_py, bits);
            return bits;
        }
        let dict_bits = crate::molt_dict_new(0);
        inc_ref_bits(_py, dict_bits);
        guard.insert(tid, dict_bits);
        dict_bits
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_local_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let local = Arc::from_raw(ptr as *const MoltLocal);
        let mut guard = local.state.lock().unwrap();
        for bits in guard.values().copied() {
            dec_ref_bits(_py, bits);
        }
        guard.clear();
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
use std::cell::Cell;
#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::collections::HashMap;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use std::time::Instant as WasmInstant;

#[cfg(target_arch = "wasm32")]
use crate::{
    attr_name_bits_from_bytes, bits_from_ptr, call_callable0, call_callable1, dec_ref_bits,
    exception_pending, inc_ref_bits, is_truthy, missing_bits, molt_getattr_builtin,
    molt_is_callable, monotonic_now_secs, obj_from_bits, ptr_from_bits, raise_exception,
    release_ptr, to_f64,
};

#[cfg(target_arch = "wasm32")]
struct MoltLock {
    locked: Cell<bool>,
}

#[cfg(target_arch = "wasm32")]
impl MoltLock {
    fn new() -> Self {
        Self {
            locked: Cell::new(false),
        }
    }

    fn try_acquire(&self) -> bool {
        if self.locked.get() {
            return false;
        }
        self.locked.set(true);
        true
    }

    fn release(&self) -> bool {
        if !self.locked.get() {
            return false;
        }
        self.locked.set(false);
        true
    }

    fn locked(&self) -> bool {
        self.locked.get()
    }
}

#[cfg(target_arch = "wasm32")]
impl MoltCondition {
    fn new() -> Self {
        Self {
            notify_seq: Cell::new(0),
        }
    }

    fn notify(&self, n: u64) {
        if n == 0 {
            return;
        }
        self.notify_seq.set(self.notify_seq.get().saturating_add(n));
    }
}

#[cfg(target_arch = "wasm32")]
impl MoltEvent {
    fn new() -> Self {
        Self {
            flag: Cell::new(false),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl MoltSemaphore {
    fn new(value: i64, bounded: bool) -> Self {
        Self {
            value: Cell::new(value),
            max_value: value,
            bounded,
        }
    }

    fn try_acquire(&self) -> bool {
        let val = self.value.get();
        if val <= 0 {
            return false;
        }
        self.value.set(val - 1);
        true
    }

    fn release(&self, n: i64) -> Result<(), &'static str> {
        let next = self.value.get().saturating_add(n);
        if self.bounded && next > self.max_value {
            return Err("Semaphore released too many times");
        }
        self.value.set(next);
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
impl MoltBarrier {
    fn new(parties: u64, timeout: Option<f64>) -> Self {
        Self {
            parties,
            default_timeout: Cell::new(timeout),
            waiting: Cell::new(0),
            generation: Cell::new(0),
            broken: Cell::new(false),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl MoltLocal {
    fn new() -> Self {
        Self {
            storage: RefCell::new(HashMap::new()),
        }
    }
}

#[cfg(target_arch = "wasm32")]
struct MoltRLock {
    locked: Cell<bool>,
    owner: Cell<u64>,
    count: Cell<u64>,
}

#[cfg(target_arch = "wasm32")]
struct MoltCondition {
    notify_seq: Cell<u64>,
}

#[cfg(target_arch = "wasm32")]
struct MoltEvent {
    flag: Cell<bool>,
}

#[cfg(target_arch = "wasm32")]
struct MoltSemaphore {
    value: Cell<i64>,
    max_value: i64,
    bounded: bool,
}

#[cfg(target_arch = "wasm32")]
struct MoltBarrier {
    parties: u64,
    default_timeout: Cell<Option<f64>>,
    waiting: Cell<u64>,
    generation: Cell<u64>,
    broken: Cell<bool>,
}

#[cfg(target_arch = "wasm32")]
struct MoltLocal {
    storage: RefCell<HashMap<u64, u64>>,
}

#[cfg(target_arch = "wasm32")]
impl MoltRLock {
    fn new() -> Self {
        Self {
            locked: Cell::new(false),
            owner: Cell::new(0),
            count: Cell::new(0),
        }
    }

    fn try_acquire(&self) -> bool {
        let tid = 1;
        if !self.locked.get() {
            self.locked.set(true);
            self.owner.set(tid);
            self.count.set(1);
            return true;
        }
        if self.owner.get() == tid {
            let count = self.count.get().saturating_add(1);
            self.count.set(count);
            return true;
        }
        false
    }

    fn release(&self) -> bool {
        let tid = 1;
        if !self.locked.get() || self.owner.get() != tid {
            return false;
        }
        let count = self.count.get().saturating_sub(1);
        self.count.set(count);
        if count == 0 {
            self.locked.set(false);
            self.owner.set(0);
        }
        true
    }

    fn locked(&self) -> bool {
        self.locked.get()
    }

    fn is_owned(&self) -> bool {
        let tid = 1;
        self.locked.get() && self.owner.get() == tid
    }

    fn release_save(&self) -> Option<u64> {
        let tid = 1;
        if !self.locked.get() || self.owner.get() != tid {
            return None;
        }
        let saved = self.count.get();
        self.locked.set(false);
        self.owner.set(0);
        self.count.set(0);
        Some(saved)
    }

    fn acquire_restore(&self, count: u64) {
        let tid = 1;
        if !self.locked.get() {
            self.locked.set(true);
            self.owner.set(tid);
            self.count.set(count.max(1));
            return;
        }
        if self.owner.get() == tid {
            let next = self.count.get().saturating_add(count.max(1));
            self.count.set(next);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn lock_from_bits(bits: u64) -> Option<Rc<MoltLock>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let rc = Rc::from_raw(ptr as *const MoltLock);
        let cloned = rc.clone();
        let _ = Rc::into_raw(rc);
        Some(cloned)
    }
}

#[cfg(target_arch = "wasm32")]
fn rlock_from_bits(bits: u64) -> Option<Rc<MoltRLock>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let rc = Rc::from_raw(ptr as *const MoltRLock);
        let cloned = rc.clone();
        let _ = Rc::into_raw(rc);
        Some(cloned)
    }
}

#[cfg(target_arch = "wasm32")]
fn condition_from_bits(bits: u64) -> Option<Rc<MoltCondition>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let rc = Rc::from_raw(ptr as *const MoltCondition);
        let cloned = rc.clone();
        let _ = Rc::into_raw(rc);
        Some(cloned)
    }
}

#[cfg(target_arch = "wasm32")]
fn event_from_bits(bits: u64) -> Option<Rc<MoltEvent>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let rc = Rc::from_raw(ptr as *const MoltEvent);
        let cloned = rc.clone();
        let _ = Rc::into_raw(rc);
        Some(cloned)
    }
}

#[cfg(target_arch = "wasm32")]
fn semaphore_from_bits(bits: u64) -> Option<Rc<MoltSemaphore>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let rc = Rc::from_raw(ptr as *const MoltSemaphore);
        let cloned = rc.clone();
        let _ = Rc::into_raw(rc);
        Some(cloned)
    }
}

#[cfg(target_arch = "wasm32")]
fn barrier_from_bits(bits: u64) -> Option<Rc<MoltBarrier>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let rc = Rc::from_raw(ptr as *const MoltBarrier);
        let cloned = rc.clone();
        let _ = Rc::into_raw(rc);
        Some(cloned)
    }
}

#[cfg(target_arch = "wasm32")]
fn local_from_bits(bits: u64) -> Option<Rc<MoltLocal>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let rc = Rc::from_raw(ptr as *const MoltLocal);
        let cloned = rc.clone();
        let _ = Rc::into_raw(rc);
        Some(cloned)
    }
}

#[cfg(target_arch = "wasm32")]
fn parse_timeout(_py: &PyToken<'_>, timeout_bits: u64, blocking: bool) -> Result<Option<f64>, u64> {
    let timeout_obj = obj_from_bits(timeout_bits);
    if timeout_obj.is_none() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "'NoneType' object cannot be interpreted as an integer or float",
        ));
    }
    let Some(timeout) = to_f64(timeout_obj) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "timeout value must be a float",
        ));
    };
    if !timeout.is_finite() {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "timeout value must be a non-negative number",
        ));
    }
    if !blocking {
        if timeout != -1.0 {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "can't specify a timeout for a non-blocking call",
            ));
        }
        return Ok(None);
    }
    if timeout < 0.0 && timeout != -1.0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "timeout value must be a non-negative number",
        ));
    }
    if timeout < 0.0 {
        return Ok(None);
    }
    Ok(Some(timeout))
}

#[cfg(target_arch = "wasm32")]
fn parse_optional_timeout(_py: &PyToken<'_>, timeout_bits: u64) -> Result<Option<f64>, u64> {
    let timeout_obj = obj_from_bits(timeout_bits);
    if timeout_obj.is_none() {
        return Ok(None);
    }
    let Some(timeout) = to_f64(timeout_obj) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "timeout value must be a float",
        ));
    };
    if !timeout.is_finite() {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "timeout value must be a non-negative number",
        ));
    }
    if timeout < 0.0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "timeout value must be a non-negative number",
        ));
    }
    Ok(Some(timeout))
}

#[cfg(target_arch = "wasm32")]
fn spin_acquire<F>(timeout: Option<f64>, mut try_acquire: F) -> bool
where
    F: FnMut() -> bool,
{
    match timeout {
        Some(val) if val == 0.0 => false,
        Some(val) => {
            let start = WasmInstant::now();
            loop {
                if try_acquire() {
                    return true;
                }
                if start.elapsed().as_secs_f64() >= val {
                    return false;
                }
                std::hint::spin_loop();
            }
        }
        None => loop {
            if try_acquire() {
                return true;
            }
            std::hint::spin_loop();
        },
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let lock = Rc::new(MoltLock::new());
        let raw = Rc::into_raw(lock) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_acquire(
    handle_bits: u64,
    blocking_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = lock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid lock handle");
        };
        let blocking = is_truthy(_py, obj_from_bits(blocking_bits));
        let timeout = match parse_timeout(_py, timeout_bits, blocking) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        if !blocking {
            return MoltObject::from_bool(lock.try_acquire()).bits();
        }
        if lock.try_acquire() {
            return MoltObject::from_bool(true).bits();
        }
        let acquired = spin_acquire(timeout, || lock.try_acquire());
        MoltObject::from_bool(acquired && lock.locked()).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_release(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = lock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid lock handle");
        };
        if !lock.release() {
            return raise_exception::<_>(_py, "RuntimeError", "release unlocked lock");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_locked(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = lock_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(lock.locked()).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_lock_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Rc::from_raw(ptr as *const MoltLock);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let lock = Rc::new(MoltRLock::new());
        let raw = Rc::into_raw(lock) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_acquire(
    handle_bits: u64,
    blocking_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid rlock handle");
        };
        let blocking = is_truthy(_py, obj_from_bits(blocking_bits));
        let timeout = match parse_timeout(_py, timeout_bits, blocking) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        if !blocking {
            return MoltObject::from_bool(lock.try_acquire()).bits();
        }
        if lock.try_acquire() {
            return MoltObject::from_bool(true).bits();
        }
        let acquired = spin_acquire(timeout, || lock.try_acquire());
        MoltObject::from_bool(acquired && lock.locked()).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_release(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid rlock handle");
        };
        if !lock.release() {
            return raise_exception::<_>(_py, "RuntimeError", "cannot release un-acquired lock");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_locked(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(lock.locked()).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_is_owned(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(lock.is_owned()).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_release_save(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid rlock handle");
        };
        match lock.release_save() {
            Some(saved) => MoltObject::from_int(saved as i64).bits(),
            None => raise_exception::<_>(_py, "RuntimeError", "cannot release un-acquired lock"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_acquire_restore(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(lock) = rlock_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid rlock handle");
        };
        let Some(count) = crate::to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "count must be an integer");
        };
        if count < 0 {
            return raise_exception::<_>(_py, "ValueError", "count must be >= 0");
        }
        lock.acquire_restore(count as u64);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_rlock_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Rc::from_raw(ptr as *const MoltRLock);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let condition = Rc::new(MoltCondition::new());
        let raw = Rc::into_raw(condition) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_wait(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(condition) = condition_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid condition handle");
        };
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let start = WasmInstant::now();
        let seq = condition.notify_seq.get();
        loop {
            if condition.notify_seq.get() != seq {
                return MoltObject::from_bool(true).bits();
            }
            if let Some(limit) = timeout {
                if start.elapsed().as_secs_f64() >= limit {
                    return MoltObject::from_bool(false).bits();
                }
            }
            std::hint::spin_loop();
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_wait_for(
    condition_bits: u64,
    predicate_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !is_truthy(_py, obj_from_bits(molt_is_callable(predicate_bits))) {
            return raise_exception::<_>(_py, "TypeError", "predicate must be callable");
        }
        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            let Some(value) = to_f64(obj_from_bits(timeout_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
            };
            Some(value)
        };
        let Some(wait_name_bits) = attr_name_bits_from_bytes(_py, b"wait") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let wait_bits = molt_getattr_builtin(condition_bits, wait_name_bits, missing);
        dec_ref_bits(_py, wait_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if wait_bits == missing {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "condition wait method is unavailable",
            );
        }
        if !is_truthy(_py, obj_from_bits(molt_is_callable(wait_bits))) {
            if obj_from_bits(wait_bits).as_ptr().is_some() {
                dec_ref_bits(_py, wait_bits);
            }
            return raise_exception::<_>(_py, "TypeError", "condition.wait must be callable");
        }
        let mut waittime = timeout;
        let mut deadline: Option<f64> = None;
        loop {
            let predicate_out = call_callable0(_py, predicate_bits);
            if exception_pending(_py) {
                if obj_from_bits(wait_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, wait_bits);
                }
                return MoltObject::none().bits();
            }
            let ok = is_truthy(_py, obj_from_bits(predicate_out));
            if obj_from_bits(predicate_out).as_ptr().is_some() {
                dec_ref_bits(_py, predicate_out);
            }
            if ok {
                if obj_from_bits(wait_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, wait_bits);
                }
                return MoltObject::from_bool(true).bits();
            }
            if let Some(current_wait) = waittime {
                if let Some(endtime) = deadline {
                    let remaining = endtime - monotonic_now_secs(_py);
                    waittime = Some(remaining);
                    if remaining <= 0.0 {
                        if obj_from_bits(wait_bits).as_ptr().is_some() {
                            dec_ref_bits(_py, wait_bits);
                        }
                        return MoltObject::from_bool(false).bits();
                    }
                } else {
                    deadline = Some(monotonic_now_secs(_py) + current_wait);
                }
            }
            let wait_arg = if let Some(current_wait) = waittime {
                MoltObject::from_float(current_wait).bits()
            } else {
                MoltObject::none().bits()
            };
            let wait_out = call_callable1(_py, wait_bits, wait_arg);
            if exception_pending(_py) {
                if obj_from_bits(wait_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, wait_bits);
                }
                return MoltObject::none().bits();
            }
            if obj_from_bits(wait_out).as_ptr().is_some() {
                dec_ref_bits(_py, wait_out);
            }
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_notify(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(condition) = condition_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid condition handle");
        };
        let Some(count) = crate::to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "notify count must be an integer");
        };
        if count > 0 {
            condition.notify(count as u64);
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_condition_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Rc::from_raw(ptr as *const MoltCondition);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_event_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let event = Rc::new(MoltEvent::new());
        let raw = Rc::into_raw(event) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_event_set(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(event) = event_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid event handle");
        };
        event.flag.set(true);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_event_clear(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(event) = event_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid event handle");
        };
        event.flag.set(false);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_event_is_set(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(event) = event_from_bits(handle_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(event.flag.get()).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_event_wait(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(event) = event_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid event handle");
        };
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let start = WasmInstant::now();
        loop {
            if event.flag.get() {
                return MoltObject::from_bool(true).bits();
            }
            if let Some(limit) = timeout {
                if start.elapsed().as_secs_f64() >= limit {
                    return MoltObject::from_bool(false).bits();
                }
            }
            std::hint::spin_loop();
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_event_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Rc::from_raw(ptr as *const MoltEvent);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_semaphore_new(value_bits: u64, bounded_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = crate::to_i64(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "semaphore initial value must be an integer",
            );
        };
        if value < 0 {
            return raise_exception::<_>(_py, "ValueError", "semaphore initial value must be >= 0");
        }
        let bounded = is_truthy(_py, obj_from_bits(bounded_bits));
        let sem = Rc::new(MoltSemaphore::new(value, bounded));
        let raw = Rc::into_raw(sem) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_semaphore_acquire(
    handle_bits: u64,
    blocking_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sem) = semaphore_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid semaphore handle");
        };
        let blocking = is_truthy(_py, obj_from_bits(blocking_bits));
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        if sem.try_acquire() {
            return MoltObject::from_bool(true).bits();
        }
        if !blocking {
            return MoltObject::from_bool(false).bits();
        }
        let start = WasmInstant::now();
        loop {
            if sem.try_acquire() {
                return MoltObject::from_bool(true).bits();
            }
            if let Some(limit) = timeout {
                if start.elapsed().as_secs_f64() >= limit {
                    return MoltObject::from_bool(false).bits();
                }
            }
            std::hint::spin_loop();
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_semaphore_release(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sem) = semaphore_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid semaphore handle");
        };
        let Some(count) = crate::to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "release count must be an integer");
        };
        if count < 1 {
            return raise_exception::<_>(_py, "ValueError", "semaphore release count must be >= 1");
        }
        if let Err(msg) = sem.release(count) {
            return raise_exception::<_>(_py, "ValueError", msg);
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_semaphore_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Rc::from_raw(ptr as *const MoltSemaphore);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_new(parties_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(parties) = crate::to_i64(obj_from_bits(parties_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "barrier parties must be an integer");
        };
        if parties <= 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "barrier parties must be greater than zero",
            );
        }
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let barrier = Rc::new(MoltBarrier::new(parties as u64, timeout));
        let raw = Rc::into_raw(barrier) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_wait(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        if barrier.broken.get() {
            return raise_exception::<_>(_py, "RuntimeError", "broken barrier");
        }
        let timeout = match parse_optional_timeout(_py, timeout_bits) {
            Ok(v) => v.or(barrier.default_timeout.get()),
            Err(bits) => return bits,
        };
        let generation = barrier.generation.get();
        let index = barrier.waiting.get();
        barrier.waiting.set(index.saturating_add(1));
        if barrier.waiting.get() == barrier.parties {
            barrier.waiting.set(0);
            barrier.generation.set(generation.saturating_add(1));
            return MoltObject::from_int(index as i64).bits();
        }
        let start = WasmInstant::now();
        loop {
            if barrier.broken.get() {
                return raise_exception::<_>(_py, "RuntimeError", "broken barrier");
            }
            if barrier.generation.get() != generation {
                return MoltObject::from_int(index as i64).bits();
            }
            if let Some(limit) = timeout {
                if start.elapsed().as_secs_f64() >= limit {
                    barrier.broken.set(true);
                    barrier.waiting.set(0);
                    barrier
                        .generation
                        .set(barrier.generation.get().saturating_add(1));
                    return raise_exception::<_>(_py, "RuntimeError", "broken barrier");
                }
            }
            std::hint::spin_loop();
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_abort(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        barrier.broken.set(true);
        barrier.waiting.set(0);
        barrier
            .generation
            .set(barrier.generation.get().saturating_add(1));
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_reset(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        barrier.broken.set(false);
        if barrier.waiting.get() > 0 {
            barrier.waiting.set(0);
            barrier
                .generation
                .set(barrier.generation.get().saturating_add(1));
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_parties(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        MoltObject::from_int(barrier.parties as i64).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_n_waiting(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        MoltObject::from_int(barrier.waiting.get() as i64).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_broken(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(barrier) = barrier_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid barrier handle");
        };
        MoltObject::from_bool(barrier.broken.get()).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_barrier_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = Rc::from_raw(ptr as *const MoltBarrier);
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_local_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let local = Rc::new(MoltLocal::new());
        let raw = Rc::into_raw(local) as *mut u8;
        bits_from_ptr(raw)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_local_get_dict(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(local) = local_from_bits(handle_bits) else {
            return raise_exception::<_>(_py, "TypeError", "invalid local handle");
        };
        let tid = 1u64;
        if let Some(bits) = local.storage.borrow().get(&tid).copied() {
            inc_ref_bits(_py, bits);
            return bits;
        }
        let dict_bits = crate::molt_dict_new(0);
        inc_ref_bits(_py, dict_bits);
        local.storage.borrow_mut().insert(tid, dict_bits);
        dict_bits
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_local_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let local = Rc::from_raw(ptr as *const MoltLocal);
        for bits in local.storage.borrow().values().copied() {
            dec_ref_bits(_py, bits);
        }
        local.storage.borrow_mut().clear();
        MoltObject::none().bits()
    })
}
