use crate::PyToken;
use crate::{
    ASYNC_PENDING_COUNT, ASYNC_POLL_COUNT, header_from_obj_ptr, profile_hit, runtime_state,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[inline]
pub(super) fn debug_current_task() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_CURRENT_TASK").as_deref() == Ok("1"))
}

#[inline]
pub(crate) fn trace_task_result() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_TASK_RESULT").as_deref() == Ok("1"))
}

pub(crate) struct AsyncHangProbe {
    threshold: usize,
    pub(crate) pending_counts: Mutex<HashMap<usize, usize>>,
}

impl AsyncHangProbe {
    fn new(threshold: usize) -> Self {
        Self {
            threshold,
            pending_counts: Mutex::new(HashMap::new()),
        }
    }
}

pub(crate) fn async_hang_probe(_py: &PyToken<'_>) -> Option<&'static AsyncHangProbe> {
    runtime_state(_py)
        .async_hang_probe
        .get_or_init(|| {
            let value = std::env::var("MOLT_ASYNC_HANG_PROBE").ok()?;
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return None;
            }
            let threshold = match trimmed.parse::<usize>() {
                Ok(0) => return None,
                Ok(val) => val,
                Err(_) => 100_000,
            };
            Some(AsyncHangProbe::new(threshold))
        })
        .as_ref()
}

pub(crate) fn async_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        let value = std::env::var("MOLT_ASYNC_TRACE").unwrap_or_default();
        let trimmed = value.trim().to_ascii_lowercase();
        !trimmed.is_empty() && trimmed != "0" && trimmed != "false"
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn async_worker_threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| {
        let max_threads = num_cpus::get().max(1);
        let parsed = std::env::var("MOLT_ASYNC_THREADS")
            .ok()
            .and_then(|val| val.trim().parse::<usize>().ok());
        parsed.unwrap_or(0).min(max_threads)
    })
}

pub(crate) fn record_async_poll(_py: &PyToken<'_>, task_ptr: *mut u8, pending: bool, site: &str) {
    profile_hit(_py, &ASYNC_POLL_COUNT);
    if pending {
        profile_hit(_py, &ASYNC_PENDING_COUNT);
    }
    let Some(probe) = async_hang_probe(_py) else {
        return;
    };
    if task_ptr.is_null() {
        return;
    }
    if !pending {
        probe
            .pending_counts
            .lock()
            .unwrap()
            .remove(&(task_ptr as usize));
        return;
    }
    let mut counts = probe.pending_counts.lock().unwrap();
    let count = counts.entry(task_ptr as usize).or_insert(0);
    *count += 1;
    if *count != probe.threshold && *count % probe.threshold != 0 {
        return;
    }
    unsafe {
        let header = header_from_obj_ptr(task_ptr);
        eprintln!(
            "Molt async hang probe: site={} polls={} ptr=0x{:x} type={} state={} poll=0x{:x}",
            site,
            count,
            task_ptr as usize,
            (*header).type_id,
            crate::object::object_state(task_ptr),
            crate::object::object_poll_fn(task_ptr)
        );
    }
}
