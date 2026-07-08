use crate::PyToken;
#[cfg(not(target_arch = "wasm32"))]
use crate::{PtrSlot, obj_from_bits, ptr_from_bits, runtime_state, to_i64};
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use crate::{IoPoller, ProcessTaskState, ThreadTaskState};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use super::{process_task_state, task_waiting_on, thread_task_state};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) enum BlockOnWaitSpec {
    Io {
        poller: Arc<IoPoller>,
        socket_ptr: *mut u8,
        events: u32,
        timeout: Option<Duration>,
    },
    Thread {
        state: Arc<ThreadTaskState>,
        timeout: Option<Duration>,
    },
    Process {
        state: Arc<ProcessTaskState>,
        timeout: Option<Duration>,
    },
}

#[cfg(target_arch = "wasm32")]
pub(crate) enum BlockOnWaitSpec {}

pub(crate) const BLOCK_ON_MIN_SLEEP: Duration = Duration::from_micros(50);
pub(crate) const BLOCK_ON_MAX_WAIT: Duration = Duration::from_millis(5);

pub(crate) fn block_on_poll_timeout(timeout: Option<Duration>) -> Duration {
    match timeout {
        Some(val) => val.min(BLOCK_ON_MAX_WAIT),
        None => BLOCK_ON_MAX_WAIT,
    }
}

pub(crate) fn block_on_wait_spec(
    _py: &PyToken<'_>,
    awaited_ptr: *mut u8,
    deadline: Option<Instant>,
) -> Option<BlockOnWaitSpec> {
    if awaited_ptr.is_null() {
        return None;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = deadline;
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        #[inline]
        fn remaining_timeout(deadline: Option<Instant>) -> Option<Duration> {
            deadline.and_then(|dl| {
                let now = Instant::now();
                if dl > now { Some(dl - now) } else { None }
            })
        }
        #[inline]
        unsafe fn wait_spec_for_ptr(
            _py: &PyToken<'_>,
            cursor: *mut u8,
            timeout: Option<Duration>,
        ) -> Option<BlockOnWaitSpec> {
            unsafe {
                let _header = crate::header_from_obj_ptr(cursor);
                let poll_fn = crate::object::object_poll_fn(cursor);
                if poll_fn == crate::io_wait_poll_fn_addr() {
                    let payload_bytes = crate::object::object_payload_size(cursor);
                    if payload_bytes < 2 * std::mem::size_of::<u64>() {
                        return None;
                    }
                    let payload_ptr = cursor as *mut u64;
                    let socket_bits = *payload_ptr;
                    let events_bits = *payload_ptr.add(1);
                    let socket_ptr = ptr_from_bits(socket_bits);
                    if socket_ptr.is_null() {
                        return None;
                    }
                    let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
                    if events == 0 {
                        return None;
                    }
                    let poller = Arc::clone(runtime_state(_py).io_poller());
                    return Some(BlockOnWaitSpec::Io {
                        poller,
                        socket_ptr,
                        events,
                        timeout,
                    });
                }
                if poll_fn == crate::thread_poll_fn_addr()
                    && let Some(state) = thread_task_state(_py, cursor)
                {
                    return Some(BlockOnWaitSpec::Thread { state, timeout });
                }
                if poll_fn == crate::process_poll_fn_addr()
                    && let Some(state) = process_task_state(_py, cursor)
                {
                    return Some(BlockOnWaitSpec::Process { state, timeout });
                }
                None
            }
        }
        unsafe {
            let mut cursor = awaited_ptr;
            for _ in 0..8 {
                let timeout = remaining_timeout(deadline);
                if let Some(spec) = wait_spec_for_ptr(_py, cursor, timeout) {
                    return Some(spec);
                }
                let next = {
                    let waiting_map = task_waiting_on(_py).lock().unwrap();
                    waiting_map.get(&PtrSlot(cursor)).map(|val| val.0)
                };
                let Some(next_ptr) = next else {
                    break;
                };
                if next_ptr.is_null() || next_ptr == cursor {
                    break;
                }
                cursor = next_ptr;
            }
        }
        None
    }
}
