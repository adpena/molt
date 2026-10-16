//! The existing BufferedWriter's Python raw-stream operation path.
//! Physical raw streams keep the callback-free backend path. This state owns
//! only the existing bounded write_buf; callback views borrow operation scope.
use super::*;
use crate::object::BufferedWriteOperation;
use crate::object::buffer_exports::ScopedBuffer;
use molt_runtime_core::OwnedRuntimeValue;
use num_traits::ToPrimitive;

pub(super) unsafe fn applies(pointer: *mut u8) -> bool {
    let handle = unsafe { file_handle_ptr(pointer) };
    !handle.is_null() && unsafe { (*handle).write_operation.is_some() }
}

fn overrides(py: &PyToken<'_>, pointer: *mut u8, names: &[&[u8]]) -> bool {
    unsafe {
        let Some(dictionary) = obj_from_bits(instance_dict_bits(pointer)).as_ptr() else {
            return false;
        };
        names.iter().any(|name| {
            !matches!(
                crate::dict_exact_string_lookup(
                    py,
                    dictionary,
                    name,
                    crate::hash_string_bytes(py, name) as u64
                ),
                crate::ExactStringLookup::Absent,
            )
        })
    }
}

fn raw_callbacks(py: &PyToken<'_>, class: u64, raw: u64, methods: &[&[u8]]) -> bool {
    let classes = builtin_classes(py);
    let Some(pointer) = obj_from_bits(raw).as_ptr() else {
        return true;
    };
    unsafe {
        if class != classes.buffered_writer || object_type_id(pointer) != TYPE_ID_FILE_HANDLE {
            return true;
        }
        let raw_class = object_class_bits(pointer);
        if ![classes.file_io, classes.bytes_io].contains(&raw_class)
            || !crate::object::class_is_immutable(py, obj_from_bits(raw_class).as_ptr().unwrap())
        {
            return true;
        }
        overrides(py, pointer, methods)
    }
}

pub(super) unsafe fn is_writer(py: &PyToken<'_>, pointer: *mut u8) -> bool {
    unsafe {
        let classes = builtin_classes(py);
        let class = object_class_bits(pointer);
        if applies(pointer) || class == classes.buffered_writer {
            return true;
        }
        // Initialized subclasses already own their callback operation. Only
        // genuine uninitialized instances need the constructor's MRO admission.
        if (*file_handle_ptr(pointer)).attachment != crate::object::FileAttachment::Uninitialized {
            return false;
        }
        let Some(class) = obj_from_bits(class).as_ptr() else {
            return false;
        };
        crate::class_mro_view(py, class)
            .iter()
            .copied()
            .find(|base| {
                [
                    classes.buffered_writer,
                    classes.buffered_reader,
                    classes.buffered_random,
                ]
                .contains(base)
            })
            == Some(classes.buffered_writer)
    }
}

pub(super) fn needs_callback(py: &PyToken<'_>, class: u64, raw: u64) -> bool {
    raw_callbacks(py, class, raw, &[b"writable", b"tell"])
}

/// Probe only methods this operation invokes. Absent instance dictionaries
/// prove exact builtin bindings in O(1); ambiguous dictionary equality enters
/// ordinary lookup. A changed binding needs no storage conversion or copy.
pub(super) unsafe fn prepare(
    py: &PyToken<'_>,
    pointer: *mut u8,
    methods: &[&[u8]],
    self_flush: bool,
) -> Result<(), u64> {
    unsafe {
        let handle = &*file_handle_ptr(pointer);
        if handle.write_operation.is_some()
            || object_class_bits(pointer) != builtin_classes(py).buffered_writer
            || handle.attachment != crate::object::FileAttachment::Attached
        {
            return Ok(());
        }
        let own_override = self_flush && overrides(py, pointer, &[b"flush"]);
        if !own_override
            && (handle.buffer_bits == 0
                || !raw_callbacks(py, object_class_bits(pointer), handle.buffer_bits, methods))
        {
            return Ok(());
        }
        if handle.buffer_bits == 0 {
            let raw = super::handle::file_handle_raw_bits(py, pointer)
                .ok_or_else(|| MoltObject::none().bits())?;
            dec_ref_bits(py, raw);
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
        }
        (*file_handle_ptr(pointer)).write_operation = Some(Arc::new(BufferedWriteOperation {
            owner: Mutex::new(Default::default()),
            ready: std::sync::Condvar::new(),
        }));
        Ok(())
    }
}

/// A user index conversion may replace the raw stream or its methods. Enter
/// the existing raw protocol for that operation before performing conversion;
/// immediate integral arguments retain the physical path without this owner.
pub(super) unsafe fn prepare_conversion(py: &PyToken<'_>, pointer: *mut u8) -> Result<(), u64> {
    unsafe {
        if !is_writer(py, pointer) || applies(pointer) {
            return Ok(());
        }
        file_handle_require_attached(py, &*file_handle_ptr(pointer))?;
        if (*file_handle_ptr(pointer)).buffer_bits == 0 {
            let raw = super::handle::file_handle_raw_bits(py, pointer)
                .ok_or_else(|| MoltObject::none().bits())?;
            dec_ref_bits(py, raw);
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
        }
        (*file_handle_ptr(pointer)).write_operation = Some(Arc::new(BufferedWriteOperation {
            owner: Mutex::new(Default::default()),
            ready: std::sync::Condvar::new(),
        }));
        Ok(())
    }
}

fn owned<'a>(py: &'a PyToken<'_>, bits: u64) -> OwnedRuntimeValue<'a> {
    unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) }
}

fn call(py: &PyToken<'_>, raw: u64, name: &[u8], argument: Option<u64>) -> u64 {
    let Some(key) = attr_name_bits_from_bytes(py, name) else {
        return MoltObject::none().bits();
    };
    let _key = owned(py, key);
    let function = molt_getattr_builtin(raw, key, missing_bits(py));
    let _function = owned(py, function);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    unsafe {
        match argument {
            Some(argument) => call_callable1(py, function, argument),
            None => call_callable0(py, function),
        }
    }
}

fn closed(py: &PyToken<'_>, raw: u64) -> Result<bool, u64> {
    if let Some(pointer) = obj_from_bits(raw).as_ptr() {
        unsafe {
            if object_type_id(pointer) == TYPE_ID_FILE_HANDLE {
                let class = object_class_bits(pointer);
                let classes = builtin_classes(py);
                let handle = &*file_handle_ptr(pointer);
                if handle.state.is_some()
                    && [classes.file_io, classes.bytes_io].contains(&class)
                    && crate::object::class_is_immutable(py, obj_from_bits(class).as_ptr().unwrap())
                {
                    return Ok(file_handle_is_closed(handle));
                }
            }
        }
    }
    let Some(key) = attr_name_bits_from_bytes(py, b"closed") else {
        return Err(MoltObject::none().bits());
    };
    let _key = owned(py, key);
    let value = molt_getattr_builtin(raw, key, missing_bits(py));
    let _value = owned(py, value);
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    let result = is_truthy(py, obj_from_bits(value));
    if exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(result)
    }
}

/// The mutex protects the operation owner, not a borrowed Python payload. Never
/// hold it while calling Python or reacquiring the GIL after a blocking wait.
struct Operation(Option<Arc<BufferedWriteOperation>>);
impl Operation {
    unsafe fn enter(py: &PyToken<'_>, pointer: *mut u8, require_ready: bool) -> Result<Self, u64> {
        let current = crate::concurrency::current_thread_id();
        loop {
            if require_ready {
                unsafe { file_handle_require_attached(py, &*file_handle_ptr(pointer)) }?;
            }
            let Some(state) = (unsafe { (*file_handle_ptr(pointer)).write_operation.clone() })
            else {
                // A pre-lock conversion may have reinitialized the writer with an
                // exact physical raw stream, whose existing backend owns locking.
                return Ok(Self(None));
            };
            let mut owner = state
                .owner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if owner.thread == 0 {
                owner.thread = current;
                drop(owner);
                return Ok(Self(Some(state)));
            }
            if owner.thread == current {
                drop(owner);
                return Err(raise_exception(
                    py,
                    "RuntimeError",
                    "reentrant call inside buffered writer",
                ));
            }
            let suspended = crate::concurrency::gil::GilReleaseGuard::suspend();
            owner = state
                .ready
                .wait(owner)
                .unwrap_or_else(|error| error.into_inner());
            drop(owner);
            drop(suspended);
        }
    }
}
impl Operation {
    unsafe fn callback(py: &PyToken<'_>, pointer: *mut u8) -> Result<Self, u64> {
        unsafe {
            let handle = &mut *file_handle_ptr(pointer);
            let state = handle
                .write_operation
                .get_or_insert_with(|| {
                    Arc::new(BufferedWriteOperation {
                        owner: Mutex::new(Default::default()),
                        ready: std::sync::Condvar::new(),
                    })
                })
                .clone();
            if state
                .owner
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .thread
                == crate::concurrency::current_thread_id()
            {
                return Ok(Self(None));
            }
            Self::enter(py, pointer, true)
        }
    }
}

// tell() is deliberately callable during raw.write(). Preserve its logical
// pending count while PendingWrite owns the Vec outside the Python payload.
struct PendingObservation {
    operation: Arc<BufferedWriteOperation>,
    previous: Option<usize>,
}
impl PendingObservation {
    unsafe fn enter(pointer: *mut u8, count: usize) -> Self {
        let operation = unsafe {
            (*file_handle_ptr(pointer))
                .write_operation
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

impl Drop for Operation {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            state
                .owner
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .thread = 0;
            state.ready.notify_one();
        }
    }
}

unsafe fn raw_owner<'a>(
    py: &'a PyToken<'_>,
    pointer: *mut u8,
) -> Result<OwnedRuntimeValue<'a>, u64> {
    let (raw, physical, callback) = unsafe {
        let handle = &*file_handle_ptr(pointer);
        file_handle_require_attached(py, handle)?;
        (
            handle.buffer_bits,
            handle.state.is_some(),
            handle.write_operation.is_some(),
        )
    };
    if raw == 0 && physical && callback {
        let raw = unsafe { super::handle::file_handle_raw_bits(py, pointer) }
            .ok_or_else(|| MoltObject::none().bits())?;
        return Ok(owned(py, raw));
    }
    Ok(OwnedRuntimeValue::retain(
        py.core_token(),
        if raw == 0 && physical {
            MoltObject::from_ptr(pointer).bits()
        } else {
            raw
        },
    ))
}

unsafe fn checked_raw<'a>(
    py: &'a PyToken<'_>,
    pointer: *mut u8,
    message: &str,
) -> Result<OwnedRuntimeValue<'a>, u64> {
    let raw = unsafe { raw_owner(py, pointer) }?;
    if unsafe { (*file_handle_ptr(pointer)).closed } || closed(py, raw.bits())? {
        return Err(raise_exception(py, "ValueError", message));
    }
    // A descriptor getter may detach the wrapper while observing raw.closed.
    unsafe { file_handle_require_attached(py, &*file_handle_ptr(pointer)) }?;
    Ok(raw)
}

pub(super) fn new(py: &PyToken<'_>, class: u64) -> u64 {
    let pointer = alloc_file_handle_with_state(
        py,
        None,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        0,
        class,
        MoltObject::none().bits(),
        "wb".into(),
        None,
        None,
        None,
        None,
        0,
        0,
    );
    if pointer.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(pointer).bits()
    }
}

/// The existing writer initializer is the only producer of ready writer state.
/// Clinic conversion precedes self.ok=0; writable validation precedes raw owner
/// replacement and positive size validation. Old pending data is discarded on
/// reinitialization, never flushed. No Rust payload reference spans a callback.
pub(super) unsafe fn initialize(
    py: &PyToken<'_>,
    bits: u64,
    pointer: *mut u8,
    raw: u64,
    size_bits: u64,
) -> u64 {
    use crate::object::FileAttachment;
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let _raw = OwnedRuntimeValue::retain(py.core_token(), raw);
    let size = crate::builtins::numbers::index_ssize_from_obj(py, size_bits);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    if unsafe { applies(pointer) } {
        // Constructor callbacks may safely reinitialize before a raw span is
        // borrowed. Refuse an actually active write operation, but do not hold
        // its lock over writable(), old-owner retirement, or tell().
        match unsafe { Operation::enter(py, pointer, false) } {
            Ok(operation) => drop(operation),
            Err(error) => return error,
        }
    }
    unsafe {
        (*file_handle_ptr(pointer)).attachment = FileAttachment::Uninitialized;
    }
    let class = unsafe { object_class_bits(pointer) };
    let callback = needs_callback(py, class, raw);
    if callback {
        let writable = owned(py, call(py, raw, b"writable", None));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if writable.bits() != MoltObject::from_bool(true).bits() {
            return raise_exception(
                py,
                "UnsupportedOperation",
                "File or stream is not writable.",
            );
        }
    } else {
        let raw_pointer = obj_from_bits(raw).as_ptr().unwrap();
        let raw_handle = unsafe { &*file_handle_ptr(raw_pointer) };
        if let Err(error) = file_handle_require_attached(py, raw_handle) {
            return error;
        }
        if file_handle_is_closed(raw_handle) {
            return raise_exception(py, "ValueError", "I/O operation on closed file.");
        }
        if !raw_handle.writable {
            return raise_exception(
                py,
                "UnsupportedOperation",
                "File or stream is not writable.",
            );
        }
    }
    inc_ref_bits(py, raw);
    let old_raw = unsafe { std::mem::replace(&mut (*file_handle_ptr(pointer)).buffer_bits, raw) };
    dec_ref_bits(py, old_raw);
    unsafe {
        (*file_handle_ptr(pointer)).buffer_size = size;
    }
    if size <= 0 {
        return raise_exception(py, "ValueError", "buffer size must be strictly positive");
    }
    // One existing Vec owns buffered bytes in both raw dispatches. Callback
    // views borrow only while the local pending-buffer owner is alive.
    unsafe {
        (*file_handle_ptr(pointer)).write_buf = Vec::new();
        (*file_handle_ptr(pointer)).write_start = 0;
    }
    let mut write_buf = Vec::new();
    if callback && write_buf.try_reserve_exact(size as usize).is_err() {
        return raise_exception(py, "MemoryError", "cannot allocate buffered writer storage");
    }
    let operation = callback.then(|| {
        Arc::new(BufferedWriteOperation {
            owner: Mutex::new(Default::default()),
            ready: std::sync::Condvar::new(),
        })
    });
    let (state, mem_bits, closefd, owns_fd) = if callback {
        (None, 0, false, false)
    } else {
        let handle = unsafe { &*file_handle_ptr(obj_from_bits(raw).as_ptr().unwrap()) };
        (
            handle.state.clone(),
            handle.mem_bits,
            handle.closefd,
            handle.owns_fd,
        )
    };
    inc_ref_bits(py, mem_bits);
    let (old_mem, old_state) = unsafe {
        let handle = &mut *file_handle_ptr(pointer);
        let old_mem = std::mem::replace(&mut handle.mem_bits, mem_bits);
        let old_state = std::mem::replace(&mut handle.state, state);
        handle.write_operation = operation;
        handle.readable = false;
        handle.writable = true;
        handle.closed = false;
        handle.closefd = closefd;
        handle.owns_fd = owns_fd;
        handle.read_buf.clear();
        handle.read_pos = 0;
        handle.write_buf = write_buf;
        handle.write_start = 0;
        (old_mem, old_state)
    };
    dec_ref_bits(py, old_mem);
    drop(old_state);
    if callback {
        // _buffered_init probes tell, including its integer conversion, and
        // deliberately clears unsupported/invalid position errors.
        let position = owned(py, call(py, raw, b"tell", None));
        if !exception_pending(py) {
            let _ = off_t(py, position.bits());
        }
        if exception_pending(py) {
            clear_exception(py);
        }
    }
    unsafe {
        (*file_handle_ptr(pointer)).attachment = FileAttachment::Attached;
    }
    MoltObject::none().bits()
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
    let class = crate::builtins::exceptions::exception_type_bits_from_name(py, "BlockingIOError");
    let exception = crate::builtins::exceptions::molt_exception_new_from_class(class, _args.bits());
    let _exception = owned(py, exception);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    crate::molt_raise(exception)
}

fn off_t(py: &PyToken<'_>, value: u64) -> Option<i64> {
    if let Some(value) = crate::builtins::numbers::index_i64_integral_bits(value) {
        return Some(value);
    }
    let message = format!(
        "'{}' object cannot be interpreted as an integer",
        crate::type_name(py, obj_from_bits(value))
    );
    let value = crate::builtins::numbers::index_bigint_from_obj(py, value, &message)?;
    match value.to_i64() {
        Some(value) => Some(value),
        None => {
            raise_exception::<u64>(
                py,
                "ValueError",
                "cannot fit 'int' into an offset-sized integer",
            );
            None
        }
    }
}

fn pending_matches(py: &PyToken<'_>, name: &str) -> bool {
    crate::builtins::exceptions::exception_last_bits_noinc(py).is_some_and(|bits| {
        crate::builtins::exceptions::exception_matches_builtin_name(py, bits, name)
    })
}

/// Read the existing typed OSError errno field, never a user __getattribute__.
fn trap_eintr(py: &PyToken<'_>) -> bool {
    if !pending_matches(py, "OSError") {
        return false;
    }
    let Some(bits) = crate::builtins::exceptions::exception_last_bits_noinc(py) else {
        return false;
    };
    let Some(storage) = crate::builtins::exceptions::ExceptionStorage::for_exception(py, bits)
    else {
        return false;
    };
    let Some(errno) = storage.typed_field(py, molt_obj_model::ExceptionTypedField::OSErrorErrno)
    else {
        return false;
    };
    if to_i64(obj_from_bits(errno.bits())) != Some(libc::EINTR as i64) {
        return false;
    }
    clear_exception(py);
    true
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
                    "ValueError",
                    "I/O operation on closed file",
                ));
            }
            if !handle.writable {
                return Err(raise_exception(py, "UnsupportedOperation", "not writable"));
            }
            let state = file_handle_physical_state(py, handle)?.clone();
            let mem = memory_backend_bits(handle);
            let mut backend = state.backend.lock().unwrap();
            let Some(backend) = backend.as_mut() else {
                return Err(raise_exception(
                    py,
                    "ValueError",
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
                return Ok(RawWrite::Blocked(
                    error.raw_os_error().unwrap_or(libc::EAGAIN),
                ));
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
    let scope =
        unsafe { crate::object::buffer_exports::CallbackMemoryView::borrow(py, owner, data, len) }?;
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
            "OSError",
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
            "MemoryError",
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
        Err(error) if !pending_matches(py, "BlockingIOError") => return error,
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

pub(super) unsafe fn close(py: &PyToken<'_>, bits: u64, pointer: *mut u8) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    {
        let _operation = match unsafe { Operation::enter(py, pointer, true) } {
            Ok(owner) => owner,
            Err(error) => return error,
        };
        let raw = match unsafe { raw_owner(py, pointer) } {
            Ok(raw) => raw,
            Err(error) => return error,
        };
        match closed(py, raw.bits()) {
            Ok(true) => return MoltObject::none().bits(),
            Err(error) => return error,
            Ok(false) => {}
        }
    }
    let _scope = crate::builtins::exceptions::ExceptionStackScope::push(py);
    let result = io_call_noargs(py, bits, b"flush");
    let flush_error = crate::builtins::exceptions::take_raised(py);
    dec_ref_bits(py, result);
    let _operation = match unsafe { Operation::enter(py, pointer, true) } {
        Ok(owner) => owner,
        Err(error) => {
            io_chain_close_error(py, flush_error);
            return error;
        }
    };
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => {
            io_chain_close_error(py, flush_error);
            return error;
        }
    };
    let result = call(py, raw.bits(), b"close", None);
    unsafe {
        let handle = &mut *file_handle_ptr(pointer);
        handle.write_buf = Vec::new();
        handle.write_start = 0;
        handle.closed = true;
    }
    io_chain_close_error(py, flush_error);
    if exception_pending(py) {
        dec_ref_bits(py, result);
        MoltObject::none().bits()
    } else {
        result
    }
}

pub(super) unsafe fn detach(py: &PyToken<'_>, bits: u64, pointer: *mut u8) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    if let Err(error) = unsafe { raw_owner(py, pointer) } {
        return error;
    }
    let result = io_call_noargs(py, bits, b"flush");
    dec_ref_bits(py, result);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let handle = unsafe { &mut *file_handle_ptr(pointer) };
    if let Err(error) = file_handle_require_attached(py, handle) {
        return error;
    }
    handle.attachment = crate::object::FileAttachment::Detached;
    std::mem::replace(&mut handle.buffer_bits, MoltObject::none().bits())
}

pub(super) unsafe fn query(py: &PyToken<'_>, bits: u64, pointer: *mut u8, name: &[u8]) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let implicit = raw.bits() == bits && unsafe { (*file_handle_ptr(pointer)).buffer_bits == 0 };
    if name == b"writable"
        && (implicit
            || !raw_callbacks(py, builtin_classes(py).buffered_writer, raw.bits(), &[name]))
    {
        let handle = unsafe { &*file_handle_ptr(obj_from_bits(raw.bits()).as_ptr().unwrap()) };
        if let Err(error) = file_handle_require_attached(py, handle) {
            return error;
        }
        if file_handle_is_closed(handle) {
            return raise_exception(py, "ValueError", "I/O operation on closed file");
        }
        return MoltObject::from_bool(handle.writable).bits();
    }
    // Other raw observations reuse the existing lazy native projection; they
    // never reinterpret a BufferedWriter's own capabilities as its raw's.
    let raw = if implicit {
        match unsafe { super::handle::file_handle_raw_bits(py, pointer) } {
            Some(raw) => owned(py, raw),
            None => return MoltObject::none().bits(),
        }
    } else {
        raw
    };
    call(py, raw.bits(), name, None)
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
            "OSError",
            &format!("Raw stream returned invalid position {position}"),
        );
    }
    let (start, end, _) = unsafe { pending(pointer) };
    let count = unsafe { (*file_handle_ptr(pointer)).write_operation.as_ref() }
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
            "OverflowError",
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
            "ValueError",
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
            "UnsupportedOperation",
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
            "OSError",
            &format!("Raw stream returned invalid position {position}"),
        );
    }
    int_bits_from_i64(py, position)
}

pub(super) unsafe fn truncate(py: &PyToken<'_>, bits: u64, pointer: *mut u8, size: u64) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let raw = match unsafe { checked_raw(py, pointer, "truncate of closed file") } {
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
