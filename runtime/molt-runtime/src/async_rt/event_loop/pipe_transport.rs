//! Asyncio pipe transport authority.
//!
//! Owns fd-backed pipe transport state, registry, native/wasm pipe intrinsics,
//! and connect_read_pipe/connect_write_pipe transport construction. Event-loop
//! callback tables stay in the parent event_loop module behind narrow helpers.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use super::*;
use crate::{MoltObject, raise_exception, runtime_state};
#[cfg(windows)]
#[inline]
fn libc_write_count(len: usize) -> libc::c_uint {
    len.min(u32::MAX as usize) as libc::c_uint
}

#[cfg(not(windows))]
#[inline]
fn libc_write_count(len: usize) -> usize {
    len
}

// ============================================================================
// Pipe Transport — fd-based read/write transports for asyncio.connect_read_pipe
// and asyncio.connect_write_pipe.
//
// Architecture:
// - PipeTransportState: per-handle state for a pipe transport (fd, direction,
//   closing/paused flags, write buffer).
// - Handle registry: runtime-owned Mutex<HashMap<i64, PipeTransportState>> with
//   atomic counter for handle allocation (same pattern as event loop handles).
// - Native targets: full fd-based I/O via libc read/write.
// - WASM targets: all pipe transport operations return error sentinels since
//   WASM does not support file descriptors in the traditional sense.
// ============================================================================

/// Internal state for a single pipe transport instance.
struct PipeTransportState {
    /// The underlying file descriptor.
    fd: i32,
    /// True for read pipes, false for write pipes.
    is_read: bool,
    /// Whether close() has been called.
    closing: bool,
    /// Whether reading is paused (read pipes only).
    paused: bool,
    /// Buffered writes pending flush (write pipes only).
    write_buffer: VecDeque<Vec<u8>>,
}

impl PipeTransportState {
    fn new(fd: i32, is_read: bool) -> Self {
        Self {
            fd,
            is_read,
            closing: false,
            paused: false,
            write_buffer: VecDeque::new(),
        }
    }
}

/// Runtime-owned pipe transport handle registry.
pub(crate) struct PipeTransportRegistry {
    transports: Mutex<HashMap<i64, PipeTransportState>>,
    #[cfg(not(target_arch = "wasm32"))]
    next_handle: AtomicI64,
}

impl PipeTransportRegistry {
    pub(crate) fn new() -> Self {
        Self {
            transports: Mutex::new(HashMap::new()),
            #[cfg(not(target_arch = "wasm32"))]
            next_handle: AtomicI64::new(1),
        }
    }

    pub(crate) fn clear(&self) {
        let transports = {
            let mut guard = self.transports.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            for (_, mut state) in transports {
                close_pipe_transport_state(&mut state);
            }
            self.next_handle.store(1, Ordering::Relaxed);
        }
        #[cfg(target_arch = "wasm32")]
        drop(transports);
    }
}

#[inline]
fn pipe_transport_registry(_py: &crate::PyToken<'_>) -> &'static PipeTransportRegistry {
    &runtime_state(_py).pipe_transport_registry
}

#[cfg(not(target_arch = "wasm32"))]
fn alloc_pipe_transport(_py: &crate::PyToken<'_>, fd: i32, is_read: bool) -> i64 {
    let registry = pipe_transport_registry(_py);
    let handle = registry.next_handle.fetch_add(1, Ordering::Relaxed);
    registry
        .transports
        .lock()
        .unwrap()
        .insert(handle, PipeTransportState::new(fd, is_read));
    handle
}

fn with_pipe<F, R>(_py: &crate::PyToken<'_>, handle: i64, f: F) -> Option<R>
where
    F: FnOnce(&mut PipeTransportState) -> R,
{
    let mut map = pipe_transport_registry(_py).transports.lock().unwrap();
    map.get_mut(&handle).map(f)
}

/// Extract a bytes-like slice from NaN-boxed bits.
/// Returns Ok(slice) or Err(exception sentinel bits).
#[cfg(not(target_arch = "wasm32"))]
fn pipe_require_bytes_slice(_py: &crate::PyToken<'_>, bits: u64) -> Result<&'static [u8], u64> {
    let obj = crate::obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "a bytes-like object is required",
        ));
    };
    unsafe {
        if let Some(slice) = crate::object::memoryview::bytes_like_slice(ptr) {
            return Ok(slice);
        }
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "a bytes-like object is required",
    ))
}

// --- Pipe transport intrinsics ---

/// Create a new pipe transport wrapping a file descriptor.
///
/// `fd_bits`: NaN-boxed integer file descriptor.
/// `is_read_bits`: NaN-boxed integer (truthy = read pipe, falsy = write pipe).
///
/// Returns a NaN-boxed integer handle for the pipe transport.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_pipe_transport_new(fd_bits: u64, is_read_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid file descriptor");
        }
        let is_read = crate::is_truthy(_py, crate::obj_from_bits(is_read_bits));
        let handle = alloc_pipe_transport(_py, fd as i32, is_read);
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_pipe_transport_new(_fd_bits: u64, _is_read_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "pipe transports are not supported on WASM",
        )
    })
}

/// Get the file descriptor from a pipe transport.
///
/// Returns a NaN-boxed integer fd, or -1 if the handle is invalid.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_get_fd(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(fd) = with_pipe(_py, handle, |state| state.fd as i64) else {
            return MoltObject::from_int(-1).bits();
        };
        MoltObject::from_int(fd).bits()
    })
}

/// Check if the pipe transport is closing.
///
/// Returns a NaN-boxed bool.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_is_closing(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(closing) = with_pipe(_py, handle, |state| state.closing) else {
            return MoltObject::from_bool(true).bits();
        };
        MoltObject::from_bool(closing).bits()
    })
}

/// Close the pipe transport.
///
/// Marks the transport as closing and closes the underlying fd.
/// For write pipes, any buffered data is flushed first.
/// Returns None.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_pipe_transport_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let mut map = pipe_transport_registry(_py).transports.lock().unwrap();
        if let Some(state) = map.get_mut(&handle) {
            close_pipe_transport_state(state);
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn close_pipe_transport_state(state: &mut PipeTransportState) {
    if state.closing {
        return;
    }
    state.closing = true;
    if !state.is_read {
        let fd = state.fd;
        for chunk in state.write_buffer.drain(..) {
            let mut offset = 0usize;
            while offset < chunk.len() {
                let rc = unsafe {
                    libc::write(
                        fd as libc::c_int,
                        chunk[offset..].as_ptr() as *const libc::c_void,
                        libc_write_count(chunk.len() - offset),
                    )
                };
                if rc <= 0 {
                    break;
                }
                offset += rc as usize;
            }
        }
    }
    unsafe {
        libc::close(state.fd as libc::c_int);
    }
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_pipe_transport_close(_handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "pipe transports are not supported on WASM",
        )
    })
}

/// Pause reading on a read pipe transport.
///
/// Returns None. Raises RuntimeError if the transport is a write pipe.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_pause_reading(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(result) = with_pipe(_py, handle, |state| {
            if !state.is_read {
                return Err("pause_reading() called on write pipe transport");
            }
            if state.closing {
                return Err("transport is closing");
            }
            state.paused = true;
            Ok(())
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "pipe transport not found");
        };
        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "RuntimeError", msg),
        }
    })
}

/// Resume reading on a read pipe transport.
///
/// Returns None. Raises RuntimeError if the transport is a write pipe.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_resume_reading(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(result) = with_pipe(_py, handle, |state| {
            if !state.is_read {
                return Err("resume_reading() called on write pipe transport");
            }
            if state.closing {
                return Err("transport is closing");
            }
            state.paused = false;
            Ok(())
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "pipe transport not found");
        };
        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "RuntimeError", msg),
        }
    })
}

/// Write data to a write pipe transport.
///
/// `data_bits`: NaN-boxed bytes object.
///
/// The data is written directly to the fd if possible; any remainder that would
/// block is buffered internally. Returns None.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_pipe_transport_write(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        // Extract bytes from the data object.
        let data = match pipe_require_bytes_slice(_py, data_bits) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        if data.is_empty() {
            return MoltObject::none().bits();
        }
        let Some(result) = with_pipe(_py, handle, |state| {
            if state.is_read {
                return Err("write() called on read pipe transport");
            }
            if state.closing {
                return Err("transport is closing");
            }
            // Try to write directly first; buffer remainder.
            let fd = state.fd;
            let mut offset = 0usize;
            while offset < data.len() {
                let rc = unsafe {
                    libc::write(
                        fd as libc::c_int,
                        data[offset..].as_ptr() as *const libc::c_void,
                        libc_write_count(data.len() - offset),
                    )
                };
                if rc < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::Interrupted
                    {
                        // Buffer remaining data for later flush.
                        state.write_buffer.push_back(data[offset..].to_vec());
                        return Ok(());
                    }
                    // Other error — buffer and let protocol handle it.
                    state.write_buffer.push_back(data[offset..].to_vec());
                    return Ok(());
                }
                offset += rc as usize;
            }
            Ok(())
        }) else {
            return raise_exception::<u64>(_py, "RuntimeError", "pipe transport not found");
        };
        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(msg) => raise_exception::<u64>(_py, "RuntimeError", msg),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_pipe_transport_write(_handle_bits: u64, _data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "pipe transports are not supported on WASM",
        )
    })
}

/// Get the write buffer size for a pipe transport.
///
/// Returns a NaN-boxed integer (total bytes buffered).
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_get_write_buffer_size(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let Some(size) = with_pipe(_py, handle, |state| {
            state
                .write_buffer
                .iter()
                .map(|chunk| chunk.len())
                .sum::<usize>() as i64
        }) else {
            return MoltObject::from_int(0).bits();
        };
        MoltObject::from_int(size).bits()
    })
}

/// Drop a pipe transport handle, removing it from the registry.
/// If the transport is not yet closed, it is closed first (native only).
/// On WASM, simply removes from the registry (no fd to close).
#[unsafe(no_mangle)]
pub extern "C" fn molt_pipe_transport_drop(handle_bits: u64) {
    // Close first to flush any pending writes and release the fd.
    // On WASM, skip close since pipe transports cannot be created there.
    #[cfg(not(target_arch = "wasm32"))]
    {
        molt_pipe_transport_close(handle_bits);
    }
    crate::with_gil_entry_nopanic!(_py, {
        let handle = crate::to_i64(crate::obj_from_bits(handle_bits)).unwrap_or(-1);
        let mut map = pipe_transport_registry(_py).transports.lock().unwrap();
        map.remove(&handle);
    });
}

/// Connect a read pipe on the event loop.
///
/// `loop_handle`: event loop handle (u64 NaN-boxed int).
/// `fd_bits`: NaN-boxed integer file descriptor.
/// `callback_bits`: NaN-boxed callable (reader callback for data_received).
///
/// Creates a PipeTransport, registers the fd as a reader on the event loop,
/// and returns the pipe transport handle.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_connect_read_pipe(
    loop_handle: u64,
    fd_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid file descriptor");
        }
        // Create the pipe transport (read mode).
        let pipe_handle = alloc_pipe_transport(_py, fd as i32, true);
        // Register the fd as a reader on the event loop.
        let Some(()) = register_pipe_reader_callback(_py, loop_handle, fd, callback_bits) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::from_int(pipe_handle).bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_connect_read_pipe(
    _loop_handle: u64,
    _fd_bits: u64,
    _callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "connect_read_pipe is not supported on WASM",
        )
    })
}

/// Connect a write pipe on the event loop.
///
/// `loop_handle`: event loop handle (u64 NaN-boxed int).
/// `fd_bits`: NaN-boxed integer file descriptor.
/// `callback_bits`: NaN-boxed callable (writer callback for write readiness).
///
/// Creates a PipeTransport, registers the fd as a writer on the event loop,
/// and returns the pipe transport handle.
#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_event_loop_connect_write_pipe(
    loop_handle: u64,
    fd_bits: u64,
    callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fd = crate::to_i64(crate::obj_from_bits(fd_bits)).unwrap_or(-1);
        if fd < 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid file descriptor");
        }
        // Create the pipe transport (write mode).
        let pipe_handle = alloc_pipe_transport(_py, fd as i32, false);
        // Register the fd as a writer on the event loop.
        let Some(()) = register_pipe_writer_callback(_py, loop_handle, fd, callback_bits) else {
            return raise_exception::<u64>(_py, "RuntimeError", "event loop not found");
        };
        MoltObject::from_int(pipe_handle).bits()
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_event_loop_connect_write_pipe(
    _loop_handle: u64,
    _fd_bits: u64,
    _callback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "connect_write_pipe is not supported on WASM",
        )
    })
}
