use crate::abi_types::Py_tss_t;
use parking_lot::{Condvar, Mutex};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicUsize, Ordering};

struct AbiLock {
    held: Mutex<bool>,
    available: Condvar,
}

static NEXT_TSS_KEY: AtomicUsize = AtomicUsize::new(1);
thread_local! {
    static TSS_VALUES: RefCell<HashMap<usize, *mut c_void>> = RefCell::new(HashMap::new());
}

#[unsafe(no_mangle)]
pub extern "C" fn PyThread_allocate_lock() -> *mut c_void {
    Box::into_raw(Box::new(AbiLock {
        held: Mutex::new(false),
        available: Condvar::new(),
    }))
    .cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_free_lock(lock: *mut c_void) {
    if !lock.is_null() {
        drop(unsafe { Box::from_raw(lock.cast::<AbiLock>()) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_acquire_lock(lock: *mut c_void, waitflag: c_int) -> c_int {
    if lock.is_null() {
        return 0;
    }
    let lock = unsafe { &*lock.cast::<AbiLock>() };
    let mut held = lock.held.lock();
    if waitflag == 0 {
        if *held {
            return 0;
        }
    } else {
        while *held {
            lock.available.wait(&mut held);
        }
    }
    *held = true;
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_release_lock(lock: *mut c_void) {
    if lock.is_null() {
        return;
    }
    let lock = unsafe { &*lock.cast::<AbiLock>() };
    *lock.held.lock() = false;
    lock.available.notify_one();
}

#[unsafe(no_mangle)]
pub extern "C" fn PyThread_tss_alloc() -> *mut Py_tss_t {
    Box::into_raw(Box::new(Py_tss_t {
        _is_initialized: 0,
        _key: 0,
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_tss_free(key: *mut Py_tss_t) {
    if !key.is_null() {
        unsafe { PyThread_tss_delete(key) };
        drop(unsafe { Box::from_raw(key) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_tss_create(key: *mut Py_tss_t) -> c_int {
    if key.is_null() {
        return -1;
    }
    if unsafe { (*key)._is_initialized } == 0 {
        unsafe {
            (*key)._key = NEXT_TSS_KEY.fetch_add(1, Ordering::Relaxed);
            (*key)._is_initialized = 1;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_tss_delete(key: *mut Py_tss_t) {
    if key.is_null() {
        return;
    }
    let id = unsafe { (*key)._key };
    TSS_VALUES.with(|values| {
        values.borrow_mut().remove(&id);
    });
    unsafe {
        (*key)._is_initialized = 0;
        (*key)._key = 0;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_tss_is_created(key: *mut Py_tss_t) -> c_int {
    if key.is_null() {
        0
    } else {
        unsafe { (*key)._is_initialized }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_tss_set(key: *mut Py_tss_t, value: *mut c_void) -> c_int {
    if key.is_null() || unsafe { (*key)._is_initialized } == 0 {
        return -1;
    }
    let id = unsafe { (*key)._key };
    TSS_VALUES.with(|values| {
        values.borrow_mut().insert(id, value);
    });
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThread_tss_get(key: *mut Py_tss_t) -> *mut c_void {
    if key.is_null() || unsafe { (*key)._is_initialized } == 0 {
        return std::ptr::null_mut();
    }
    let id = unsafe { (*key)._key };
    TSS_VALUES.with(|values| {
        values
            .borrow()
            .get(&id)
            .copied()
            .unwrap_or(std::ptr::null_mut())
    })
}
