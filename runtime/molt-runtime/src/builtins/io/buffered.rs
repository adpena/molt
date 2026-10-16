//! Shared buffered-stream lifecycle and callback operation ownership.
//! Exact native raw streams retain their direct backend path. Python values
//! live only in the existing traced handle edges and scoped runtime owners.
use super::*;
use crate::object::BufferedOperation;
use molt_runtime_core::OwnedRuntimeValue;
use num_traits::ToPrimitive;

pub(super) unsafe fn applies(pointer: *mut u8) -> bool {
    let handle = unsafe { file_handle_ptr(pointer) };
    !handle.is_null() && unsafe { (*handle).buffered_operation.is_some() }
}

// The first concrete buffered base in the real MRO owns the mode, including
// uninitialized and detached instances. An operation owner never implies mode.
unsafe fn buffered_class(py: &PyToken<'_>, pointer: *mut u8) -> Option<u64> {
    let classes = builtin_classes(py);
    let class = unsafe { object_class_bits(pointer) };
    let concrete = [
        classes.buffered_reader,
        classes.buffered_writer,
        classes.buffered_random,
    ];
    if concrete.contains(&class) {
        return Some(class);
    }
    let class = obj_from_bits(class).as_ptr()?;
    unsafe { crate::class_mro_view(py, class) }
        .iter()
        .copied()
        .find(|base| concrete.contains(base))
}

pub(super) unsafe fn is_reader(py: &PyToken<'_>, pointer: *mut u8) -> bool {
    (unsafe { buffered_class(py, pointer) }) == Some(builtin_classes(py).buffered_reader)
}

pub(super) unsafe fn is_writer(py: &PyToken<'_>, pointer: *mut u8) -> bool {
    (unsafe { buffered_class(py, pointer) }) == Some(builtin_classes(py).buffered_writer)
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
                crate::ExactStringLookup::Absent
            )
        })
    }
}

pub(super) fn raw_callbacks(py: &PyToken<'_>, class: u64, raw: u64, methods: &[&[u8]]) -> bool {
    let classes = builtin_classes(py);
    let Some(pointer) = obj_from_bits(raw).as_ptr() else {
        return true;
    };
    unsafe {
        if ![classes.buffered_reader, classes.buffered_writer].contains(&class)
            || object_type_id(pointer) != TYPE_ID_FILE_HANDLE
        {
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

/// Probe only this operation's raw methods. Native defaults require no owner
/// allocation; changed bindings promote the same handle without copying data.
pub(super) unsafe fn prepare(
    py: &PyToken<'_>,
    pointer: *mut u8,
    methods: &[&[u8]],
    self_flush: bool,
) -> Result<(), u64> {
    unsafe {
        let handle = &*file_handle_ptr(pointer);
        let class = object_class_bits(pointer);
        let classes = builtin_classes(py);
        if handle.buffered_operation.is_some()
            || ![classes.buffered_reader, classes.buffered_writer].contains(&class)
            || handle.attachment != crate::object::FileAttachment::Attached
        {
            return Ok(());
        }
        let own_override = self_flush && overrides(py, pointer, &[b"flush"]);
        if !own_override
            && (handle.buffer_bits == 0 || !raw_callbacks(py, class, handle.buffer_bits, methods))
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
        (*file_handle_ptr(pointer)).buffered_operation = Some(Arc::new(BufferedOperation {
            owner: Mutex::new(Default::default()),
            ready: std::sync::Condvar::new(),
        }));
        Ok(())
    }
}

/// Index conversion may replace raw methods. Promote before invoking it so the
/// existing operation owner protects the post-conversion callback path.
pub(super) unsafe fn prepare_conversion(py: &PyToken<'_>, pointer: *mut u8) -> Result<(), u64> {
    unsafe {
        if (!is_reader(py, pointer) && !is_writer(py, pointer)) || applies(pointer) {
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
        (*file_handle_ptr(pointer)).buffered_operation = Some(Arc::new(BufferedOperation {
            owner: Mutex::new(Default::default()),
            ready: std::sync::Condvar::new(),
        }));
        Ok(())
    }
}

pub(super) fn owned<'a>(py: &'a PyToken<'_>, bits: u64) -> OwnedRuntimeValue<'a> {
    unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) }
}

pub(super) fn call(py: &PyToken<'_>, raw: u64, name: &[u8], argument: Option<u64>) -> u64 {
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

pub(super) fn closed(py: &PyToken<'_>, raw: u64) -> Result<bool, u64> {
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
pub(super) struct Operation(Option<Arc<BufferedOperation>>);
impl Operation {
    pub(super) unsafe fn enter(
        py: &PyToken<'_>,
        pointer: *mut u8,
        require_ready: bool,
    ) -> Result<Self, u64> {
        let current = crate::concurrency::current_thread_id();
        loop {
            if require_ready {
                unsafe { file_handle_require_attached(py, &*file_handle_ptr(pointer)) }?;
            }
            let Some(state) = (unsafe { (*file_handle_ptr(pointer)).buffered_operation.clone() })
            else {
                // A pre-lock conversion may have reinitialized the stream with an
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
                    if unsafe { is_reader(py, pointer) } {
                        "reentrant call inside buffered reader"
                    } else {
                        "reentrant call inside buffered writer"
                    },
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
    pub(super) unsafe fn callback(py: &PyToken<'_>, pointer: *mut u8) -> Result<Self, u64> {
        unsafe {
            let handle = &mut *file_handle_ptr(pointer);
            let state = handle
                .buffered_operation
                .get_or_insert_with(|| {
                    Arc::new(BufferedOperation {
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

pub(super) unsafe fn raw_owner<'a>(
    py: &'a PyToken<'_>,
    pointer: *mut u8,
) -> Result<OwnedRuntimeValue<'a>, u64> {
    let (raw, physical, callback) = unsafe {
        let handle = &*file_handle_ptr(pointer);
        file_handle_require_attached(py, handle)?;
        (
            handle.buffer_bits,
            handle.state.is_some(),
            handle.buffered_operation.is_some(),
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

pub(super) unsafe fn checked_raw<'a>(
    py: &'a PyToken<'_>,
    pointer: *mut u8,
    message: &str,
) -> Result<OwnedRuntimeValue<'a>, u64> {
    let raw = unsafe { raw_owner(py, pointer) }?;
    let is_closed = unsafe { (*file_handle_ptr(pointer)).closed } || closed(py, raw.bits())?;
    // CPython's buffered closed check permits consuming existing read-ahead
    // after the raw stream closes. Refilling still observes the raw failure.
    let buffered_read =
        unsafe { is_reader(py, pointer) && unread_bytes(&*file_handle_ptr(pointer)) != 0 };
    if is_closed && !buffered_read {
        return Err(raise_exception(py, "ValueError", message));
    }
    // A descriptor getter may detach the wrapper while observing raw.closed.
    unsafe { file_handle_require_attached(py, &*file_handle_ptr(pointer)) }?;
    // It may also reinitialize the wrapper. Retain the current edge before
    // retiring the observed owner; do not invoke the descriptor a second time.
    let current = unsafe { raw_owner(py, pointer) }?;
    drop(raw);
    Ok(current)
}

pub(super) fn new(py: &PyToken<'_>, class: u64, readable: bool) -> u64 {
    let pointer = alloc_file_handle_with_state(
        py,
        None,
        readable,
        !readable,
        false,
        false,
        false,
        false,
        false,
        0,
        class,
        MoltObject::none().bits(),
        if readable { "rb" } else { "wb" }.into(),
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

/// The initializer is the only producer of ready reader/writer state.
/// Clinic conversion precedes self.ok=0; capability validation precedes raw owner
/// replacement and positive size validation. Old pending data is discarded on
/// reinitialization, never flushed. No Rust payload reference spans a callback.
pub(super) unsafe fn initialize(
    py: &PyToken<'_>,
    bits: u64,
    pointer: *mut u8,
    raw: u64,
    size_bits: u64,
    readable: bool,
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
        // borrowed. Refuse an actually active operation, but do not hold
        // its lock over capability lookup, old-owner retirement, or tell().
        match unsafe { Operation::enter(py, pointer, false) } {
            Ok(operation) => drop(operation),
            Err(error) => return error,
        }
    }
    unsafe {
        (*file_handle_ptr(pointer)).attachment = FileAttachment::Uninitialized;
    }
    let class = unsafe { object_class_bits(pointer) };
    let capability: &[u8] = if readable { b"readable" } else { b"writable" };
    let capability_error = if readable {
        "File or stream is not readable."
    } else {
        "File or stream is not writable."
    };
    let callback = raw_callbacks(py, class, raw, &[capability, b"tell"]);
    if callback {
        let supported = owned(py, call(py, raw, capability, None));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if supported.bits() != MoltObject::from_bool(true).bits() {
            return raise_exception(py, "UnsupportedOperation", capability_error);
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
        if !(if readable {
            raw_handle.readable
        } else {
            raw_handle.writable
        }) {
            return raise_exception(py, "UnsupportedOperation", capability_error);
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
        (*file_handle_ptr(pointer)).read_buf = Vec::new();
        (*file_handle_ptr(pointer)).read_pos = 0;
    }
    let mut buffer = Vec::new();
    if (readable || callback) && buffer.try_reserve_exact(size as usize).is_err() {
        return raise_exception(
            py,
            "MemoryError",
            if readable {
                "cannot allocate buffered reader storage"
            } else {
                "cannot allocate buffered writer storage"
            },
        );
    }
    let operation = callback.then(|| {
        Arc::new(BufferedOperation {
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
        handle.buffered_operation = operation;
        handle.readable = readable;
        handle.writable = !readable;
        handle.closed = false;
        handle.closefd = closefd;
        handle.owns_fd = owns_fd;
        handle.read_pos = 0;
        if readable {
            handle.read_buf = buffer;
        } else {
            handle.write_buf = buffer;
        }
        handle.write_start = 0;
        (old_mem, old_state)
    };
    dec_ref_bits(py, old_mem);
    drop(old_state);
    let initial_position = if callback {
        // _buffered_init probes tell, including its integer conversion, and
        // deliberately clears unsupported/invalid position errors.
        let position = owned(py, call(py, raw, b"tell", None));
        let position = (!exception_pending(py))
            .then(|| off_t(py, position.bits()))
            .flatten();
        if exception_pending(py) {
            clear_exception(py);
        }
        position.filter(|position| *position >= 0)
    } else if readable {
        // Exact native readers establish this fact once without Python lookup.
        // A nonseekable raw stream has no position cache.
        let state = unsafe {
            (*file_handle_ptr(obj_from_bits(raw).as_ptr().unwrap()))
                .state
                .clone()
        };
        let position = state.and_then(|state| {
            let mut backend = state.backend.lock().unwrap();
            match backend.as_mut() {
                Some(backend) => unsafe { backend_tell(py, &state, backend) }
                    .ok()
                    .and_then(|position| i64::try_from(position).ok()),
                None => None,
            }
        });
        if exception_pending(py) {
            clear_exception(py);
        }
        position
    } else {
        None
    };
    unsafe {
        // A tell callback can reinitialize the wrapper with another raw owner.
        if readable && (*file_handle_ptr(pointer)).buffer_bits == raw {
            let handle = &mut *file_handle_ptr(pointer);
            // The zero-initialized position survives a failed optional probe
            // in CPython; reinitialization likewise preserves its prior cache.
            handle.buffered_raw_position = initial_position
                .or(handle.buffered_raw_position)
                .or(Some(0));
        }
        (*file_handle_ptr(pointer)).attachment = FileAttachment::Attached;
    }
    MoltObject::none().bits()
}

pub(super) fn off_t(py: &PyToken<'_>, value: u64) -> Option<i64> {
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

pub(super) fn pending_matches(py: &PyToken<'_>, name: &str) -> bool {
    crate::builtins::exceptions::exception_last_bits_noinc(py).is_some_and(|bits| {
        crate::builtins::exceptions::exception_matches_builtin_name(py, bits, name)
    })
}

/// Read the existing typed OSError errno field, never a user __getattribute__.
pub(super) fn trap_eintr(py: &PyToken<'_>) -> bool {
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

pub(super) unsafe fn close(py: &PyToken<'_>, bits: u64, pointer: *mut u8) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    {
        let _operation = match unsafe { Operation::enter(py, pointer, true) } {
            Ok(owner) => owner,
            Err(error) => return error,
        };
        let mut raw = match unsafe { raw_owner(py, pointer) } {
            Ok(raw) => raw,
            Err(error) => return error,
        };
        if raw.bits() == bits && unsafe { (*file_handle_ptr(pointer)).buffer_bits == 0 } {
            raw = match unsafe { super::handle::file_handle_raw_bits(py, pointer) } {
                Some(raw) => owned(py, raw),
                None => return MoltObject::none().bits(),
            };
        }
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
        handle.read_buf = Vec::new();
        handle.read_pos = 0;
        handle.write_buf = Vec::new();
        handle.write_start = 0;
        handle.buffered_raw_position = None;
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
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    // Detach returns the real raw object, including an implicit native raw
    // that has not previously needed its public projection.
    if raw.bits() == bits && unsafe { (*file_handle_ptr(pointer)).buffer_bits == 0 } {
        let Some(raw) = (unsafe { super::handle::file_handle_raw_bits(py, pointer) }) else {
            return MoltObject::none().bits();
        };
        dec_ref_bits(py, raw);
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
    }
    drop(raw);
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
    handle.buffered_raw_position = None;
    std::mem::replace(&mut handle.buffer_bits, MoltObject::none().bits())
}

pub(super) unsafe fn query(py: &PyToken<'_>, bits: u64, pointer: *mut u8, name: &[u8]) -> u64 {
    let _owner = OwnedRuntimeValue::retain(py.core_token(), bits);
    let raw = match unsafe { raw_owner(py, pointer) } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let implicit = raw.bits() == bits && unsafe { (*file_handle_ptr(pointer)).buffer_bits == 0 };
    if [b"readable".as_slice(), b"writable"].contains(&name)
        && (implicit
            || !raw_callbacks(
                py,
                unsafe { object_class_bits(pointer) },
                raw.bits(),
                &[name],
            ))
    {
        let handle = unsafe { &*file_handle_ptr(obj_from_bits(raw.bits()).as_ptr().unwrap()) };
        if let Err(error) = file_handle_require_attached(py, handle) {
            return error;
        }
        if file_handle_is_closed(handle) {
            return raise_exception(py, "ValueError", "I/O operation on closed file");
        }
        return MoltObject::from_bool(if name == b"readable" {
            handle.readable
        } else {
            handle.writable
        })
        .bits();
    }
    // Other raw observations reuse the existing lazy native projection; they
    // never reinterpret the buffered wrapper's own capabilities as its raw's.
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
