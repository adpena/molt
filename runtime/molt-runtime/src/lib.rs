//! Molt Runtime Core
//! Handles memory management, task scheduling, channels, and FFI boundaries.

use molt_obj_model::MoltObject;
use crossbeam_channel::{unbounded, Receiver, Sender};
use crossbeam_deque::{Injector, Stealer, Worker};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::collections::HashMap;

#[repr(C)]
pub struct MoltHeader {
    pub type_id: u32,
    pub ref_count: u32,
    pub poll_fn: u64, // Function pointer for polling
    pub state: i64,   // State machine state
    pub size: usize,  // Total size of allocation
}

#[no_mangle]
pub extern "C" fn molt_alloc(size: usize) -> *mut u8 {
    let total_size = size + std::mem::size_of::<MoltHeader>();
    let layout = std::alloc::Layout::from_size_align(total_size, 8).unwrap();
    unsafe {
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() { return std::ptr::null_mut(); }
        let header = ptr as *mut MoltHeader;
        (*header).type_id = 100; 
        (*header).ref_count = 1;
        (*header).poll_fn = 0;
        (*header).state = 0;
        (*header).size = total_size;
        ptr.add(std::mem::size_of::<MoltHeader>())
    }
}

// --- Channels ---

pub struct MoltChannel {
    pub sender: Sender<i64>,
    pub receiver: Receiver<i64>,
}

#[no_mangle]
pub extern "C" fn molt_chan_new() -> *mut u8 {
    let (s, r) = unbounded();
    let chan = Box::new(MoltChannel { sender: s, receiver: r });
    Box::into_raw(chan) as *mut u8
}

#[no_mangle]
pub unsafe extern "C" fn molt_chan_send(chan_ptr: *mut u8, val: i64) -> i64 {
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.sender.try_send(val) {
        Ok(_) => 0, // Ready(None)
        Err(_) => std::mem::transmute::<u64, i64>(0x7ffc_0000_0000_0000), // PENDING
    }
}

#[no_mangle]
pub unsafe extern "C" fn molt_chan_recv(chan_ptr: *mut u8) -> i64 {
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(_) => std::mem::transmute::<u64, i64>(0x7ffc_0000_0000_0000), // PENDING
    }
}

// --- Scheduler ---

pub struct MoltTask {
    pub future_ptr: *mut u8,
}

unsafe impl Send for MoltTask {}

pub struct MoltScheduler {
    injector: Arc<Injector<MoltTask>>,
    stealers: Vec<Stealer<MoltTask>>,
    running: Arc<AtomicBool>,
}

impl MoltScheduler {
    pub fn new() -> Self {
        let num_threads = num_cpus::get();
        let injector = Arc::new(Injector::new());
        let mut workers = Vec::new();
        let mut stealers = Vec::new();
        let running = Arc::new(AtomicBool::new(true));

        for _ in 0..num_threads {
            workers.push(Worker::new_fifo());
        }

        for w in &workers {
            stealers.push(w.stealer());
        }

        for (i, worker) in workers.into_iter().enumerate() {
            let injector_clone = Arc::clone(&injector);
            let stealers_clone = stealers.clone();
            let running_clone = Arc::clone(&running);

            thread::spawn(move || {
                loop {
                    if !running_clone.load(Ordering::Relaxed) { break; }

                    if let Some(task) = worker.pop() {
                        Self::execute_task(task, &injector_clone);
                        continue;
                    }

                    match injector_clone.steal_batch_and_pop(&worker) {
                        crossbeam_deque::Steal::Success(task) => {
                            Self::execute_task(task, &injector_clone);
                            continue;
                        }
                        crossbeam_deque::Steal::Retry => continue,
                        crossbeam_deque::Steal::Empty => {}
                    }

                    let mut stolen = false;
                    for (j, stealer) in stealers_clone.iter().enumerate() {
                        if i == j { continue; }
                        if let crossbeam_deque::Steal::Success(task) = stealer.steal_batch_and_pop(&worker) {
                            Self::execute_task(task, &injector_clone);
                            stolen = true;
                            break;
                        }
                    }

                    if !stolen {
                        thread::yield_now();
                    }
                }
            });
        }

        Self {
            injector,
            stealers,
            running,
        }
    }

    fn execute_task(task: MoltTask, injector: &Injector<MoltTask>) {
        unsafe {
            let task_ptr = task.future_ptr;
            let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
            let poll_fn_addr = (*header).poll_fn;
            if poll_fn_addr != 0 {
                let poll_fn: extern "C" fn(*mut u8) -> i64 = std::mem::transmute(poll_fn_addr as usize);
                let res = poll_fn(task_ptr);
                if res == std::mem::transmute::<u64, i64>(0x7ffc_0000_0000_0000) {
                    injector.push(task);
                }
            }
        }
    }
}

lazy_static::lazy_static! {
    static ref SCHEDULER: MoltScheduler = MoltScheduler::new();
}

#[no_mangle]
pub unsafe extern "C" fn molt_spawn(task_ptr: *mut u8) {
    SCHEDULER.injector.push(MoltTask { future_ptr: task_ptr });
}

#[no_mangle]
pub unsafe extern "C" fn molt_block_on(task_ptr: *mut u8) -> i64 {
    let header = task_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    let poll_fn_addr = (*header).poll_fn;
    if poll_fn_addr == 0 { return 0; }
    let poll_fn: extern "C" fn(*mut u8) -> i64 = std::mem::transmute(poll_fn_addr as usize);
    loop {
        let res = poll_fn(task_ptr);
        if res == unsafe { std::mem::transmute::<u64, i64>(0x7ffc_0000_0000_0000) } {
             std::thread::yield_now();
             continue;
        }
        return res;
    }
}

#[no_mangle]
pub unsafe extern "C" fn molt_async_sleep(_obj_ptr: *mut u8) -> i64 {
    static mut CALLED: bool = false;
    if !CALLED {
        CALLED = true;
        return std::mem::transmute::<u64, i64>(0x7ffc_0000_0000_0000);
    } else {
        CALLED = false; 
        return 0;
    }
}

// --- Reference Counting ---

/// # Safety
/// Dereferences raw pointer to increment ref count.
#[no_mangle]
pub unsafe extern "C" fn molt_inc_ref(ptr: *mut u8) {
    if ptr.is_null() { return; }
    let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    (*header_ptr).ref_count += 1;
}

/// # Safety
/// Dereferences raw pointer to decrement ref count. Frees memory if count reaches 0.
#[no_mangle]
pub unsafe extern "C" fn molt_dec_ref(ptr: *mut u8) {
    if ptr.is_null() { return; }
    let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader;
    let header = &mut *header_ptr;
    header.ref_count -= 1;
    if header.ref_count == 0 {
        let size = header.size;
        let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
        std::alloc::dealloc(header_ptr as *mut u8, layout);
    }
}

// --- JSON ---

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_json_parse_int(ptr: *const u8, len: usize) -> i64 {
    let s = {
        let slice = std::slice::from_raw_parts(ptr, len);
        std::str::from_utf8(slice).unwrap()
    };
    let v: serde_json::Value = serde_json::from_str(s).unwrap();
    v.as_i64().unwrap_or(0)
}

// --- Generic ---

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn molt_get_attr_generic(_obj_ptr: *mut u8, attr_name_ptr: *const u8, attr_name_len: usize) -> i64 {
    let _s = {
        let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
        std::str::from_utf8(slice).unwrap()
    };
    0 
}