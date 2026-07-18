use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(molt_has_net_io)]
use mio::net::TcpStream as MioTcpStream;
#[cfg(molt_has_net_io)]
use mio::{Events, Interest, Poll, Registry, Token, Waker};
#[cfg(all(molt_has_net_io, unix))]
use std::os::unix::io::AsRawFd;
#[cfg(all(molt_has_net_io, windows))]
use std::os::windows::io::AsRawSocket;

#[cfg(molt_has_net_io)]
use super::sockets::{socket_ptr_from_bits_or_fd, socket_ref_inc, with_socket_mut};
use super::wake_await_waiters;
use crate::require_net_capability;
use crate::{
    GilGuard, GilReleaseGuard, MoltObject, PtrSlot, PyToken, dec_ref_bits, header_from_obj_ptr,
    inc_ref_bits, io_wait_poll_fn_addr, molt_future_new, monotonic_now_secs, obj_from_bits,
    pending_bits_i64, ptr_from_bits, raise_exception, resolve_obj_ptr, runtime_state, to_f64,
    to_i64,
};
#[cfg(target_arch = "wasm32")]
use crate::{IO_EVENT_ERROR, IO_EVENT_READ, IO_EVENT_WRITE};
#[cfg(molt_has_net_io)]
use crate::{IO_EVENT_ERROR, IO_EVENT_READ, IO_EVENT_WRITE, raise_os_error};
fn trace_io_poller() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_IO_POLLER").as_deref() == Ok("1"))
}

fn trace_io_wait_errors() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_IO_WAIT_ERRORS").as_deref() == Ok("1"))
}
#[cfg(molt_has_net_io)]
mod blocking;
#[cfg(molt_has_net_io)]
mod native;
mod wait_api;
#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(molt_has_net_io)]
mod worker;

#[cfg(molt_has_net_io)]
use blocking::{BlockingWaiter, BlockingWaiterList};
#[cfg(molt_has_net_io)]
pub(crate) use native::IoPoller;
#[cfg(molt_has_net_io)]
use native::socket_debug_fd;
pub use wait_api::*;
#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::IoPoller;
#[cfg(molt_has_net_io)]
use worker::io_worker;
