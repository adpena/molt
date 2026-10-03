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
pub(crate) struct SleepEntry {
    deadline: Instant,
    task_ptr: PtrSlot,
    generation: u64,
}

impl PartialEq for SleepEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
            && self.generation == other.generation
            && self.task_ptr == other.task_ptr
    }
}

impl Eq for SleepEntry {}

impl PartialOrd for SleepEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SleepEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.generation.cmp(&self.generation))
    }
}

pub(crate) struct SleepState {
    heap: BinaryHeap<SleepEntry>,
    tasks: HashMap<PtrSlot, u64>,
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
                heap: BinaryHeap::new(),
                tasks: HashMap::new(),
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
        #[cfg(not(target_arch = "wasm32"))]
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
        {
            let removed = guard.tasks.remove(&PtrSlot(task_ptr));
            if removed.is_some() && async_trace_enabled() {
                eprintln!(
                    "molt async trace: sleep_cancel task=0x{:x}",
                    task_ptr as usize
                );
            }
            #[cfg(not(target_arch = "wasm32"))]
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

    pub(crate) fn take_due_scheduler_tasks(&self, _py: &PyToken<'_>) -> Vec<*mut u8> {
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

    pub(crate) fn is_scheduled(&self, _py: &PyToken<'_>, task_ptr: *mut u8) -> bool {
        let _ = _py;
        let guard = self.inner.lock().unwrap();
        if guard.shutdown {
            return false;
        }
        guard.tasks.contains_key(&PtrSlot(task_ptr))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn wait_until_scheduler_due(&self) -> bool {
        let mut guard = self.inner.lock().unwrap();
        loop {
            if guard.shutdown {
                return false;
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
                        // Do not detach a raw task pointer before owning the GIL.
                        // Cancellation/GC may retire it while we wait for the GIL.
                        return true;
                    }
                    let wait = entry.deadline.saturating_duration_since(now);
                    let (next_guard, _) = self.cv.wait_timeout(guard, wait).unwrap();
                    guard = next_guard;
                }
                None => {
                    guard = self.cv.wait(guard).unwrap();
                }
            }
        }
    }

    pub(crate) fn shutdown(&self, _py: &PyToken<'_>) {
        let _ = _py;
        {
            let mut guard = self.inner.lock().unwrap();
            guard.shutdown = true;
            guard.blocking.clear();
            {
                guard.tasks.clear();
                guard.heap.clear();
                #[cfg(not(target_arch = "wasm32"))]
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
        if !queue.wait_until_scheduler_due() {
            return;
        }
        let gil = GilGuard::new();
        let py = gil.token();
        for task_ptr in queue.take_due_scheduler_tasks(&py) {
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod custody_tests {
    use super::*;

    #[test]
    fn cancellation_after_deadline_observation_prevents_raw_pointer_claim() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let queue = Arc::new(SleepQueue::new());
            let task = crate::molt_future_new(0, 0);
            let task_ptr = crate::ptr_from_bits(task);
            queue.register_scheduler(py, task_ptr, Instant::now());
            let worker_queue = Arc::clone(&queue);
            let (observed_tx, observed_rx) = std::sync::mpsc::channel();
            let (resume_tx, resume_rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                assert!(worker_queue.wait_until_scheduler_due());
                observed_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                let gil = GilGuard::new();
                let token = gil.token();
                worker_queue.take_due_scheduler_tasks(&token).len()
            });
            observed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            // Observing an expired deadline is not custody of its task pointer.
            assert!(queue.is_scheduled(py, task_ptr));
            queue.cancel_task(py, task_ptr);
            crate::dec_ref_bits(py, task);
            resume_tx.send(()).unwrap();
            let claimed = {
                let _released = crate::GilReleaseGuard::suspend();
                worker.join().unwrap()
            };
            assert_eq!(claimed, 0);
            queue.shutdown(py);
        });
    }
}
