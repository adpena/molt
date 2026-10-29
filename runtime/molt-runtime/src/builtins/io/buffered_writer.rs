//! The existing BufferedWriter's Python raw-stream operation path.
//! Physical raw streams keep the callback-free backend path. This state owns
//! only the existing bounded write_buf; callback views borrow operation scope.
use super::buffered::{
    Operation, call, checked_raw, off_t, owned, pending_matches, raw_callbacks, raw_owner,
    trap_eintr,
};
use super::*;
use crate::BuiltinExc;
use crate::object::BufferedOperation;
use crate::object::buffer_exports::ScopedBuffer;
use molt_runtime_core::OwnedRuntimeValue;

// tell() is deliberately callable during raw.write(). Preserve its logical
// pending count while PendingWrite owns the Vec outside the Python payload.
struct PendingObservation {
    operation: Arc<BufferedOperation>,
    previous: Option<usize>,
}
impl PendingObservation {
    unsafe fn enter(pointer: *mut u8, count: usize) -> Self {
        let operation = unsafe {
            (*file_handle_ptr(pointer))
                .buffered_operation
                .as_ref()
                .unwrap()
                .clone()
        };
        let previous = operation
            .owner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .replace(count);
        Self {
            operation,
            previous,
        }
    }
}
impl Drop for PendingObservation {
    fn drop(&mut self) {
        self.operation
            .owner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending = self.previous;
    }
}

unsafe fn check_signals(py: &PyToken<'_>, pointer: *mut u8, pending: usize) -> Result<(), u64> {
    if !crate::builtins::signal_ext::signal_tripped() {
        return Ok(());
    }
    let _operation = unsafe { Operation::callback(py, pointer) }?;
    let _pending = unsafe { PendingObservation::enter(pointer, pending) };
    if crate::builtins::signal_ext::signal_check_signals(py) < 0 {
        Err(MoltObject::none().bits())
    } else {
        Ok(())
    }
}

fn blocking(py: &PyToken<'_>, errno: i32, written: usize) -> u64 {
    let message = alloc_string(py, b"write could not complete without blocking");
    if message.is_null() {
        return MoltObject::none().bits();
    }
    let _message = owned(py, MoltObject::from_ptr(message).bits());
    let count = int_bits_from_i64(py, written as i64);
    let _count = owned(py, count);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let args = alloc_tuple(
        py,
        &[
            MoltObject::from_int(errno as i64).bits(),
            _message.bits(),
            count,
        ],
    );
    if args.is_null() {
        return MoltObject::none().bits();
    }
    let _args = owned(py, MoltObject::from_ptr(args).bits());
    let class = crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::BlockingIOError);
    let exception = crate::builtins::exceptions::molt_exception_new_from_class(class, _args.bits());
    let _exception = owned(py, exception);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    crate::molt_raise(exception)
}

// The existing C-API portability shim owns the target's errno cell.
unsafe extern "C" {
    fn molt_capi_errno() -> i32;
    fn molt_capi_set_errno(value: i32);
}
enum RawWrite {
    Written(usize),
    Blocked(i32),
}

/// Physical errors carry SDK errno; callback/C-API errors already carry the
/// public Python domain and never cross this adoption boundary.
pub(super) fn physical_blocked_errno(error: &std::io::Error) -> i32 {
    #[cfg(target_arch = "wasm32")]
    {
        molt_runtime_platform::synthetic_errno::from_io_error(error).code()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        error.raw_os_error().unwrap_or(libc::EAGAIN)
    }
}

unsafe fn raw_write<T: ?Sized>(
    py: &PyToken<'_>,
    writer: *mut u8,
    raw: u64,
    owner: &T,
    data: *mut u8,
    len: usize,
    pending: usize,
) -> Result<RawWrite, u64> {
    let implicit_source = raw == MoltObject::from_ptr(writer).bits();
    let mut raw = raw;
    loop {
        // A signal handler may observe .raw/tell and materialize the existing
        // exact raw wrapper. It shares the same physical backend ownership.
        if implicit_source && unsafe { (*file_handle_ptr(writer)).buffer_bits != 0 } {
            raw = unsafe { (*file_handle_ptr(writer)).buffer_bits };
        }
        let implicit = unsafe { (*file_handle_ptr(writer)).buffer_bits == 0 }
            && raw == MoltObject::from_ptr(writer).bits();
        if !implicit && raw_callbacks(py, builtin_classes(py).buffered_writer, raw, &[b"write"]) {
            break;
        }
        let pointer = obj_from_bits(raw).as_ptr().expect("proven physical raw");
        let result = unsafe {
            let handle = &*file_handle_ptr(pointer);
            file_handle_require_attached(py, handle)?;
            if file_handle_is_closed(handle) {
                return Err(raise_exception(
                    py,
                    BuiltinExc::ValueError,
                    "I/O operation on closed file",
                ));
            }
            if !handle.writable {
                return Err(raise_exception(
                    py,
                    BuiltinExc::UnsupportedOperation,
                    "not writable",
                ));
            }
            let state = file_handle_physical_state(py, handle)?.clone();
            let mem = memory_backend_bits(handle);
            let mut backend = state.backend.lock().unwrap();
            let Some(backend) = backend.as_mut() else {
                return Err(raise_exception(
                    py,
                    BuiltinExc::ValueError,
                    "I/O operation on closed file",
                ));
            };
            let bytes = if len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(data, len)
            };
            backend_write_bytes(py, mem, backend, bytes)
        };
        match result {
            Ok(count) => return Ok(RawWrite::Written(count)),
            Err(super::buffer::BackendWriteError::Io(error))
                if error.kind() == ErrorKind::WouldBlock =>
            {
                return Ok(RawWrite::Blocked(physical_blocked_errno(&error)));
            }
            Err(super::buffer::BackendWriteError::Io(error))
                if error.kind() == ErrorKind::Interrupted =>
            {
                unsafe { check_signals(py, writer, pending) }?;
            }
            Err(error) => return Err(error.raise(py)),
        }
    }
    let _operation = unsafe { Operation::callback(py, writer) }?;
    let _pending = unsafe { PendingObservation::enter(writer, pending) };
    let scope = unsafe {
        crate::object::buffer_exports::CallbackMemoryView::borrow(
            py,
            owner,
            data,
            len,
            crate::object::buffer_exports::CallbackMemoryViewAccess::ReadOnly,
        )
    }?;
    let view = scope.bits();
    let (result, errno) = loop {
        unsafe { molt_capi_set_errno(0) };
        let result = call(py, raw, b"write", Some(view));
        let errno = unsafe { molt_capi_errno() };
        if !exception_pending(py) {
            break (result, errno);
        }
        dec_ref_bits(py, result);
        if !trap_eintr(py) {
            return Err(MoltObject::none().bits());
        }
    };
    drop(scope);
    let _result = owned(py, result);
    if obj_from_bits(result).is_none() {
        return Ok(RawWrite::Blocked(errno));
    }
    let converted = crate::builtins::numbers::index_ssize_from_obj(py, result);
    // CPython intentionally replaces failed result conversion with this OSError.
    let count = if exception_pending(py) {
        clear_exception(py);
        -1
    } else {
        converted
    };
    if count < 0 || count as usize > len {
        return Err(raise_exception(
            py,
            BuiltinExc::OSError,
            &format!(
                "raw write() returned invalid length {count} (should have been between 0 and {len})"
            ),
        ));
    }
    Ok(RawWrite::Written(count as usize))
}

unsafe fn pending(pointer: *mut u8) -> (usize, usize, usize) {
    let handle = unsafe { &*file_handle_ptr(pointer) };
    (
        handle.write_start,
        handle.write_buf.len(),
        handle.buffer_size.max(0) as usize,
    )
}

unsafe fn append(py: &PyToken<'_>, pointer: *mut u8, input: &[u8]) -> Result<(), u64> {
    let handle = unsafe { &mut *file_handle_ptr(pointer) };
    debug_assert!(input.len() <= handle.buffer_size as usize - handle.write_buf.len());
    if handle.write_buf.try_reserve(input.len()).is_err() {
        return Err(raise_exception(
            py,
            BuiltinExc::MemoryError,
            "cannot allocate buffered writer storage",
        ));
    }
    handle.write_buf.extend_from_slice(input);
    Ok(())
}

unsafe fn compact(_py: &PyToken<'_>, pointer: *mut u8) -> Result<(), u64> {
    let handle = unsafe { &mut *file_handle_ptr(pointer) };
    if handle.write_start != 0 {
        handle.write_buf.drain(..handle.write_start);
        handle.write_start = 0;
    }
    Ok(())
}

/// Temporarily own the existing pending Vec across raw callbacks. No payload
/// borrow crosses Python, and every outcome restores capacity and exact prefix.
struct PendingWrite {
    pointer: *mut u8,
    bytes: Vec<u8>,
    start: usize,
}
impl PendingWrite {
    unsafe fn take(pointer: *mut u8) -> Self {
        let handle = unsafe { &mut *file_handle_ptr(pointer) };
        Self {
            pointer,
            bytes: std::mem::take(&mut handle.write_buf),
            start: std::mem::replace(&mut handle.write_start, 0),
        }
    }
}
impl Drop for PendingWrite {
    fn drop(&mut self) {
        unsafe {
            let handle = &mut *file_handle_ptr(self.pointer);
            debug_assert!(handle.write_buf.is_empty());
            if self.start == self.bytes.len() {
                self.bytes.clear();
                self.start = 0;
            }
            handle.write_buf = std::mem::take(&mut self.bytes);
            handle.write_start = self.start;
        }
    }
}

unsafe fn drain(py: &PyToken<'_>, pointer: *mut u8, raw: u64) -> Result<Option<i32>, u64> {
    let mut pending = unsafe { PendingWrite::take(pointer) };
    while pending.start < pending.bytes.len() {
        let data = unsafe { pending.bytes.as_ptr().add(pending.start).cast_mut() };
        let count = match unsafe {
            raw_write(
                py,
                pointer,
                raw,
                &pending,
                data,
                pending.bytes.len() - pending.start,
                pending.bytes.len() - pending.start,
            )
        }? {
            RawWrite::Written(count) => count,
            RawWrite::Blocked(errno) => return Ok(Some(errno)),
        };
        pending.start += count;
        unsafe { check_signals(py, pointer, pending.bytes.len() - pending.start) }?;
    }
    Ok(None)
}

pub(super) unsafe fn write(
    py: &PyToken<'_>,
    _bits: u64,
    pointer: *mut u8,
    input: ScopedBuffer<'_, '_>,
    length: usize,
) -> u64 {
    // The shared entry owns the receiver and admitted input before choosing
    // the current raw implementation, including exporter-triggered reinit.
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(owner) => owner,
        Err(error) => return error,
    };
    let raw = match unsafe { checked_raw(py, pointer, "write to closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let (_, end, capacity) = unsafe { pending(pointer) };
    let eager_full = crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
    if length <= capacity - end && (!eager_full || length < capacity) {
        if let Err(error) = unsafe { append(py, pointer, input.as_slice()) } {
            return error;
        }
        return int_bits_from_i64(py, length as i64);
    }
    let drained = unsafe { drain(py, pointer, raw.bits()) };
    let blocked_errno = match drained {
        Ok(Some(errno)) => errno,
        _ => unsafe { molt_capi_errno() },
    };
    match drained {
        Err(error) if !pending_matches(py, BuiltinExc::BlockingIOError) => return error,
        Err(_) | Ok(Some(_)) => {
            clear_exception(py);
            if let Err(error) = unsafe { compact(py, pointer) } {
                return error;
            }
            let (_, end, _) = unsafe { pending(pointer) };
            let accepted = length.min(capacity - end);
            if let Err(error) = unsafe { append(py, pointer, &input.as_slice()[..accepted]) } {
                return error;
            }
            if accepted == length {
                return int_bits_from_i64(py, length as i64);
            }
            return blocking(py, blocked_errno, accepted);
        }
        Ok(None) => {}
    }
    let mut accepted = 0usize;
    while length - accepted > capacity || (eager_full && length - accepted == capacity) {
        match unsafe {
            raw_write(
                py,
                pointer,
                raw.bits(),
                &input,
                input.view().data.wrapping_add(accepted),
                length - accepted,
                0,
            )
        } {
            Err(error) => return error,
            Ok(RawWrite::Blocked(errno)) => {
                if let Err(error) = unsafe {
                    append(
                        py,
                        pointer,
                        &input.as_slice()[accepted..accepted + capacity],
                    )
                } {
                    return error;
                }
                return blocking(py, errno, accepted + capacity);
            }
            Ok(RawWrite::Written(count)) => accepted += count,
        }
        if let Err(error) = unsafe { check_signals(py, pointer, 0) } {
            return error;
        }
    }
    if let Err(error) = unsafe { append(py, pointer, &input.as_slice()[accepted..]) } {
        return error;
    }
    int_bits_from_i64(py, length as i64)
}

pub(super) unsafe fn flush(py: &PyToken<'_>, bits: u64, pointer: *mut u8) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(owner) => owner,
        Err(error) => return error,
    };
    let raw = match unsafe { checked_raw(py, pointer, "flush of closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    match unsafe { drain(py, pointer, raw.bits()) } {
        Ok(None) => MoltObject::none().bits(),
        Ok(Some(errno)) => blocking(py, errno, 0),
        Err(error) => error,
    }
}

pub(super) unsafe fn tell(py: &PyToken<'_>, bits: u64, pointer: *mut u8) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let result = call(py, raw.bits(), b"tell", None);
    let _result = owned(py, result);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let Some(position) = off_t(py, result) else {
        return MoltObject::none().bits();
    };
    if position < 0 {
        return raise_exception(
            py,
            BuiltinExc::OSError,
            &format!("Raw stream returned invalid position {position}"),
        );
    }
    let (start, end, _) = unsafe { pending(pointer) };
    let count = unsafe { (*file_handle_ptr(pointer)).buffered_operation.as_ref() }
        .and_then(|operation| {
            operation
                .owner
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pending
        })
        .unwrap_or(end - start);
    match position.checked_add(count as i64) {
        Some(position) => int_bits_from_i64(py, position),
        None => raise_exception(
            py,
            BuiltinExc::OverflowError,
            "buffered position exceeds file offset range",
        ),
    }
}

pub(super) unsafe fn seek(
    py: &PyToken<'_>,
    bits: u64,
    pointer: *mut u8,
    offset: u64,
    whence: u64,
) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let Some(whence) = crate::builtins::numbers::index_c_int_from_obj(py, whence) else {
        return MoltObject::none().bits();
    };
    if let Err(error) = unsafe { raw_owner(py, pointer) } {
        return error;
    }
    let supported = (0..=2).contains(&whence);
    #[cfg(unix)]
    let supported = supported || whence == libc::SEEK_HOLE || whence == libc::SEEK_DATA;
    if !supported {
        return raise_exception(
            py,
            BuiltinExc::ValueError,
            &format!("whence value {whence} unsupported"),
        );
    }
    let raw = match unsafe { checked_raw(py, pointer, "seek of closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let seekable = call(py, raw.bits(), b"seekable", None);
    let _seekable = owned(py, seekable);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    if seekable != MoltObject::from_bool(true).bits() {
        return raise_exception(
            py,
            BuiltinExc::UnsupportedOperation,
            "File or stream is not seekable.",
        );
    }
    let Some(offset) = off_t(py, offset) else {
        return MoltObject::none().bits();
    };
    let offset = owned(py, int_bits_from_i64(py, offset));
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(owner) => owner,
        Err(error) => return error,
    };
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    match unsafe { drain(py, pointer, raw.bits()) } {
        Ok(None) => {}
        Ok(Some(errno)) => return blocking(py, errno, 0),
        Err(error) => return error,
    }
    let Some(key) = attr_name_bits_from_bytes(py, b"seek") else {
        return MoltObject::none().bits();
    };
    let _key = owned(py, key);
    let function = molt_getattr_builtin(raw.bits(), key, missing_bits(py));
    let _function = owned(py, function);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let result = unsafe {
        call_callable2(
            py,
            function,
            offset.bits(),
            MoltObject::from_int(whence as i64).bits(),
        )
    };
    let _result = owned(py, result);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let Some(position) = off_t(py, result) else {
        return MoltObject::none().bits();
    };
    if position < 0 {
        return raise_exception(
            py,
            BuiltinExc::OSError,
            &format!("Raw stream returned invalid position {position}"),
        );
    }
    int_bits_from_i64(py, position)
}

pub(super) unsafe fn truncate(py: &PyToken<'_>, bits: u64, pointer: *mut u8, size: u64) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let _checked_raw = match unsafe { checked_raw(py, pointer, "truncate of closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(owner) => owner,
        Err(error) => return error,
    };
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    match unsafe { drain(py, pointer, raw.bits()) } {
        Ok(None) => {}
        Ok(Some(errno)) => return blocking(py, errno, 0),
        Err(error) => return error,
    }
    let result = call(py, raw.bits(), b"truncate", Some(size));
    let result = owned(py, result);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let position = call(py, raw.bits(), b"tell", None);
    let _position = owned(py, position);
    if !exception_pending(py) {
        let _ = off_t(py, position);
    }
    if exception_pending(py) {
        clear_exception(py);
    }
    result.into_bits()
}
