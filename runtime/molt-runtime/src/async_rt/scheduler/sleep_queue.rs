use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    ASYNC_SLEEP_REGISTER_COUNT, ASYNC_WAKEUP_COUNT, GilGuard, PtrSlot, PyToken, profile_hit,
    runtime_state,
};

use super::{async_trace_enabled, enqueue_task_ptr};

#[derive(Copy, Clone)]
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct SleepEntry {
    deadline: Instant,
    task_ptr: PtrSlot,
    generation: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl PartialEq for SleepEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
            && self.generation == other.generation
            && self.task_ptr == other.task_ptr
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Eq for SleepEntry {}

#[cfg(not(target_arch = "wasm32"))]
impl PartialOrd for SleepEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Ord for SleepEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.generation.cmp(&self.generation))
    }
}

pub(crate) struct SleepState {
    #[cfg(not(target_arch = "wasm32"))]
    heap: BinaryHeap<SleepEntry>,
    #[cfg(not(target_arch = "wasm32"))]
    tasks: HashMap<PtrSlot, u64>,
    #[cfg(not(target_arch = "wasm32"))]
    next_gen: u64,
    blocking: HashMap<PtrSlot, Instant>,
    shutdown: bool,
}

pub(crate) struct SleepQueue {
    inner: Mutex<SleepState>,
    #[cfg(not(target_arch = "wasm32"))]
    cv: Condvar,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl SleepQueue {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(SleepState {
                #[cfg(not(target_arch = "wasm32"))]
                heap: BinaryHeap::new(),
                #[cfg(not(target_arch = "wasm32"))]
                tasks: HashMap::new(),
                #[cfg(not(target_arch = "wasm32"))]
                next_gen: 0,
                blocking: HashMap::new(),
                shutdown: false,
            }),
            #[cfg(not(target_arch = "wasm32"))]
            cv: Condvar::new(),
            #[cfg(not(target_arch = "wasm32"))]
            worker: Mutex::new(None),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn set_worker_handle(&self, handle: thread::JoinHandle<()>) {
        let mut guard = self.worker.lock().unwrap();
        *guard = Some(handle);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn register_scheduler(
        &self,
        _py: &PyToken<'_>,
        task_ptr: *mut u8,
        deadline: Instant,
    ) {
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return;
        }
        if guard.tasks.contains_key(&PtrSlot(task_ptr)) {
            if async_trace_enabled() {
                eprintln!(
                    "molt async trace: sleep_register_skip task=0x{:x}",
                    task_ptr as usize
                );
            }
            return;
        }
        let generation = guard.next_gen;
        guard.next_gen += 1;
        guard.tasks.insert(PtrSlot(task_ptr), generation);
        profile_hit(_py, &ASYNC_SLEEP_REGISTER_COUNT);
        guard.heap.push(SleepEntry {
            deadline,
            task_ptr: PtrSlot(task_ptr),
            generation,
        });
        if async_trace_enabled() {
            let delay = deadline.saturating_duration_since(Instant::now());
            eprintln!(
                "molt async trace: sleep_register task=0x{:x} delay_ms={} gen={}",
                task_ptr as usize,
                delay.as_secs_f64() * 1000.0,
                generation
            );
        }
        self.cv.notify_one();
    }

    pub(crate) fn register_blocking(
        &self,
        _py: &PyToken<'_>,
        task_ptr: *mut u8,
        deadline: Instant,
    ) {
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return;
        }
        profile_hit(_py, &ASYNC_SLEEP_REGISTER_COUNT);
        guard.blocking.insert(PtrSlot(task_ptr), deadline);
        if async_trace_enabled() {
            let delay = deadline.saturating_duration_since(Instant::now());
            eprintln!(
                "molt async trace: sleep_register_blocking task=0x{:x} delay_ms={}",
                task_ptr as usize,
                delay.as_secs_f64() * 1000.0
            );
        }
    }

    pub(crate) fn cancel_task(&self, _py: &PyToken<'_>, task_ptr: *mut u8) {
        let _ = _py;
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return;
        }
        guard.blocking.remove(&PtrSlot(task_ptr));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let removed = guard.tasks.remove(&PtrSlot(task_ptr));
            if removed.is_some() && async_trace_enabled() {
                eprintln!(
                    "molt async trace: sleep_cancel task=0x{:x}",
                    task_ptr as usize
                );
            }
            self.cv.notify_one();
        }
    }

    pub(crate) fn take_blocking_deadline(
        &self,
        _py: &PyToken<'_>,
        task_ptr: *mut u8,
    ) -> Option<Instant> {
        let _ = _py;
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return None;
        }
        guard.blocking.remove(&PtrSlot(task_ptr))
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn next_scheduler_deadline(&self) -> Option<Instant> {
        let _ = self;
        None
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn next_scheduler_deadline(&self) -> Option<Instant> {
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return None;
        }
        loop {
            let entry = guard.heap.peek()?;
            let key = entry.task_ptr;
            if guard.tasks.get(&key) != Some(&entry.generation) {
                guard.heap.pop();
                continue;
            }
            return Some(entry.deadline);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn take_due_scheduler_tasks(&self) -> Vec<*mut u8> {
        let mut guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return Vec::new();
        }
        let now = Instant::now();
        let mut due: Vec<*mut u8> = Vec::new();
        while let Some(entry) = guard.heap.peek() {
            let key = entry.task_ptr;
            if guard.tasks.get(&key) != Some(&entry.generation) {
                guard.heap.pop();
                continue;
            }
            if entry.deadline > now {
                break;
            }
            let entry = guard.heap.pop().expect("heap entry disappeared");
            guard.tasks.remove(&key);
            due.push(entry.task_ptr.0);
        }
        due
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn is_scheduled(&self, _py: &PyToken<'_>, task_ptr: *mut u8) -> bool {
        let _ = _py;
        let guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return false;
        }
        guard.tasks.contains_key(&PtrSlot(task_ptr))
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn is_scheduled(&self, _py: &PyToken<'_>, _task_ptr: *mut u8) -> bool {
        let _ = _py;
        false
    }

    pub(crate) fn shutdown(&self, _py: &PyToken<'_>) {
        let _ = _py;
        {
            let mut guard = self.inner.lock().unwrap();
            guard.shutdown = true;
            guard.blocking.clear();
            #[cfg(not(target_arch = "wasm32"))]
            {
                guard.tasks.clear();
                guard.heap.clear();
                self.cv.notify_all();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(handle) = self.worker.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sleep_worker(queue: Arc<SleepQueue>) {
    if async_trace_enabled() {
        eprintln!("molt async trace: sleep_worker_start");
    }
    loop {
        let task_ptr = {
            let mut guard = queue.inner.lock().unwrap();
            loop {
                if guard.shutdown {
                    return;
                }
                match guard.heap.peek() {
                    Some(entry) => {
                        let key = entry.task_ptr;
                        if guard.tasks.get(&key) != Some(&entry.generation) {
                            guard.heap.pop();
                            continue;
                        }
                        let now = Instant::now();
                        if entry.deadline <= now {
                            let entry = guard.heap.pop().unwrap();
                            guard.tasks.remove(&key);
                            break entry.task_ptr.0;
                        }
                        let wait = entry.deadline.saturating_duration_since(now);
                        let (next_guard, _) = queue.cv.wait_timeout(guard, wait).unwrap();
                        guard = next_guard;
                    }
                    None => {
                        guard = queue.cv.wait(guard).unwrap();
                    }
                }
            }
        };
        let gil = GilGuard::new();
        let py = gil.token();
        profile_hit(&py, &ASYNC_WAKEUP_COUNT);
        if async_trace_enabled() {
            eprintln!(
                "molt async trace: sleep_wakeup task=0x{:x}",
                task_ptr as usize
            );
        }
        enqueue_task_ptr(&py, task_ptr);
    }
}

pub(crate) fn monotonic_now_secs(_py: &PyToken<'_>) -> f64 {
    let nanos = runtime_state(_py)
        .start_time
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .max(1);
    nanos as f64 / 1_000_000_000.0
}

pub(crate) fn monotonic_now_nanos(_py: &PyToken<'_>) -> u128 {
    runtime_state(_py)
        .start_time
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .max(1)
}

pub(crate) fn instant_from_monotonic_secs(_py: &PyToken<'_>, secs: f64) -> Instant {
    let start = runtime_state(_py).start_time.get_or_init(Instant::now);
    if !secs.is_finite() || secs <= 0.0 {
        return *start;
    }
    *start + Duration::from_secs_f64(secs)
}
