//! One bounded reader algorithm for native storage and Python raw streams.
//! The raw transport owns no buffered cursor; callbacks borrow operation-local
//! storage, and all Python edges remain on the existing traced file handle.
use super::buffered::{self, Operation, call, checked_raw, off_t, owned, raw_owner};
use super::*;
use crate::object::buffer_exports::{
    CallbackMemoryView, CallbackMemoryViewAccess, ScopedWritableBuffer,
};
use molt_runtime_core::OwnedRuntimeValue;

enum RawRead {
    Count(usize),
    Blocked,
}

fn bytes(py: &PyToken<'_>, data: &[u8]) -> u64 {
    let pointer = alloc_bytes(py, data);
    if pointer.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(pointer).bits()
    }
}

fn reserve(py: &PyToken<'_>, data: &mut Vec<u8>, additional: usize) -> Result<(), u64> {
    data.try_reserve(additional)
        .map_err(|_| raise_exception(py, "MemoryError", "cannot allocate buffered reader storage"))
}

unsafe fn advance_raw(pointer: *mut u8, count: usize) {
    unsafe {
        let handle = &mut *file_handle_ptr(pointer);
        handle.buffered_raw_position = handle
            .buffered_raw_position
            .and_then(|position| position.checked_add(count as i64));
    }
}

/// No Rust reference to the destination or to the file payload spans Python.
/// `owner` retains exclusive destination storage until the scoped view expires.
unsafe fn raw_read<T: ?Sized>(
    py: &PyToken<'_>,
    pointer: *mut u8,
    raw: u64,
    owner: &T,
    destination: *mut u8,
    length: usize,
) -> Result<RawRead, u64> {
    let implicit = unsafe { (*file_handle_ptr(pointer)).buffer_bits == 0 }
        && raw == MoltObject::from_ptr(pointer).bits();
    if implicit
        || !buffered::raw_callbacks(py, builtin_classes(py).buffered_reader, raw, &[b"readinto"])
    {
        let (state, memory) = unsafe {
            let handle = &*file_handle_ptr(obj_from_bits(raw).as_ptr().unwrap());
            file_handle_require_attached(py, handle)?;
            if file_handle_is_closed(handle) {
                return Err(raise_exception(py, "ValueError", "read of closed file"));
            }
            (
                file_handle_physical_state(py, handle)?.clone(),
                OwnedRuntimeValue::retain(py.core_token(), memory_backend_bits(handle)),
            )
        };
        loop {
            let result = unsafe {
                let mut backend = state.backend.lock().unwrap();
                let backend = backend.as_mut().ok_or_else(|| {
                    raise_exception::<u64>(py, "ValueError", "read of closed file")
                })?;
                super::buffer::backend_read_bytes(py, memory.bits(), backend, destination, length)
            };
            match result {
                Ok(count) => {
                    unsafe { advance_raw(pointer, count) };
                    return Ok(RawRead::Count(count));
                }
                Err(super::buffer::BackendReadError::Io(error))
                    if error.kind() == ErrorKind::WouldBlock =>
                {
                    return Ok(RawRead::Blocked);
                }
                Err(super::buffer::BackendReadError::Io(error))
                    if error.kind() == ErrorKind::Interrupted =>
                {
                    // A signal handler can reenter any public IO operation.
                    let _callback = unsafe { Operation::callback(py, pointer) }?;
                    if crate::builtins::signal_ext::signal_check_signals(py) < 0 {
                        return Err(MoltObject::none().bits());
                    }
                }
                Err(error) => return Err(error.raise(py)),
            }
        }
    }
    let _callback = unsafe { Operation::callback(py, pointer) }?;
    let view = unsafe {
        CallbackMemoryView::borrow(
            py,
            owner,
            destination,
            length,
            CallbackMemoryViewAccess::Writable,
        )
    }?;
    let result = loop {
        let result = owned(py, call(py, raw, b"readinto", Some(view.bits())));
        if !exception_pending(py) {
            break result;
        }
        if !buffered::trap_eintr(py) {
            return Err(MoltObject::none().bits());
        }
    };
    drop(view);
    if obj_from_bits(result.bits()).is_none() {
        return Ok(RawRead::Blocked);
    }
    let count = super::raw_base::count(py, result.bits());
    drop(result);
    let Some(count) = count else {
        use crate::builtins::exceptions::{
            ExceptionFieldSlot, RaisedSnapshot, exception_replace_field_bits,
        };
        let previous = crate::builtins::exceptions::take_raised(py);
        raise_exception::<u64>(py, "OSError", "raw readinto() failed");
        if let RaisedSnapshot::Thread(prior) | RaisedSnapshot::Task(_, prior) = previous
            && let Some(current) = crate::builtins::exceptions::exception_last_bits_noinc(py)
        {
            exception_replace_field_bits(py, current, ExceptionFieldSlot::Cause, prior)
                .expect("IO conversion error admits its cause");
        }
        io_chain_close_error(py, previous);
        return Err(MoltObject::none().bits());
    };
    if count < 0 || count as usize > length {
        return Err(raise_exception(
            py,
            "OSError",
            &format!(
                "raw readinto() returned invalid length {count} (should have been between 0 and {length})"
            ),
        ));
    }
    unsafe { advance_raw(pointer, count as usize) };
    Ok(RawRead::Count(count as usize))
}

/// Refill the same bounded buffer. Move its allocation out of the native
/// payload before exposing a writable callback view, restoring it on every exit.
struct FillBuffer {
    pointer: *mut u8,
    data: Vec<u8>,
    position: usize,
}
impl Drop for FillBuffer {
    fn drop(&mut self) {
        unsafe {
            let handle = &mut *file_handle_ptr(self.pointer);
            handle.read_buf = std::mem::take(&mut self.data);
            handle.read_pos = self.position;
        }
    }
}

unsafe fn reset(pointer: *mut u8) {
    unsafe {
        clear_read_buffer(&mut *file_handle_ptr(pointer));
    }
}

unsafe fn fill(py: &PyToken<'_>, pointer: *mut u8, raw: u64) -> Result<RawRead, u64> {
    let (data, position, size) = unsafe {
        let handle = &mut *file_handle_ptr(pointer);
        let size = handle.buffer_size as usize;
        if handle.read_buf.len() == size {
            clear_read_buffer(handle);
        }
        (
            std::mem::take(&mut handle.read_buf),
            std::mem::replace(&mut handle.read_pos, 0),
            size,
        )
    };
    let mut buffer = FillBuffer {
        pointer,
        data,
        position,
    };
    let start = buffer.data.len();
    reserve(py, &mut buffer.data, size - start)?;
    buffer.data.resize(size, 0);
    let destination = unsafe { buffer.data.as_mut_ptr().add(start) };
    let result = unsafe { raw_read(py, pointer, raw, &buffer, destination, size - start) };
    match result {
        Ok(RawRead::Count(count)) => {
            buffer.data.truncate(start + count);
            Ok(RawRead::Count(count))
        }
        Ok(RawRead::Blocked) => {
            buffer.data.truncate(start);
            Ok(RawRead::Blocked)
        }
        Err(error) => {
            buffer.data.truncate(start);
            Err(error)
        }
    }
}

unsafe fn consume(
    py: &PyToken<'_>,
    pointer: *mut u8,
    output: &mut Vec<u8>,
    limit: usize,
) -> Result<usize, u64> {
    let handle = unsafe { &mut *file_handle_ptr(pointer) };
    let count = unread_bytes(handle).min(limit);
    reserve(py, output, count)?;
    output.extend_from_slice(&handle.read_buf[handle.read_pos..handle.read_pos + count]);
    handle.read_pos += count;
    Ok(count)
}

unsafe fn read_all(py: &PyToken<'_>, pointer: *mut u8, raw: u64) -> u64 {
    let mut output = Vec::new();
    if let Err(error) = unsafe { consume(py, pointer, &mut output, usize::MAX) } {
        return error;
    }
    unsafe { reset(pointer) };
    let raw = if raw == MoltObject::from_ptr(pointer).bits()
        && unsafe { (*file_handle_ptr(pointer)).buffer_bits == 0 }
    {
        match unsafe { file_handle_raw_bits(py, pointer) } {
            Some(raw) => owned(py, raw),
            None => return MoltObject::none().bits(),
        }
    } else {
        OwnedRuntimeValue::retain(py.core_token(), raw)
    };
    let Some(name) = attr_name_bits_from_bytes(py, b"readall") else {
        return MoltObject::none().bits();
    };
    let _name = owned(py, name);
    let readall = match crate::builtins::platform::getattr_optional_bits(py, raw.bits(), name) {
        Ok(readall) => readall.map(|function| owned(py, function)),
        Err(error) => return error,
    };
    loop {
        let result = owned(
            py,
            match &readall {
                Some(function) => unsafe { call_callable0(py, function.bits()) },
                None => call(py, raw.bits(), b"read", None),
            },
        );
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if obj_from_bits(result.bits()).is_none() {
            return if output.is_empty() {
                result.into_bits()
            } else {
                bytes(py, &output)
            };
        }
        let Some(result_pointer) = obj_from_bits(result.bits()).as_ptr() else {
            return raise_exception(
                py,
                "TypeError",
                if readall.is_some() {
                    "readall() should return bytes"
                } else {
                    "read() should return bytes"
                },
            );
        };
        if unsafe { object_type_id(result_pointer) } != TYPE_ID_BYTES {
            return raise_exception(
                py,
                "TypeError",
                if readall.is_some() {
                    "readall() should return bytes"
                } else {
                    "read() should return bytes"
                },
            );
        }
        let data = unsafe {
            std::slice::from_raw_parts(bytes_data(result_pointer), bytes_len(result_pointer))
        };
        if readall.is_some() && output.is_empty() {
            unsafe { advance_raw(pointer, data.len()) };
            return result.into_bits();
        }
        if let Err(error) = reserve(py, &mut output, data.len()) {
            return error;
        }
        output.extend_from_slice(data);
        unsafe { advance_raw(pointer, data.len()) };
        if readall.is_some() || data.is_empty() {
            return bytes(py, &output);
        }
    }
}

pub(super) unsafe fn read(
    py: &PyToken<'_>,
    bits: u64,
    pointer: *mut u8,
    size: u64,
    once: bool,
) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let length = if !once && obj_from_bits(size).is_none() {
        -1
    } else {
        crate::builtins::numbers::index_ssize_from_obj(py, size)
    };
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    if let Err(error) = unsafe { file_handle_require_attached(py, &*file_handle_ptr(pointer)) } {
        return error;
    }
    if length < -1 && !once {
        return raise_exception(py, "ValueError", "read length must be non-negative or -1");
    }
    let _checked_raw = match unsafe { checked_raw(py, pointer, "read of closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let length = if once && length < 0 {
        unsafe { (*file_handle_ptr(pointer)).buffer_size }
    } else {
        length
    };
    if length == 0 {
        return bytes(py, &[]);
    }
    if length == -1 {
        if let Err(error) = unsafe { buffered::prepare(py, pointer, &[b"readall", b"read"], false) }
        {
            return error;
        }
        let _operation = match unsafe { Operation::enter(py, pointer, true) } {
            Ok(operation) => operation,
            Err(error) => return error,
        };
        let raw = match unsafe { raw_owner(py, pointer) } {
            Ok(raw) => raw,
            Err(error) => return error,
        };
        return unsafe { read_all(py, pointer, raw.bits()) };
    }
    let length = length as usize;
    let available = unsafe { unread_bytes(&*file_handle_ptr(pointer)) };
    if available >= length || (once && available != 0) {
        let handle = unsafe { &*file_handle_ptr(pointer) };
        let count = available.min(length);
        let result = bytes(
            py,
            &handle.read_buf[handle.read_pos..handle.read_pos + count],
        );
        if !exception_pending(py) {
            unsafe {
                (*file_handle_ptr(pointer)).read_pos += count;
            }
        }
        return result;
    }
    if let Err(error) = unsafe { buffered::prepare(py, pointer, &[b"readinto"], false) } {
        return error;
    }
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(operation) => operation,
        Err(error) => return error,
    };
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let mut output = Vec::new();
    if let Err(error) = unsafe { consume(py, pointer, &mut output, length) } {
        return error;
    }
    if output.len() == length || (once && !output.is_empty()) {
        return bytes(py, &output);
    }
    unsafe { reset(pointer) };
    let buffer_size = unsafe { (*file_handle_ptr(pointer)).buffer_size as usize };
    while output.len() < length {
        let remaining = length - output.len();
        let direct = if once {
            remaining
        } else {
            remaining / buffer_size * buffer_size
        };
        let result = if direct != 0 {
            if let Err(error) = reserve(py, &mut output, direct) {
                return error;
            }
            let start = output.len();
            output.resize(start + direct, 0);
            let destination = unsafe { output.as_mut_ptr().add(start) };
            let result = unsafe { raw_read(py, pointer, raw.bits(), &output, destination, direct) };
            output.truncate(
                start
                    + match result {
                        Ok(RawRead::Count(count)) => count,
                        _ => 0,
                    },
            );
            result
        } else {
            let result = unsafe { fill(py, pointer, raw.bits()) };
            if let Ok(RawRead::Count(count)) = result
                && count != 0
                && let Err(error) = unsafe { consume(py, pointer, &mut output, remaining) }
            {
                return error;
            }
            result
        };
        match result {
            Err(error) => return error,
            Ok(RawRead::Blocked) if output.is_empty() && !once => return MoltObject::none().bits(),
            Ok(RawRead::Blocked | RawRead::Count(0)) => break,
            Ok(RawRead::Count(_)) if once => break,
            Ok(RawRead::Count(_)) => {}
        }
    }
    bytes(py, &output)
}

pub(super) unsafe fn readinto(
    py: &PyToken<'_>,
    bits: u64,
    pointer: *mut u8,
    buffer: &ScopedWritableBuffer<'_, '_>,
    once: bool,
) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let _checked_raw = match unsafe { checked_raw(py, pointer, "readinto of closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let mut written = unsafe {
        let handle = &mut *file_handle_ptr(pointer);
        let count = unread_bytes(handle).min(buffer.len());
        std::ptr::copy(
            handle.read_buf.as_ptr().add(handle.read_pos),
            buffer.as_mut_ptr(),
            count,
        );
        handle.read_pos += count;
        count
    };
    if written == buffer.len() {
        return int_bits_from_i64(py, written as i64);
    }
    if let Err(error) = unsafe { buffered::prepare(py, pointer, &[b"readinto"], false) } {
        return error;
    }
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(operation) => operation,
        Err(error) => return error,
    };
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    unsafe { reset(pointer) };
    let size = unsafe { (*file_handle_ptr(pointer)).buffer_size as usize };
    loop {
        if written == buffer.len() {
            break;
        }
        let remaining = buffer.len() - written;
        let result = if remaining > size {
            unsafe {
                raw_read(
                    py,
                    pointer,
                    raw.bits(),
                    buffer,
                    buffer.as_mut_ptr().add(written),
                    remaining,
                )
            }
        } else if once && written != 0 {
            break;
        } else {
            unsafe { fill(py, pointer, raw.bits()) }
        };
        match result {
            Err(error) => return error,
            Ok(RawRead::Blocked) if written == 0 => return MoltObject::none().bits(),
            Ok(RawRead::Blocked | RawRead::Count(0)) => break,
            Ok(RawRead::Count(count)) if remaining > size => {
                written += count;
                if once {
                    break;
                }
            }
            Ok(RawRead::Count(_)) => {
                let handle = unsafe { &mut *file_handle_ptr(pointer) };
                let count = unread_bytes(handle).min(remaining);
                unsafe {
                    std::ptr::copy(
                        handle.read_buf.as_ptr().add(handle.read_pos),
                        buffer.as_mut_ptr().add(written),
                        count,
                    );
                }
                handle.read_pos += count;
                written += count;
                if once {
                    break;
                }
            }
        }
    }
    int_bits_from_i64(py, written as i64)
}

pub(super) unsafe fn peek(py: &PyToken<'_>, bits: u64, pointer: *mut u8, size: u64) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let _size = crate::builtins::numbers::index_ssize_from_obj(py, size);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    if let Err(error) = unsafe { buffered::prepare(py, pointer, &[b"readinto"], false) } {
        return error;
    }
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(operation) => operation,
        Err(error) => return error,
    };
    let raw = match unsafe { checked_raw(py, pointer, "peek of closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    if unsafe { unread_bytes(&*file_handle_ptr(pointer)) == 0 } {
        unsafe { reset(pointer) };
        if let Err(error) = unsafe { fill(py, pointer, raw.bits()) } {
            return error;
        }
    }
    let handle = unsafe { &*file_handle_ptr(pointer) };
    bytes(py, &handle.read_buf[handle.read_pos..])
}

pub(super) unsafe fn readline(py: &PyToken<'_>, bits: u64, pointer: *mut u8, size: u64) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let limit = if obj_from_bits(size).is_none() {
        -1
    } else {
        crate::builtins::numbers::index_ssize_from_obj(py, size)
    };
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let _checked_raw = match unsafe { checked_raw(py, pointer, "readline of closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let mut output = Vec::new();
    let limit = if limit < 0 {
        usize::MAX
    } else {
        limit as usize
    };
    let (fast_count, fast_complete) = unsafe {
        let handle = &*file_handle_ptr(pointer);
        let count = unread_bytes(handle).min(limit);
        match handle.read_buf[handle.read_pos..handle.read_pos + count]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            Some(end) => (end + 1, true),
            None => (count, count == limit),
        }
    };
    if fast_complete {
        let handle = unsafe { &*file_handle_ptr(pointer) };
        let result = bytes(
            py,
            &handle.read_buf[handle.read_pos..handle.read_pos + fast_count],
        );
        if !exception_pending(py) {
            unsafe {
                (*file_handle_ptr(pointer)).read_pos += fast_count;
            }
        }
        return result;
    }
    if let Err(error) = unsafe { buffered::prepare(py, pointer, &[b"readinto"], false) } {
        return error;
    }
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(operation) => operation,
        Err(error) => return error,
    };
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    while output.len() < limit {
        let (count, newline) = unsafe {
            let handle = &*file_handle_ptr(pointer);
            let available = unread_bytes(handle).min(limit - output.len());
            let data = &handle.read_buf[handle.read_pos..handle.read_pos + available];
            match data.iter().position(|byte| *byte == b'\n') {
                Some(end) => (end + 1, true),
                None => (available, false),
            }
        };
        if let Err(error) = unsafe { consume(py, pointer, &mut output, count) } {
            return error;
        }
        if newline || output.len() == limit {
            break;
        }
        unsafe { reset(pointer) };
        match unsafe { fill(py, pointer, raw.bits()) } {
            Err(error) => return error,
            Ok(RawRead::Blocked | RawRead::Count(0)) => break,
            Ok(RawRead::Count(_)) => {}
        }
    }
    bytes(py, &output)
}

pub(super) unsafe fn readlines(py: &PyToken<'_>, bits: u64, hint: u64) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let hint = if obj_from_bits(hint).is_none() {
        -1
    } else {
        crate::builtins::numbers::index_ssize_from_obj(py, hint)
    };
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let list = alloc_list(py, &[]);
    if list.is_null() {
        return MoltObject::none().bits();
    }
    let result = owned(py, MoltObject::from_ptr(list).bits());
    let iterator = owned(py, molt_iter(bits));
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let mut total = 0usize;
    loop {
        use crate::object::iterable::SpecialIterationStep;
        let line = match crate::object::ops_iter::iteration_step(py, iterator.bits()) {
            Ok(SpecialIterationStep::Item(value)) => owned(py, value),
            Ok(SpecialIterationStep::Exhausted(value)) => {
                dec_ref_bits(py, value);
                break;
            }
            Ok(SpecialIterationStep::Missing) => {
                return raise_exception(py, "TypeError", "object is not an iterator");
            }
            Err(()) => return MoltObject::none().bits(),
        };
        molt_list_append(result.bits(), line.bits());
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if hint > 0 {
            let length = owned(py, molt_len(line.bits()));
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            total = total.saturating_add(to_i64(obj_from_bits(length.bits())).unwrap() as usize);
            if total > hint as usize {
                break;
            }
        }
    }
    result.into_bits()
}

unsafe fn raw_tell(py: &PyToken<'_>, pointer: *mut u8, raw: u64) -> Result<i64, u64> {
    let implicit = raw == MoltObject::from_ptr(pointer).bits()
        && unsafe { (*file_handle_ptr(pointer)).buffer_bits == 0 };
    if implicit
        || !buffered::raw_callbacks(py, builtin_classes(py).buffered_reader, raw, &[b"tell"])
    {
        let position = unsafe {
            let handle = &*file_handle_ptr(obj_from_bits(raw).as_ptr().unwrap());
            file_handle_require_attached(py, handle)?;
            let state = file_handle_physical_state(py, handle)?.clone();
            let mut backend = state.backend.lock().unwrap();
            let backend = backend.as_mut().ok_or_else(|| {
                raise_exception::<u64>(py, "ValueError", "I/O operation on closed file")
            })?;
            backend_tell(py, &state, backend)?
        };
        let position = i64::try_from(position).map_err(|_| {
            raise_exception::<u64>(
                py,
                "ValueError",
                "cannot fit 'int' into an offset-sized integer",
            )
        })?;
        unsafe {
            (*file_handle_ptr(pointer)).buffered_raw_position = Some(position);
        }
        return Ok(position);
    }
    let value = owned(py, call(py, raw, b"tell", None));
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    let position = off_t(py, value.bits()).ok_or_else(|| MoltObject::none().bits())?;
    if position < 0 {
        return Err(raise_exception(
            py,
            "OSError",
            &format!("Raw stream returned invalid position {position}"),
        ));
    }
    unsafe {
        (*file_handle_ptr(pointer)).buffered_raw_position = Some(position);
    }
    Ok(position)
}

pub(super) unsafe fn tell(py: &PyToken<'_>, bits: u64, pointer: *mut u8) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let position = match unsafe { raw_tell(py, pointer, raw.bits()) } {
        Ok(position) => position,
        Err(error) => return error,
    };
    if let Err(error) = unsafe { file_handle_require_attached(py, &*file_handle_ptr(pointer)) } {
        return error;
    }
    int_bits_from_i64(
        py,
        position
            .saturating_sub(unsafe { unread_bytes(&*file_handle_ptr(pointer)) } as i64)
            .max(0),
    )
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
    if let Err(error) = unsafe { file_handle_require_attached(py, &*file_handle_ptr(pointer)) } {
        return error;
    }
    let supported = (0..=2).contains(&whence);
    #[cfg(unix)]
    let supported = supported || whence == libc::SEEK_HOLE || whence == libc::SEEK_DATA;
    if !supported {
        return raise_exception(
            py,
            "ValueError",
            &format!("whence value {whence} unsupported"),
        );
    }
    if let Err(error) =
        unsafe { buffered::prepare(py, pointer, &[b"seekable", b"tell", b"seek"], false) }
    {
        return error;
    }
    let raw = match unsafe { checked_raw(py, pointer, "seek of closed file") } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let seekable = owned(py, call(py, raw.bits(), b"seekable", None));
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    if seekable.bits() != MoltObject::from_bool(true).bits() {
        return raise_exception(
            py,
            "UnsupportedOperation",
            "File or stream is not seekable.",
        );
    }
    let Some(offset) = off_t(py, offset) else {
        return MoltObject::none().bits();
    };
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    if whence == 0 || whence == 1 {
        let position = match unsafe { (*file_handle_ptr(pointer)).buffered_raw_position } {
            Some(position) => position,
            None => match unsafe { raw_tell(py, pointer, raw.bits()) } {
                Ok(position) => position,
                Err(error) => return error,
            },
        };
        let handle = unsafe { &mut *file_handle_ptr(pointer) };
        let available = unread_bytes(handle) as i128;
        let logical = i128::from(position) - available;
        let delta = if whence == 0 {
            i128::from(offset) - logical
        } else {
            i128::from(offset)
        };
        if available > 0 && delta >= -(handle.read_pos as i128) && delta <= available {
            handle.read_pos = (handle.read_pos as i128 + delta) as usize;
            return int_bits_from_i64(py, (logical + delta).max(0) as i64);
        }
    }
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(operation) => operation,
        Err(error) => return error,
    };
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let offset = if whence == 1 {
        match offset.checked_sub(unsafe { unread_bytes(&*file_handle_ptr(pointer)) } as i64) {
            Some(offset) => offset,
            None => {
                return raise_exception(
                    py,
                    "OverflowError",
                    "buffered position exceeds file offset range",
                );
            }
        }
    } else {
        offset
    };
    let offset = owned(py, int_bits_from_i64(py, offset));
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    // An implicit native raw is materialized only for the actual public seek
    // observation. Buffered seeks above neither allocate it nor call raw.seek.
    let raw = if raw.bits() == bits && unsafe { (*file_handle_ptr(pointer)).buffer_bits == 0 } {
        match unsafe { file_handle_raw_bits(py, pointer) } {
            Some(raw) => owned(py, raw),
            None => return MoltObject::none().bits(),
        }
    } else {
        raw
    };
    let Some(name) = attr_name_bits_from_bytes(py, b"seek") else {
        return MoltObject::none().bits();
    };
    let _name = owned(py, name);
    let method = owned(py, molt_getattr_builtin(raw.bits(), name, missing_bits(py)));
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let result = owned(py, unsafe {
        call_callable2(
            py,
            method.bits(),
            offset.bits(),
            MoltObject::from_int(whence as i64).bits(),
        )
    });
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let Some(position) = off_t(py, result.bits()) else {
        return MoltObject::none().bits();
    };
    if position < 0 {
        return raise_exception(
            py,
            "OSError",
            &format!("Raw stream returned invalid position {position}"),
        );
    }
    unsafe {
        reset(pointer);
        (*file_handle_ptr(pointer)).buffered_raw_position = Some(position);
    }
    int_bits_from_i64(py, position)
}

pub(super) unsafe fn flush(py: &PyToken<'_>, bits: u64, pointer: *mut u8) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    match unsafe { checked_raw(py, pointer, "flush of closed file") } {
        Ok(_) => MoltObject::none().bits(),
        Err(error) => error,
    }
}
