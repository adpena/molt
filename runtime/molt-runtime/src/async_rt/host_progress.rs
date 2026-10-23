//! Host progress for WASM's poll-driven host ABIs.
//!
//! Process, stream and socket host imports have no multiplexed completion
//! wait, so their retries need an explicit deadline. These primitives serve
//! every such operation; networking (the I/O poller) is one consumer, so they
//! live outside it and exist with or without a networking provider.

use crate::{PyToken, header_from_obj_ptr, runtime_state};
use std::time::{Duration, Instant};

/// Host ABIs without a multiplexed completion wait need an explicit retry
/// deadline. Socket/WebSocket registrations bound a host wait by this interval;
/// process and stream retries put the same interval into the owning task's
/// existing timer queue. An idle loop with no such work has no polling cap.
pub(crate) const HOST_PROGRESS_POLL_SLICE: Duration = Duration::from_millis(5);

/// With no deadline and no host readiness waiter, no guest code can become
/// ready; chunking only keeps each WASI clock wait finite.
const UNBOUNDED_IDLE_CHUNK: Duration = Duration::from_secs(3600);

/// Give a scheduled task a concrete retry deadline for a poll-driven host
/// operation. Inline futures are subscribed through their scheduled awaiter by
/// `sleep_register_impl`; they must not create an unowned scheduler timer.
pub(crate) fn register_host_progress_retry(py: &PyToken<'_>, task: *mut u8) -> bool {
    if task.is_null() {
        return false;
    }
    let flags = unsafe { (*header_from_obj_ptr(task)).load_synchronized_flags() };
    if flags & (crate::HEADER_FLAG_BLOCK_ON | crate::HEADER_FLAG_SPAWN_RETAIN) == 0 {
        return false;
    }
    let deadline = Instant::now() + HOST_PROGRESS_POLL_SLICE;
    if flags & crate::HEADER_FLAG_BLOCK_ON != 0 {
        runtime_state(py)
            .sleep_queue()
            .register_blocking(py, task, deadline);
    } else {
        crate::async_rt::scheduler::register_task_sleep(py, task, deadline);
    }
    true
}

/// How long the only guest thread parks: until `timeout`, or for one
/// host-progress slice while host readiness waiters exist.
pub(crate) fn idle_wait_duration(host_waiters: bool, timeout: Option<Duration>) -> Duration {
    match (host_waiters, timeout) {
        (false, Some(timeout)) => timeout,
        (false, None) => UNBOUNDED_IDLE_CHUNK,
        (true, Some(timeout)) => timeout.min(HOST_PROGRESS_POLL_SLICE),
        (true, None) => HOST_PROGRESS_POLL_SLICE,
    }
}
