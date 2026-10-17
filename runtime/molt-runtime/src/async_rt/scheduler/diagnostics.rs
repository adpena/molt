use crate::PyToken;
use crate::runtime_flags::{self, RuntimeFlag};
use crate::{
    ASYNC_PENDING_COUNT, ASYNC_POLL_COUNT, header_from_obj_ptr, profile_hit, runtime_state,
};
use std::collections::HashMap;
use std::sync::Mutex;

#[inline]
pub(super) fn debug_current_task() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_CURRENT_TASK)
}

#[inline]
pub(crate) fn trace_task_result() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_TASK_RESULT)
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
    runtime_flags::enabled(RuntimeFlag::MOLT_ASYNC_TRACE)
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
