use crate::{MoltObject, PyToken};

#[cfg(not(target_arch = "wasm32"))]
use super::current_thread_id;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use crate::{
    bits_from_ptr, is_truthy, obj_from_bits, ptr_from_bits, raise_exception, release_ptr, to_f64,
    GilReleaseGuard,
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
        let acquired = if !blocking {
            lock.try_acquire()
        } else if matches!(timeout, Some(t) if t == Duration::ZERO) {
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
        let acquired = if !blocking {
            lock.try_acquire()
        } else if matches!(timeout, Some(t) if t == Duration::ZERO) {
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

#[cfg(target_arch = "wasm32")]
use std::cell::Cell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use std::time::Instant as WasmInstant;

#[cfg(target_arch = "wasm32")]
use crate::{
    bits_from_ptr, is_truthy, obj_from_bits, ptr_from_bits, raise_exception, release_ptr, to_f64,
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
struct MoltRLock {
    locked: Cell<bool>,
    owner: Cell<u64>,
    count: Cell<u64>,
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
