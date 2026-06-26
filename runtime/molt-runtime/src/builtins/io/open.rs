use super::*;

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
    #[allow(clippy::useless_conversion)]
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
    if fd < 0 {
        return None;
    }
    // `File::from_raw_fd` will happily wrap an invalid fd; validate upfront so
    // `open(fd)` matches CPython and raises immediately for EBADF.
    let rc = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFD) };
    if rc < 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_fd(fd as i32) })
}

#[cfg(windows)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let handle = unsafe { libc::get_osfhandle(fd as libc::c_int) };
    if handle == -1 {
        return None;
    }
    let dup = duplicate_handle(handle as *mut std::ffi::c_void)?;
    Some(unsafe { std::fs::File::from_raw_handle(dup as *mut _) })
}

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::wasi::io::FromRawFd;
    if fd < 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_fd(fd as std::os::wasi::io::RawFd) })
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
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd as libc::c_int) };
    if duped < 0 { None } else { Some(duped as i64) }
}

#[cfg(windows)]
pub(crate) fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd as libc::c_int) };
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
fn windows_crt_fd_from_handle(
    handle: *mut std::ffi::c_void,
    readable: bool,
    writable: bool,
) -> Option<i64> {
    let dup = duplicate_handle(handle)?;
    let mut flags = libc::O_BINARY;
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

fn open_arg_path(_py: &PyToken<'_>, file_bits: u64) -> Result<(std::path::PathBuf, u64), String> {
    let obj = obj_from_bits(file_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        let name_ptr = alloc_string(_py, text.as_bytes());
        if name_ptr.is_null() {
            return Err("open failed".to_string());
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        return Ok((std::path::PathBuf::from(text), name_bits));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let name_ptr = alloc_bytes(_py, bytes);
                if name_ptr.is_null() {
                    return Err("open failed".to_string());
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    let path = std::ffi::OsString::from_vec(bytes.to_vec());
                    return Ok((std::path::PathBuf::from(path), name_bits));
                }
                #[cfg(windows)]
                {
                    let path = std::str::from_utf8(bytes)
                        .map_err(|_| "open path bytes must be utf-8".to_string())?;
                    return Ok((std::path::PathBuf::from(path), name_bits));
                }
            }
            let fspath_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.fspath_name, b"__fspath__");
            if let Some(call_bits) = attr_lookup_ptr(_py, ptr, fspath_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return Err("open failed".to_string());
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(text) = string_obj_to_owned(res_obj) {
                    let name_ptr = alloc_string(_py, text.as_bytes());
                    if name_ptr.is_null() {
                        return Err("open failed".to_string());
                    }
                    let name_bits = MoltObject::from_ptr(name_ptr).bits();
                    dec_ref_bits(_py, res_bits);
                    return Ok((std::path::PathBuf::from(text), name_bits));
                }
                if let Some(res_ptr) = res_obj.as_ptr()
                    && object_type_id(res_ptr) == TYPE_ID_BYTES
                {
                    let len = bytes_len(res_ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                    let name_ptr = alloc_bytes(_py, bytes);
                    if name_ptr.is_null() {
                        return Err("open failed".to_string());
                    }
                    let name_bits = MoltObject::from_ptr(name_ptr).bits();
                    #[cfg(unix)]
                    {
                        use std::os::unix::ffi::OsStringExt;
                        let path = std::ffi::OsString::from_vec(bytes.to_vec());
                        dec_ref_bits(_py, res_bits);
                        return Ok((std::path::PathBuf::from(path), name_bits));
                    }
                    #[cfg(windows)]
                    {
                        let path = std::str::from_utf8(bytes)
                            .map_err(|_| "open path bytes must be utf-8".to_string())?;
                        dec_ref_bits(_py, res_bits);
                        return Ok((std::path::PathBuf::from(path), name_bits));
                    }
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                dec_ref_bits(_py, res_bits);
                let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
                return Err(format!(
                    "expected {obj_type}.__fspath__() to return str or bytes, not {res_type}"
                ));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
    Err(format!(
        "expected str, bytes or os.PathLike object, not {obj_type}"
    ))
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

    let debug_open_fd = std::env::var("MOLT_DEBUG_OPEN_FD").as_deref() == Ok("1");

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
    if mode_info.readable && !has_capability(_py, "fs.read") {
        return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
    }
    if mode_info.writable && !has_capability(_py, "fs.write") {
        return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
    }

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
    let opener_obj = obj_from_bits(opener_bits);
    let opener_is_none = opener_obj.is_none();

    let mut path_guard = BitsGuard { py: _py, bits: 0 };
    let mut path = None;
    let mut fd: Option<i64> = None;
    let mut debug_fd_value: Option<i64> = None;
    let path_name_bits = if let Some(i) = to_i64(obj_from_bits(file_bits)) {
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
        match open_arg_path(_py, file_bits) {
            Ok((resolved, name_bits)) => {
                if !closefd {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "Cannot use closefd=False with file name",
                    );
                }
                path = Some(resolved);
                path_guard.bits = name_bits;
                name_bits
            }
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        }
    };

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
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Some(fd_val) = to_i64(obj_from_bits(fd_bits)) {
                if let Some(handle) = file_from_fd(fd_val) {
                    file = Some(handle);
                } else {
                    return raise_exception::<_>(_py, "OSError", "open failed");
                }
            } else {
                let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
                let msg = format!("expected opener to return int, got {type_name}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            dec_ref_bits(_py, fd_bits);
        } else {
            // VFS dispatch (Plan B v0.1)
            // If the path resolves through a VFS mount, serve the read
            // from the in-memory backend rather than the real filesystem.
            let path_str = path.to_string_lossy();
            if let Some(vfs) = runtime_state(_py).get_vfs()
                && let Some((mount_prefix, backend, rel_path)) = vfs.resolve(&path_str)
            {
                let is_write = mode_info.writable;
                // Capability check
                let cap_result = crate::vfs::caps::check_mount_capability(
                    &mount_prefix,
                    is_write,
                    &|cap_name| has_capability(_py, cap_name),
                );
                if let Err(vfs_err) = cap_result {
                    let msg = format!("{vfs_err}: '{path_str}'");
                    return raise_exception::<_>(_py, "PermissionError", &msg);
                }
                // VFS read / write dispatch
                // For reads: load existing file content into a bytearray.
                // For writes: start with empty (truncate) or existing
                //   (append) content, and register a writeback entry so
                //   molt_file_close flushes the final bytearray content
                //   back to the VFS backend.
                let data: Vec<u8> = if is_write && !mode_info.append {
                    // Write-truncate: start empty.
                    Vec::new()
                } else if is_write && mode_info.append {
                    // Append: seed with existing content (if any).
                    backend.open_read(&rel_path).unwrap_or_default()
                } else {
                    // Read-only: load the full file.
                    match backend.open_read(&rel_path) {
                        Ok(bytes) => bytes,
                        Err(vfs_err) => {
                            let msg = format!("{vfs_err}: '{path_str}'");
                            return match vfs_err {
                                crate::vfs::VfsError::NotFound => {
                                    raise_exception::<_>(_py, "FileNotFoundError", &msg)
                                }
                                crate::vfs::VfsError::PermissionDenied
                                | crate::vfs::VfsError::ReadOnly
                                | crate::vfs::VfsError::CapabilityDenied(_) => {
                                    raise_exception::<_>(_py, "PermissionError", &msg)
                                }
                                crate::vfs::VfsError::IsDirectory => {
                                    raise_exception::<_>(_py, "IsADirectoryError", &msg)
                                }
                                _ => raise_exception::<_>(_py, "OSError", &msg),
                            };
                        }
                    }
                };

                // Clone the Arc before dropping the VFS lock so we can
                // register it in the writeback map for writable handles.
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
                });

                // Register VFS writeback so molt_file_close can flush
                // the bytearray content back to the VFS backend.
                if let Some(entry) = vfs_backend_arc {
                    vfs_writeback_register(_py, &vfs_state, entry);
                }

                // Reuse the same encoding / errors / newline resolution
                // that the normal path uses.
                let enc = if mode_info.text {
                    let e = encoding.unwrap_or_else(|| "utf-8".to_string());
                    let (label, _kind) = match normalize_text_encoding(&e) {
                        Ok(val) => val,
                        Err(msg) => {
                            dec_ref_bits(_py, mem_bits);
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                    };
                    Some(label)
                } else {
                    None
                };
                let enc_original = enc.clone();
                let errs = if mode_info.text {
                    Some(errors.unwrap_or_else(|| "strict".to_string()))
                } else {
                    None
                };

                let vfs_readable = mode_info.readable || mode_info.append;
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
                let buf_size = if buffering == 0 {
                    0
                } else if line_buffering || buffering < 0 {
                    DEFAULT_BUFFER_SIZE
                } else {
                    buffering
                };
                let buffer_bits = if mode_info.text {
                    let buffer_ptr = alloc_file_handle_with_state(
                        _py,
                        Arc::clone(&vfs_state),
                        vfs_readable,
                        vfs_writable,
                        false, // text
                        false, // closefd
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
                    vfs_state,
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
        crt_fd = windows_crt_fd_from_handle(handle, mode_info.readable, mode_info.writable);
    }
    let Some(file) = file else {
        return raise_exception::<_>(_py, "OSError", "open failed");
    };

    // Keep text-I/O encoding normalization explicit so open()/TextIOWrapper
    // remains deterministic across native and wasm builds.
    let encoding = if mode_info.text {
        let encoding = encoding.unwrap_or_else(|| "utf-8".to_string());
        let (label, _kind) = match normalize_text_encoding(&encoding) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
        };
        Some(label)
    } else {
        None
    };
    let errors = if mode_info.text {
        Some(errors.unwrap_or_else(|| "strict".to_string()))
    } else {
        None
    };

    let encoding_original = encoding.clone();
    let state = Arc::new(MoltFileState {
        backend: Mutex::new(Some(MoltFileBackend::File(file))),
        #[cfg(windows)]
        crt_fd: Mutex::new(crt_fd),
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
    let buffer_size = if buffering == 0 {
        0
    } else if line_buffering || buffering < 0 {
        DEFAULT_BUFFER_SIZE
    } else {
        buffering
    };
    let buffer_bits = if mode_info.text {
        let buffer_ptr = alloc_file_handle_with_state(
            _py,
            Arc::clone(&state),
            mode_info.readable,
            mode_info.writable,
            false,
            false,
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
        state,
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
    let trace_stdio = std::env::var("MOLT_TRACE_STDIO_BUILD").as_deref() == Ok("1");
    let effective_fd = if cfg!(target_arch = "wasm32") {
        fd
    } else {
        match dup_fd(fd) {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        }
    };
    let Some(file) = file_from_fd(effective_fd) else {
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
        DEFAULT_BUFFER_SIZE
    } else {
        buffering
    };

    let state = Arc::new(MoltFileState {
        backend: Mutex::new(Some(MoltFileBackend::File(file))),
        #[cfg(windows)]
        crt_fd: Mutex::new(Some(effective_fd)),
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
            Arc::clone(&state),
            mode_info.readable,
            mode_info.writable,
            false,
            false,
            true,
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
        if trace_stdio && exception_pending(_py) {
            let exc_bits = molt_exception_last();
            let kind_bits = molt_exception_kind(exc_bits);
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<exc>".to_string());
            eprintln!("stdio build pending after buffer alloc fd={fd}: {kind}");
        }
        MoltObject::from_ptr(buffer_ptr).bits()
    } else {
        0
    };
    let ptr = alloc_file_handle_with_state(
        _py,
        state,
        mode_info.readable,
        mode_info.writable,
        mode_info.text,
        true,
        true,
        line_buffering,
        write_through,
        buffer_size,
        handle_class_bits,
        name_bits,
        mode_string,
        Some("utf-8".to_string()),
        Some("utf-8".to_string()),
        Some(errors.to_string()),
        None,
        buffer_bits,
        0,
    );
    if buffer_bits != 0 {
        dec_ref_bits(_py, buffer_bits);
    }
    if trace_stdio && exception_pending(_py) {
        let exc_bits = molt_exception_last();
        let kind_bits = molt_exception_kind(exc_bits);
        let kind =
            string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_else(|| "<exc>".to_string());
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
    let trace_stdio = std::env::var("MOLT_TRACE_STDIO_BUILD").as_deref() == Ok("1");
    let cached_bits = slot.load(Ordering::Acquire);
    if cached_bits != 0 && !obj_from_bits(cached_bits).is_none() {
        inc_ref_bits(_py, cached_bits);
        return cached_bits;
    }

    let handle_bits = make_handle();
    if obj_from_bits(handle_bits).is_none() {
        return handle_bits;
    }
    if trace_stdio && exception_pending(_py) {
        let exc_bits = molt_exception_last();
        let kind_bits = molt_exception_kind(exc_bits);
        let kind =
            string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_else(|| "<exc>".to_string());
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
            alloc_stdio_handle(_py, 0, true, false, "<stdin>", "surrogateescape", false)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stdout() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cached_stdio_handle(_py, &runtime_state(_py).io.sys_stdout_handle_bits, || {
            alloc_stdio_handle(_py, 1, false, true, "<stdout>", "surrogateescape", false)
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
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_io_new(
    _cls_bits: u64,
    name_bits: u64,
    mode_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode_obj = obj_from_bits(mode_bits);
        let mut mode = if mode_obj.is_none() {
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
        let closefd_bits = if obj_from_bits(closefd_bits).is_none() {
            MoltObject::from_bool(true).bits()
        } else {
            closefd_bits
        };
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
        )
    })
}
