use super::*;
use crate::runtime_flags::{self, RuntimeFlag};

struct FileMode {
    options: OpenOptions,
    readable: bool,
    writable: bool,
    append: bool,
    create: bool,
    truncate: bool,
    create_new: bool,
    text: bool,
}

fn parse_file_mode(mode: &str) -> Result<FileMode, String> {
    let mut kind: Option<char> = None;
    let mut kind_dup = false;
    let mut read = false;
    let mut write = false;
    let mut append = false;
    let mut truncate = false;
    let mut create = false;
    let mut create_new = false;
    let mut saw_plus = 0usize;
    let mut saw_text = false;
    let mut saw_binary = false;

    for ch in mode.chars() {
        match ch {
            'r' | 'w' | 'a' | 'x' => {
                if let Some(prev) = kind {
                    if prev == ch {
                        kind_dup = true;
                    } else {
                        return Err(
                            "must have exactly one of create/read/write/append mode".to_string()
                        );
                    }
                } else {
                    kind = Some(ch);
                }
                match ch {
                    'r' => read = true,
                    'w' => {
                        write = true;
                        truncate = true;
                        create = true;
                    }
                    'a' => {
                        write = true;
                        append = true;
                        create = true;
                    }
                    'x' => {
                        write = true;
                        create = true;
                        create_new = true;
                    }
                    _ => {}
                }
            }
            '+' => {
                saw_plus += 1;
                read = true;
                write = true;
            }
            'b' => saw_binary = true,
            't' => saw_text = true,
            _ => return Err(format!("invalid mode: '{mode}'")),
        }
    }

    if saw_binary && saw_text {
        return Err("can't have text and binary mode at once".to_string());
    }
    if saw_plus > 1 {
        return Err(format!("invalid mode: '{mode}'"));
    }
    if kind.is_none() {
        return Err(
            "Must have exactly one of create/read/write/append mode and at most one plus"
                .to_string(),
        );
    }
    if kind_dup {
        return Err(format!("invalid mode: '{mode}'"));
    }

    let mut options = OpenOptions::new();
    options
        .read(read)
        .write(write)
        .append(append)
        .truncate(truncate)
        .create(create);
    if create_new {
        options.create_new(true);
    }
    Ok(FileMode {
        options,
        readable: read,
        writable: write,
        append,
        create,
        truncate,
        create_new,
        text: !saw_binary,
    })
}

fn open_arg_type(_py: &PyToken<'_>, bits: u64, name: &str, allow_none: bool) -> Option<String> {
    let obj = obj_from_bits(bits);
    if allow_none && obj.is_none() {
        return None;
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Some(text);
    }
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = if allow_none {
        format!("open() argument '{name}' must be str or None, not {type_name}")
    } else {
        format!("open() argument '{name}' must be str, not {type_name}")
    };
    raise_exception::<_>(_py, "TypeError", &msg)
}

pub(super) fn open_arg_newline(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("open() argument 'newline' must be str or None, not {type_name}");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    match text.as_str() {
        "" | "\n" | "\r" | "\r\n" => Some(text),
        _ => {
            let msg = format!("illegal newline value: {text}");
            raise_exception::<_>(_py, "ValueError", &msg)
        }
    }
}

pub(super) fn reconfigure_arg_type(_py: &PyToken<'_>, bits: u64, name: &str) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Some(text);
    }
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("reconfigure() argument '{name}' must be str or None, not {type_name}");
    raise_exception::<_>(_py, "TypeError", &msg)
}

pub(super) fn reconfigure_arg_newline(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("reconfigure() argument 'newline' must be str or None, not {type_name}");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    match text.as_str() {
        "" | "\n" | "\r" | "\r\n" => Some(text),
        _ => {
            let msg = format!("illegal newline value: {text}");
            raise_exception::<_>(_py, "ValueError", &msg)
        }
    }
}

fn open_arg_encoding(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    open_arg_type(_py, bits, "encoding", true)
}

fn open_arg_errors(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    open_arg_type(_py, bits, "errors", true)
}

fn file_mode_to_flags(mode: &FileMode) -> i32 {
    let mut flags = 0;
    if mode.readable && !mode.writable {
        flags |= libc::O_RDONLY;
    } else if mode.writable && !mode.readable {
        flags |= libc::O_WRONLY;
    } else {
        flags |= libc::O_RDWR;
    }
    if mode.append {
        flags |= libc::O_APPEND;
    }
    if mode.create {
        flags |= libc::O_CREAT;
    }
    if mode.truncate {
        flags |= libc::O_TRUNC;
    }
    if mode.create_new {
        flags |= libc::O_EXCL;
    }
    flags
}

#[cfg(unix)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    let fd = libc::c_int::try_from(fd).ok()?;
    if fd < 0 {
        return None;
    }
    // `File::from_raw_fd` will happily wrap an invalid fd; validate upfront so
    // `open(fd)` matches CPython and raises immediately for EBADF.
    let rc = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFD) };
    if rc < 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(windows)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let fd = libc::c_int::try_from(fd).ok()?;
    let handle = unsafe { libc::get_osfhandle(fd) };
    if handle == -1 {
        return None;
    }
    let dup = duplicate_handle(handle as *mut std::ffi::c_void)?;
    Some(unsafe { std::fs::File::from_raw_handle(dup as *mut _) })
}

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::wasi::io::FromRawFd;
    let fd = std::os::wasi::io::RawFd::try_from(fd).ok()?;
    Some(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(all(
    not(any(unix, windows)),
    not(all(target_arch = "wasm32", target_os = "wasi"))
))]
fn file_from_fd(_fd: i64) -> Option<std::fs::File> {
    None
}

#[cfg(unix)]
pub(crate) fn dup_fd(fd: i64) -> Option<i64> {
    let fd = libc::c_int::try_from(fd).ok()?;
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd) };
    if duped < 0 { None } else { Some(duped as i64) }
}

#[cfg(windows)]
pub(crate) fn dup_fd(fd: i64) -> Option<i64> {
    let fd = libc::c_int::try_from(fd).ok()?;
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd) };
    if duped < 0 { None } else { Some(duped as i64) }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn dup_fd(_fd: i64) -> Option<i64> {
    None
}

#[cfg(windows)]
pub(super) fn windows_handle_isatty(handle: *mut std::ffi::c_void) -> bool {
    if handle.is_null() || handle as isize == -1 {
        return false;
    }
    unsafe {
        let file_type = GetFileType(handle);
        if file_type != FILE_TYPE_CHAR {
            return false;
        }
        let mut mode: u32 = 0;
        GetConsoleMode(handle, &mut mode as *mut u32) != 0
    }
}

#[cfg(windows)]
fn duplicate_handle(handle: *mut std::ffi::c_void) -> Option<*mut std::ffi::c_void> {
    if handle.is_null() || handle as isize == -1 {
        return None;
    }
    unsafe {
        let process = GetCurrentProcess();
        let mut dup: *mut std::ffi::c_void = std::ptr::null_mut();
        let ok = DuplicateHandle(
            process,
            handle,
            process,
            &mut dup as *mut *mut std::ffi::c_void,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        );
        if ok == 0 { None } else { Some(dup) }
    }
}

#[cfg(windows)]
pub(crate) fn windows_path_from_handle(handle: *mut std::ffi::c_void) -> Option<String> {
    if handle.is_null() || handle as isize == -1 {
        return None;
    }
    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
    let needed = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, flags) };
    if needed == 0 {
        return None;
    }
    let mut buf: Vec<u16> = vec![0u16; needed as usize + 1];
    let wrote =
        unsafe { GetFinalPathNameByHandleW(handle, buf.as_mut_ptr(), buf.len() as u32, flags) };
    if wrote == 0 {
        return None;
    }
    let mut text = String::from_utf16_lossy(&buf[..wrote as usize]);
    if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        text = format!("\\\\{rest}");
    } else if let Some(rest) = text.strip_prefix("\\\\?\\") {
        text = rest.to_string();
    }
    Some(text)
}

#[cfg(windows)]
pub(crate) fn windows_crt_fd_from_handle(
    handle: *mut std::ffi::c_void,
    readable: bool,
    writable: bool,
    text: bool,
) -> Option<i64> {
    let dup = duplicate_handle(handle)?;
    let mut flags = if text { libc::O_TEXT } else { libc::O_BINARY };
    if readable && writable {
        flags |= libc::O_RDWR;
    } else if readable {
        flags |= libc::O_RDONLY;
    } else {
        flags |= libc::O_WRONLY;
    }
    let fd = unsafe { libc::open_osfhandle(dup as isize, flags) };
    if fd < 0 {
        unsafe {
            CloseHandle(dup);
        }
        None
    } else {
        Some(fd as i64)
    }
}

fn stdio_isatty(fd: i64) -> bool {
    #[cfg(unix)]
    {
        if fd < 0 {
            return false;
        }
        unsafe { libc::isatty(fd as libc::c_int) == 1 }
    }
    #[cfg(windows)]
    {
        if fd < 0 {
            return false;
        }
        unsafe { libc::isatty(fd as libc::c_int) == 1 }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fd;
        false
    }
}

// open() exposes its fspath result; FileIO exposes the original argument.
// CPython _iomodule.c resolves before construction, while fileio.c retains nameobj.
#[derive(Clone, Copy)]
enum OpenFileName {
    Resolved,
    Original,
}

#[allow(clippy::too_many_arguments)]
fn open_impl(
    _py: &PyToken<'_>,
    file_bits: u64,
    mode_bits: u64,
    buffering_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
    name_policy: OpenFileName,
    file_io_class: Option<u64>,
) -> u64 {
    struct BitsGuard<'a> {
        py: &'a PyToken<'a>,
        bits: u64,
    }
    impl<'a> Drop for BitsGuard<'a> {
        fn drop(&mut self) {
            if self.bits != 0 {
                dec_ref_bits(self.py, self.bits);
            }
        }
    }

    let debug_open_fd = runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_OPEN_FD);

    let mode_obj = obj_from_bits(mode_bits);
    if mode_obj.is_none() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "open() argument 'mode' must be str, not NoneType",
        );
    }
    let mode = match string_obj_to_owned(mode_obj) {
        Some(mode) => mode,
        None => {
            let type_name = class_name_for_error(type_of_bits(_py, mode_bits));
            let msg = format!("open() argument 'mode' must be str, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    };
    let mode_info = match parse_file_mode(&mode) {
        Ok(parsed) => parsed,
        Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
    };
    let operation = if mode_info.readable && mode_info.writable {
        crate::OperationId::IoOpenReadwrite
    } else if mode_info.readable {
        crate::OperationId::IoOpenRead
    } else {
        crate::OperationId::IoOpenWrite
    };
    let buffering = {
        let obj = obj_from_bits(buffering_bits);
        if obj.is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "'NoneType' object cannot be interpreted as an integer",
            );
        }
        let type_name = class_name_for_error(type_of_bits(_py, buffering_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        index_i64_from_obj(_py, buffering_bits, &msg)
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    if buffering < -1 {
        return raise_exception::<_>(_py, "ValueError", "buffering must be >= -1");
    }
    let buffering = if buffering < 0 { -1 } else { buffering };
    let line_buffering = buffering == 1 && mode_info.text;
    if buffering == 0 && mode_info.text {
        return raise_exception::<_>(_py, "ValueError", "can't have unbuffered text I/O");
    }

    let encoding = if mode_info.text {
        open_arg_encoding(_py, encoding_bits)
    } else if !obj_from_bits(encoding_bits).is_none() {
        return raise_exception::<_>(
            _py,
            "ValueError",
            "binary mode doesn't take an encoding argument",
        );
    } else {
        None
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let errors = if mode_info.text {
        open_arg_errors(_py, errors_bits)
    } else if !obj_from_bits(errors_bits).is_none() {
        return raise_exception::<_>(
            _py,
            "ValueError",
            "binary mode doesn't take an errors argument",
        );
    } else {
        None
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let newline = if mode_info.text {
        open_arg_newline(_py, newline_bits)
    } else if !obj_from_bits(newline_bits).is_none() {
        return raise_exception::<_>(
            _py,
            "ValueError",
            "binary mode doesn't take a newline argument",
        );
    } else {
        None
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }

    let closefd = is_truthy(_py, obj_from_bits(closefd_bits));
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let opener_obj = obj_from_bits(opener_bits);
    let opener_is_none = opener_obj.is_none();

    let mut path_guard = BitsGuard { py: _py, bits: 0 };
    let mut path = None;
    let mut fd: Option<i64> = None;
    let mut debug_fd_value: Option<i64> = None;
    let path_name_bits = if let Some(i) =
        crate::builtins::numbers::index_i64_integral_bits(file_bits)
            .and_then(|value| libc::c_int::try_from(value).ok())
    {
        // FileIO retries path admission after failed direct C-int conversion;
        // opener results instead retain their checked-conversion exception.
        let i = i64::from(i);
        if i < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative file descriptor");
        }
        fd = Some(i);
        if debug_open_fd {
            debug_fd_value = Some(i);
        }
        let bits = MoltObject::from_int(i).bits();
        path_guard.bits = bits;
        bits
    } else {
        match path_and_name_from_bits(_py, file_bits) {
            Ok((resolved, name_bits, _flavor)) => {
                let name_bits = match name_policy {
                    OpenFileName::Resolved => name_bits,
                    OpenFileName::Original => {
                        inc_ref_bits(_py, file_bits);
                        dec_ref_bits(_py, name_bits);
                        file_bits
                    }
                };
                path_guard.bits = name_bits;
                if !closefd {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "Cannot use closefd=False with file name",
                    );
                }
                path = Some(resolved);
                name_bits
            }
            Err(error) => return error,
        }
    };

    // Normalize arguments before selecting one route. No descriptor, opener,
    // host filesystem, or bundle payload access occurs before this gate.
    let vfs_route = if opener_is_none {
        match path
            .as_ref()
            .map(|path| super::selected_vfs_route(_py, path))
            .transpose()
        {
            Ok(route) => route.flatten(),
            Err(error) => return error,
        }
    } else {
        None
    };
    let mount_requirements = match vfs_route.as_ref() {
        Some((prefix, _, _)) => match crate::vfs::caps::mount_requirements(
            prefix,
            mode_info.readable,
            mode_info.writable,
        ) {
            Ok(requirements) => Some(requirements),
            Err(error) => return raise_exception::<_>(_py, "PermissionError", &error.to_string()),
        },
        None => None,
    };
    let extras = mount_requirements
        .as_ref()
        .map_or(&[][..], |requirements| requirements.capabilities());
    if let Err(error) = crate::require_operation_with_capabilities(
        _py,
        operation,
        crate::audit::AuditArgs::None,
        extras,
    ) {
        return error;
    }
    if mount_requirements
        .as_ref()
        .is_some_and(|requirements| requirements.read_only)
    {
        return raise_exception::<_>(_py, "PermissionError", "Read-only file system");
    }

    let mut file = None;
    #[cfg(windows)]
    let mut crt_fd: Option<i64> = None;
    if let Some(fd_val) = fd {
        if !opener_is_none {
            return raise_exception::<_>(_py, "ValueError", "opener only works with file path");
        }
        let effective_fd = if closefd {
            fd_val
        } else {
            match dup_fd(fd_val) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(_py, "OSError", "open failed");
                }
            }
        };
        if let Some(handle) = file_from_fd(effective_fd) {
            file = Some(handle);
            #[cfg(windows)]
            {
                crt_fd = Some(effective_fd);
            }
        } else {
            #[cfg(windows)]
            unsafe {
                libc::close(effective_fd as libc::c_int);
            }
            return raise_exception::<_>(_py, "OSError", "open failed");
        }
    } else if let Some(path) = path {
        let flags = file_mode_to_flags(&mode_info);
        if !opener_is_none {
            if !is_truthy(_py, obj_from_bits(molt_is_callable(opener_bits))) {
                let type_name = class_name_for_error(type_of_bits(_py, opener_bits));
                let msg = format!("'{type_name}' object is not callable");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let path_bits = path_name_bits;
            let flags_bits = MoltObject::from_int(flags as i64).bits();
            let fd_bits = unsafe { call_callable2(_py, opener_bits, path_bits, flags_bits) };
            let failed = exception_pending(_py);
            let _fd_owner = unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(_py.core_token(), fd_bits)
            };
            if failed {
                return MoltObject::none().bits();
            }
            if crate::builtins::numbers::index_integral_payload_bits(fd_bits).is_some() {
                let Some(fd_val) = crate::builtins::numbers::index_c_int_from_obj(_py, fd_bits)
                else {
                    return MoltObject::none().bits();
                };
                let fd_val = i64::from(fd_val);
                if fd_val < 0 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "opener returned a negative file descriptor",
                    );
                }
                if let Some(handle) = file_from_fd(fd_val) {
                    file = Some(handle);
                    #[cfg(windows)]
                    {
                        crt_fd = Some(fd_val);
                    }
                } else {
                    #[cfg(windows)]
                    unsafe {
                        libc::close(fd_val as libc::c_int);
                    }
                    return raise_exception::<_>(_py, "OSError", "open failed");
                }
            } else {
                let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
                let msg = format!("expected opener to return int, got {type_name}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        } else {
            if let Some((_mount_prefix, backend, rel_path)) = vfs_route {
                let is_write = mode_info.writable;
                let path_str = path.to_string_lossy();
                // Selected mode, not writability, determines initial contents.
                // Missing files are creatable only for w/a/x; r+ retains bytes.
                let initial_data = if mode_info.truncate || mode_info.create_new {
                    match backend.stat(&rel_path) {
                        Ok(_) if mode_info.create_new => Err(crate::vfs::VfsError::AlreadyExists),
                        Ok(stat) if stat.is_dir => Err(crate::vfs::VfsError::IsDirectory),
                        Ok(_) => Ok(Vec::new()),
                        Err(crate::vfs::VfsError::NotFound) => Ok(Vec::new()),
                        Err(error) => Err(error),
                    }
                } else if mode_info.append && !mode_info.readable {
                    backend.append_seed(&rel_path)
                } else {
                    match backend.open_read(&rel_path) {
                        Err(crate::vfs::VfsError::NotFound) if mode_info.create => Ok(Vec::new()),
                        result => result,
                    }
                };
                let data = match initial_data {
                    Ok(bytes) => bytes,
                    Err(error) => return raise_vfs_error(_py, error, &path_str),
                };

                // Retain the selected backend for writable-handle retirement.
                let vfs_backend_arc = if is_write {
                    Some((Arc::clone(&backend), rel_path.clone()))
                } else {
                    None
                };

                // Build an in-memory file handle (like BytesIO) backed
                // by the VFS data so the rest of the runtime sees a
                // normal file object.
                let initial_pos = if mode_info.append { data.len() } else { 0 };
                let bytearray_ptr = alloc_bytearray(_py, &data);
                if bytearray_ptr.is_null() {
                    return raise_exception::<_>(_py, "OSError", "open failed");
                }
                let mem_bits = MoltObject::from_ptr(bytearray_ptr).bits();
                let vfs_state = Arc::new(MoltFileState {
                    backend: Mutex::new(Some(MoltFileBackend::Memory(MoltMemoryBackend {
                        pos: initial_pos,
                    }))),
                    #[cfg(windows)]
                    crt_fd: Mutex::new(None),
                    host_descriptor: false,
                });

                // Reuse the same encoding / errors / newline resolution
                // that the normal path uses.
                let (enc, enc_original) = if mode_info.text {
                    if !warn_default_text_encoding(_py, encoding.as_deref()) {
                        dec_ref_bits(_py, mem_bits);
                        return MoltObject::none().bits();
                    }
                    let (label, display) = match selected_text_encoding(_py, encoding.as_deref()) {
                        Ok(val) => val,
                        Err(msg) => {
                            dec_ref_bits(_py, mem_bits);
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                    };
                    (Some(label), Some(display))
                } else {
                    (None, None)
                };
                let errs = if mode_info.text {
                    Some(errors.unwrap_or_else(|| "strict".to_string()))
                } else {
                    None
                };

                let vfs_readable = mode_info.readable;
                let vfs_writable = is_write;

                let builtins = builtin_classes(_py);
                let buffered_class_bits = if vfs_readable && vfs_writable {
                    builtins.buffered_random
                } else if vfs_writable {
                    builtins.buffered_writer
                } else {
                    builtins.buffered_reader
                };
                let binary_class_bits = if buffering == 0 {
                    file_io_class.unwrap_or(builtins.file_io)
                } else {
                    buffered_class_bits
                };
                let handle_class_bits = if mode_info.text {
                    builtins.text_io_wrapper
                } else {
                    binary_class_bits
                };
                let buffer_class_bits = if mode_info.text {
                    buffered_class_bits
                } else {
                    0
                };
                let buf_size = if buffering == 0 {
                    0
                } else if line_buffering || buffering < 0 {
                    default_buffer_size(_py)
                } else {
                    buffering
                };
                let buffer_bits = if mode_info.text {
                    let buffer_ptr = alloc_file_handle_with_state(
                        _py,
                        Some(Arc::clone(&vfs_state)),
                        vfs_readable,
                        vfs_writable,
                        false, // text
                        closefd,
                        true,  // owns_fd
                        false, // line_buffering
                        false, // write_through
                        buf_size,
                        buffer_class_bits,
                        path_name_bits,
                        mode.clone(),
                        None,
                        None,
                        None,
                        None,
                        0,
                        mem_bits,
                    );
                    if buffer_ptr.is_null() {
                        dec_ref_bits(_py, mem_bits);
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(buffer_ptr).bits()
                } else {
                    0
                };
                let ptr = alloc_file_handle_with_state(
                    _py,
                    Some(vfs_state),
                    vfs_readable,
                    vfs_writable,
                    mode_info.text,
                    true, // closefd
                    true, // owns_fd
                    line_buffering,
                    false, // write_through
                    buf_size,
                    handle_class_bits,
                    path_name_bits,
                    mode.clone(),
                    enc,
                    enc_original,
                    errs,
                    newline,
                    buffer_bits,
                    if mode_info.text { 0 } else { mem_bits },
                );
                // Publish writeback only after every fallible construction
                // step. Refused codecs/warnings/allocations own no map entry.
                if !ptr.is_null()
                    && let Some(entry) = vfs_backend_arc
                {
                    unsafe {
                        vfs_writeback_register(
                            _py,
                            (*file_handle_ptr(ptr))
                                .state
                                .as_ref()
                                .expect("new physical stream owns its state"),
                            entry,
                        );
                    }
                }
                dec_ref_bits(_py, mem_bits);
                if buffer_bits != 0 {
                    dec_ref_bits(_py, buffer_bits);
                }
                return if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                };
            }
            // End VFS dispatch
            file = match mode_info.options.open(&path) {
                Ok(file) => Some(file),
                Err(err) => {
                    let short = match err.kind() {
                        ErrorKind::NotFound => "No such file or directory".to_string(),
                        ErrorKind::PermissionDenied => "Permission denied".to_string(),
                        ErrorKind::AlreadyExists => "File exists".to_string(),
                        ErrorKind::InvalidInput => "Invalid argument".to_string(),
                        ErrorKind::IsADirectory => "Is a directory".to_string(),
                        ErrorKind::NotADirectory => "Not a directory".to_string(),
                        _ => err.to_string(),
                    };
                    let path_display = path.to_string_lossy();
                    let raw_code = err.raw_os_error();
                    let msg = if let Some(code) = raw_code {
                        format!("[Errno {code}] {short}: '{path_display}'")
                    } else {
                        format!("{short}: '{path_display}'")
                    };
                    if let Some(code) = raw_code {
                        return raise_os_error_errno::<_>(_py, code as i64, &msg);
                    }
                    match err.kind() {
                        ErrorKind::AlreadyExists => {
                            return raise_exception::<_>(_py, "FileExistsError", &msg);
                        }
                        ErrorKind::NotFound => {
                            return raise_exception::<_>(_py, "FileNotFoundError", &msg);
                        }
                        ErrorKind::PermissionDenied => {
                            return raise_exception::<_>(_py, "PermissionError", &msg);
                        }
                        ErrorKind::IsADirectory => {
                            return raise_exception::<_>(_py, "IsADirectoryError", &msg);
                        }
                        ErrorKind::NotADirectory => {
                            return raise_exception::<_>(_py, "NotADirectoryError", &msg);
                        }
                        _ => return raise_exception::<_>(_py, "OSError", &msg),
                    }
                }
            };
        }
    }
    #[cfg(windows)]
    if crt_fd.is_none()
        && let Some(file_ref) = file.as_ref()
    {
        use std::os::windows::io::AsRawHandle;
        let handle = file_ref.as_raw_handle();
        crt_fd = windows_crt_fd_from_handle(handle, mode_info.readable, mode_info.writable, false);
        if crt_fd.is_none() {
            return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "open");
        }
    }
    let Some(file) = file else {
        return raise_exception::<_>(_py, "OSError", "open failed");
    };

    let state = Arc::new(MoltFileState {
        backend: Mutex::new(Some(MoltFileBackend::File(file))),
        #[cfg(windows)]
        crt_fd: Mutex::new(crt_fd),
        host_descriptor: false,
    });

    if mode_info.append {
        // O_APPEND only controls writes. CPython also positions every append
        // constructor (path, opener, or supplied fd) at EOF before publication.
        let seek_error = {
            let mut guard = state.backend.lock().unwrap();
            let Some(MoltFileBackend::File(file)) = guard.as_mut() else {
                unreachable!()
            };
            host_file_seek(&state, file, std::io::SeekFrom::End(0)).err()
        };
        #[cfg(windows)]
        let nonseekable = seek_error
            .as_ref()
            .is_some_and(|error| error.raw_os_error() == Some(libc::ESPIPE));
        #[cfg(not(windows))]
        let nonseekable = seek_error
            .as_ref()
            .is_some_and(|error| error.kind() == ErrorKind::NotSeekable);
        if let Some(error) = seek_error.filter(|_| !nonseekable) {
            // A failed FileIO constructor never takes a caller-supplied fd,
            // even with closefd=True. Private duplicates/path/opener fds still
            // retire through the existing state owner on this error path.
            if fd.is_some() && closefd {
                #[cfg(windows)]
                {
                    state.crt_fd.lock().unwrap().take();
                }
                #[cfg(not(windows))]
                if let Some(MoltFileBackend::File(file)) = state.backend.lock().unwrap().take() {
                    std::mem::forget(file);
                }
            }
            return match error.raw_os_error() {
                Some(errno) => raise_os_error_errno::<u64>(_py, i64::from(errno), "open"),
                None => raise_os_error::<u64>(_py, error, "open"),
            };
        }
    }

    // Keep text-I/O encoding normalization explicit so open()/TextIOWrapper
    // remains deterministic across native and wasm builds.
    let (encoding, encoding_original) = if mode_info.text {
        if !warn_default_text_encoding(_py, encoding.as_deref()) {
            return MoltObject::none().bits();
        }
        let (label, display) = match selected_text_encoding(_py, encoding.as_deref()) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
        };
        (Some(label), Some(display))
    } else {
        (None, None)
    };
    let errors = if mode_info.text {
        Some(errors.unwrap_or_else(|| "strict".to_string()))
    } else {
        None
    };

    let builtins = builtin_classes(_py);
    let buffered_class_bits = if mode_info.readable && mode_info.writable {
        builtins.buffered_random
    } else if mode_info.writable {
        builtins.buffered_writer
    } else {
        builtins.buffered_reader
    };
    let binary_class_bits = if buffering == 0 {
        file_io_class.unwrap_or(builtins.file_io)
    } else {
        buffered_class_bits
    };
    let handle_class_bits = if mode_info.text {
        builtins.text_io_wrapper
    } else {
        binary_class_bits
    };
    let buffer_class_bits = if mode_info.text {
        buffered_class_bits
    } else {
        0
    };
    let buffer_size = if buffering == 0 {
        0
    } else if line_buffering || buffering < 0 {
        default_buffer_size(_py)
    } else {
        buffering
    };
    let buffer_bits = if mode_info.text {
        let buffer_ptr = alloc_file_handle_with_state(
            _py,
            Some(Arc::clone(&state)),
            mode_info.readable,
            mode_info.writable,
            false,
            closefd,
            true,
            false,
            false,
            buffer_size,
            buffer_class_bits,
            path_name_bits,
            mode.clone(),
            None,
            None,
            None,
            None,
            0,
            0,
        );
        if buffer_ptr.is_null() {
            return MoltObject::none().bits();
        }
        if debug_fd_value == Some(0) && debug_open_fd {
            eprintln!(
                "molt open(fd=0) buffer_handle_ptr=0x{:x}",
                buffer_ptr as usize
            );
        }
        MoltObject::from_ptr(buffer_ptr).bits()
    } else {
        0
    };
    let ptr = alloc_file_handle_with_state(
        _py,
        Some(state),
        mode_info.readable,
        mode_info.writable,
        mode_info.text,
        closefd,
        true,
        line_buffering,
        false,
        buffer_size,
        handle_class_bits,
        path_name_bits,
        mode,
        encoding,
        encoding_original,
        errors,
        newline,
        buffer_bits,
        0,
    );
    if debug_fd_value == Some(0) && debug_open_fd && !ptr.is_null() {
        eprintln!(
            "molt open(fd=0) -> file_handle_ptr=0x{:x} closefd={}",
            ptr as usize, closefd
        );
    }
    if buffer_bits != 0 {
        dec_ref_bits(_py, buffer_bits);
    }
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn alloc_stdio_handle(
    _py: &PyToken<'_>,
    fd: i64,
    readable: bool,
    writable: bool,
    name: &str,
    errors: &str,
    write_through: bool,
) -> u64 {
    let trace_stdio = runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_STDIO_BUILD);
    // Native hosts give each stream a private duplicate that the handle owns
    // and closes. WASI has no `dup`: the stream borrows the host descriptor and
    // must never close it, or host output after runtime teardown is lost.
    let borrows_host_fd = cfg!(target_arch = "wasm32");
    let effective_fd = if borrows_host_fd {
        fd
    } else {
        match dup_fd(fd) {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        }
    };
    let Some(file) = file_from_fd(effective_fd) else {
        #[cfg(windows)]
        unsafe {
            libc::close(effective_fd as libc::c_int);
        }
        return MoltObject::none().bits();
    };
    let mode = if readable && writable {
        "r+"
    } else if readable {
        "r"
    } else {
        "w"
    };
    let mode_info = match parse_file_mode(mode) {
        Ok(parsed) => parsed,
        Err(_) => return MoltObject::none().bits(),
    };
    let buffering = -1;
    let line_buffering = if writable { stdio_isatty(fd) } else { false };
    let buffer_size = if buffering == 0 {
        0
    } else if line_buffering || buffering < 0 {
        default_buffer_size(_py)
    } else {
        buffering
    };

    let state = Arc::new(MoltFileState {
        backend: Mutex::new(Some(MoltFileBackend::File(file))),
        #[cfg(windows)]
        crt_fd: Mutex::new(Some(effective_fd)),
        host_descriptor: borrows_host_fd,
    });
    let builtins = builtin_classes(_py);
    let buffered_class_bits = if mode_info.readable && mode_info.writable {
        builtins.buffered_random
    } else if mode_info.writable {
        builtins.buffered_writer
    } else {
        builtins.buffered_reader
    };
    let binary_class_bits = if buffering == 0 {
        builtins.file_io
    } else {
        buffered_class_bits
    };
    let handle_class_bits = if mode_info.text {
        builtins.text_io_wrapper
    } else {
        binary_class_bits
    };
    let buffer_class_bits = if mode_info.text {
        buffered_class_bits
    } else {
        0
    };
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let mode_string = mode.to_string();
    let buffer_bits = if mode_info.text {
        let buffer_ptr = alloc_file_handle_with_state(
            _py,
            Some(Arc::clone(&state)),
            mode_info.readable,
            mode_info.writable,
            false,
            false,
            !borrows_host_fd,
            false,
            false,
            buffer_size,
            buffer_class_bits,
            name_bits,
            mode_string.clone(),
            None,
            None,
            None,
            None,
            0,
            0,
        );
        if buffer_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            return MoltObject::none().bits();
        }
        if trace_stdio
            && let Some((kind, _message)) =
                crate::builtins::exceptions::pending_exception_diagnostic(_py)
        {
            eprintln!("stdio build pending after buffer alloc fd={fd}: {kind}");
        }
        MoltObject::from_ptr(buffer_ptr).bits()
    } else {
        0
    };
    let ptr = alloc_file_handle_with_state(
        _py,
        Some(state),
        mode_info.readable,
        mode_info.writable,
        mode_info.text,
        false, // closefd: closing a standard stream never closes its fd
        !borrows_host_fd,
        line_buffering,
        write_through,
        buffer_size,
        handle_class_bits,
        name_bits,
        mode_string,
        Some(
            normalize_text_encoding(
                &crate::builtins::platform::startup_encoding(_py).stdio_encoding,
            )
            .expect("startup validates the stdio codec")
            .0,
        ),
        Some(
            crate::builtins::platform::startup_encoding(_py)
                .stdio_encoding
                .clone(),
        ),
        Some(errors.to_string()),
        None,
        buffer_bits,
        0,
    );
    if buffer_bits != 0 {
        dec_ref_bits(_py, buffer_bits);
    }
    if trace_stdio
        && let Some((kind, _message)) =
            crate::builtins::exceptions::pending_exception_diagnostic(_py)
    {
        eprintln!("stdio build pending after wrapper alloc fd={fd}: {kind}");
    }
    dec_ref_bits(_py, name_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn cached_stdio_handle(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    make_handle: impl FnOnce() -> u64,
) -> u64 {
    let trace_stdio = runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_STDIO_BUILD);
    let cached_bits = slot.load(Ordering::Acquire);
    if cached_bits != 0 && !obj_from_bits(cached_bits).is_none() {
        inc_ref_bits(_py, cached_bits);
        return cached_bits;
    }

    let handle_bits = make_handle();
    if obj_from_bits(handle_bits).is_none() {
        return handle_bits;
    }
    if trace_stdio
        && let Some((kind, _message)) =
            crate::builtins::exceptions::pending_exception_diagnostic(_py)
    {
        eprintln!("stdio build pending after make_handle: {kind}");
    }

    // Keep one pinned reference so repeated sys stdio lookups share the same
    // handle object instead of allocating/closing duplicate descriptors.
    inc_ref_bits(_py, handle_bits);
    let prev = slot.swap(handle_bits, Ordering::AcqRel);
    if prev != 0 && prev != handle_bits && !obj_from_bits(prev).is_none() {
        dec_ref_bits(_py, prev);
    }
    handle_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stdin() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cached_stdio_handle(_py, &runtime_state(_py).io.sys_stdin_handle_bits, || {
            alloc_stdio_handle(
                _py,
                0,
                true,
                false,
                "<stdin>",
                &crate::builtins::platform::startup_encoding(_py).stdio_errors,
                false,
            )
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stdout() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cached_stdio_handle(_py, &runtime_state(_py).io.sys_stdout_handle_bits, || {
            alloc_stdio_handle(
                _py,
                1,
                false,
                true,
                "<stdout>",
                &crate::builtins::platform::startup_encoding(_py).stdio_errors,
                false,
            )
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stderr() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cached_stdio_handle(_py, &runtime_state(_py).io.sys_stderr_handle_bits, || {
            alloc_stdio_handle(_py, 2, false, true, "<stderr>", "backslashreplace", true)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_open(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        open_impl(
            _py,
            path_bits,
            mode_bits,
            MoltObject::from_int(-1).bits(),
            none,
            none,
            none,
            MoltObject::from_bool(true).bits(),
            none,
            OpenFileName::Resolved,
            None,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_open_ex(
    file_bits: u64,
    mode_bits: u64,
    buffering_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        open_impl(
            _py,
            file_bits,
            mode_bits,
            buffering_bits,
            encoding_bits,
            errors_bits,
            newline_bits,
            closefd_bits,
            opener_bits,
            OpenFileName::Resolved,
            None,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_io_new(
    cls_bits: u64,
    name_bits: u64,
    mode_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if crate::builtins::type_ops::native_constructor_receiver(
            _py,
            builtin_classes(_py).file_io,
            Some(cls_bits),
            "FileIO",
        )
        .is_none()
        {
            return MoltObject::none().bits();
        }
        let mode_obj = obj_from_bits(mode_bits);
        let mut mode = if mode_bits == missing_bits(_py) {
            "r".to_string()
        } else if let Some(mode) = string_obj_to_owned(mode_obj) {
            mode
        } else {
            let type_name = class_name_for_error(type_of_bits(_py, mode_bits));
            let msg = format!("FileIO() argument 'mode' must be str, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if mode.contains('t') {
            return raise_exception::<_>(_py, "ValueError", "FileIO() doesn't take text mode");
        }
        if !mode.contains('b') {
            mode.push('b');
        }
        let mode_ptr = alloc_string(_py, mode.as_bytes());
        if mode_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mode_bits = MoltObject::from_ptr(mode_ptr).bits();
        let buffering_bits = MoltObject::from_int(0).bits();
        let none = MoltObject::none().bits();
        let out = open_impl(
            _py,
            name_bits,
            mode_bits,
            buffering_bits,
            none,
            none,
            none,
            closefd_bits,
            opener_bits,
            OpenFileName::Original,
            Some(cls_bits),
        );
        dec_ref_bits(_py, mode_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_io_init(
    _self_bits: u64,
    _name_bits: u64,
    _mode_bits: u64,
    _closefd_bits: u64,
    _opener_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_open_builtin(
    file_bits: u64,
    mode_bits: u64,
    buffering_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        open_impl(
            _py,
            file_bits,
            mode_bits,
            buffering_bits,
            encoding_bits,
            errors_bits,
            newline_bits,
            closefd_bits,
            opener_bits,
            OpenFileName::Resolved,
            None,
        )
    })
}
