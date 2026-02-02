use crate::PyToken;
use crate::*;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Seek, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

macro_rules! file_handle_require_attached {
    ($py:expr, $handle:expr) => {
        if $handle.detached {
            return raise_exception::<_>($py, "ValueError", file_handle_detached_message($handle));
        }
    };
}

#[allow(clippy::too_many_arguments)]
fn alloc_file_handle_with_state(
    _py: &PyToken<'_>,
    state: Arc<MoltFileState>,
    readable: bool,
    writable: bool,
    text: bool,
    closefd: bool,
    owns_fd: bool,
    line_buffering: bool,
    write_through: bool,
    buffer_size: i64,
    class_bits: u64,
    name_bits: u64,
    mode: String,
    encoding: Option<String>,
    errors: Option<String>,
    newline: Option<String>,
    buffer_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut MoltFileHandle>();
    let ptr = alloc_object(_py, total, TYPE_ID_FILE_HANDLE);
    if ptr.is_null() {
        return ptr;
    }
    let handle = Box::new(MoltFileHandle {
        state,
        readable,
        writable,
        text,
        closefd,
        owns_fd,
        closed: false,
        detached: false,
        line_buffering,
        write_through,
        buffer_size,
        class_bits,
        name_bits,
        mode,
        encoding,
        errors,
        newline,
        buffer_bits,
        pending_byte: None,
    });
    if name_bits != 0 {
        inc_ref_bits(_py, name_bits);
    }
    if buffer_bits != 0 {
        inc_ref_bits(_py, buffer_bits);
    }
    let handle_ptr = Box::into_raw(handle);
    unsafe {
        *(ptr as *mut *mut MoltFileHandle) = handle_ptr;
    }
    ptr
}

fn file_handle_close_ptr(ptr: *mut u8) -> bool {
    if ptr.is_null() {
        return false;
    }
    unsafe {
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return false;
        }
        let handle = &mut *handle_ptr;
        if handle.closed {
            return false;
        }
        handle.closed = true;
        if !handle.owns_fd {
            return false;
        }
        let mut guard = handle.state.file.lock().unwrap();
        guard.take().is_some()
    }
}

pub(crate) unsafe fn file_handle_enter(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let bits = MoltObject::from_ptr(ptr).bits();
    let handle_ptr = file_handle_ptr(ptr);
    if !handle_ptr.is_null() {
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(_py, handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        }
        handle.closed = false;
    }
    inc_ref_bits(_py, bits);
    bits
}

pub(crate) unsafe fn file_handle_exit(_py: &PyToken<'_>, ptr: *mut u8, _exc_bits: u64) -> u64 {
    let handle_ptr = file_handle_ptr(ptr);
    if !handle_ptr.is_null() {
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(_py, handle);
        file_handle_close_ptr(ptr);
        handle.closed = true;
    }
    MoltObject::from_bool(false).bits()
}

pub(crate) fn close_payload(_py: &PyToken<'_>, payload_bits: u64) {
    let payload = obj_from_bits(payload_bits);
    let Some(ptr) = payload.as_ptr() else {
        return raise_exception::<()>(_py, "AttributeError", "object has no attribute 'close'");
    };
    unsafe {
        if object_type_id(ptr) == TYPE_ID_FILE_HANDLE {
            let handle_ptr = file_handle_ptr(ptr);
            if !handle_ptr.is_null() {
                let handle = &*handle_ptr;
                file_handle_require_attached!(_py, handle);
            }
            file_handle_close_ptr(ptr);
            return;
        }
    }
    raise_exception::<()>(_py, "AttributeError", "object has no attribute 'close'");
}

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

fn open_arg_newline(_py: &PyToken<'_>, bits: u64) -> Option<String> {
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

fn reconfigure_arg_type(_py: &PyToken<'_>, bits: u64, name: &str) -> Option<String> {
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

fn reconfigure_arg_newline(_py: &PyToken<'_>, bits: u64) -> Option<String> {
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
    Some(unsafe { std::fs::File::from_raw_fd(fd as i32) })
}

#[cfg(windows)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
    if handle == -1 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_handle(handle as *mut _) })
}

#[cfg(not(any(unix, windows)))]
fn file_from_fd(_fd: i64) -> Option<std::fs::File> {
    None
}

#[cfg(unix)]
fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd as libc::c_int) };
    if duped < 0 {
        None
    } else {
        Some(duped as i64)
    }
}

#[cfg(windows)]
fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::_dup(fd as libc::c_int) };
    if duped < 0 {
        None
    } else {
        Some(duped as i64)
    }
}

#[cfg(not(any(unix, windows)))]
fn dup_fd(_fd: i64) -> Option<i64> {
    None
}

#[cfg(windows)]
const FILE_TYPE_CHAR: u32 = 0x0002;

#[cfg(windows)]
const HANDLE_FLAG_INHERIT: u32 = 0x00000001;

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetFileType(hFile: *mut std::ffi::c_void) -> u32;
    fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
    fn GetHandleInformation(
        hObject: *mut std::ffi::c_void,
        lpdwFlags: *mut u32,
    ) -> i32;
    fn SetHandleInformation(
        hObject: *mut std::ffi::c_void,
        dwMask: u32,
        dwFlags: u32,
    ) -> i32;
}

#[cfg(windows)]
fn windows_handle_isatty(handle: *mut std::ffi::c_void) -> bool {
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
        let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
        windows_handle_isatty(handle as *mut std::ffi::c_void)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fd;
        false
    }
}

pub(crate) fn path_from_bits(
    _py: &PyToken<'_>,
    file_bits: u64,
) -> Result<std::path::PathBuf, String> {
    let obj = obj_from_bits(file_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(std::path::PathBuf::from(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    let path = std::ffi::OsString::from_vec(bytes.to_vec());
                    return Ok(std::path::PathBuf::from(path));
                }
                #[cfg(windows)]
                {
                    let path = std::str::from_utf8(bytes)
                        .map_err(|_| "open path bytes must be utf-8".to_string())?;
                    return Ok(std::path::PathBuf::from(path));
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
                    dec_ref_bits(_py, res_bits);
                    return Ok(std::path::PathBuf::from(text));
                }
                if let Some(res_ptr) = res_obj.as_ptr() {
                    if object_type_id(res_ptr) == TYPE_ID_BYTES {
                        let len = bytes_len(res_ptr);
                        let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                        #[cfg(unix)]
                        {
                            use std::os::unix::ffi::OsStringExt;
                            let path = std::ffi::OsString::from_vec(bytes.to_vec());
                            dec_ref_bits(_py, res_bits);
                            return Ok(std::path::PathBuf::from(path));
                        }
                        #[cfg(windows)]
                        {
                            let path = std::str::from_utf8(bytes)
                                .map_err(|_| "open path bytes must be utf-8".to_string())?;
                            dec_ref_bits(_py, res_bits);
                            return Ok(std::path::PathBuf::from(path));
                        }
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
                if let Some(res_ptr) = res_obj.as_ptr() {
                    if object_type_id(res_ptr) == TYPE_ID_BYTES {
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
    let path_name_bits = if let Some(i) = to_i64(obj_from_bits(file_bits)) {
        fd = Some(i);
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
                    let msg = if let Some(code) = err.raw_os_error() {
                        format!("[Errno {code}] {short}: '{path_display}'")
                    } else {
                        format!("{short}: '{path_display}'")
                    };
                    match err.kind() {
                        ErrorKind::AlreadyExists => {
                            return raise_exception::<_>(_py, "FileExistsError", &msg)
                        }
                        ErrorKind::NotFound => {
                            return raise_exception::<_>(_py, "FileNotFoundError", &msg)
                        }
                        ErrorKind::PermissionDenied => {
                            return raise_exception::<_>(_py, "PermissionError", &msg)
                        }
                        ErrorKind::IsADirectory => {
                            return raise_exception::<_>(_py, "IsADirectoryError", &msg)
                        }
                        ErrorKind::NotADirectory => {
                            return raise_exception::<_>(_py, "NotADirectoryError", &msg)
                        }
                        _ => return raise_exception::<_>(_py, "OSError", &msg),
                    }
                }
            };
        }
    }
    let Some(file) = file else {
        return raise_exception::<_>(_py, "OSError", "open failed");
    };

    // TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial):
    // extend encoding support beyond utf-8/ascii/latin-1 and expand error handlers for text I/O.
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

    let state = Arc::new(MoltFileState {
        file: Mutex::new(Some(file)),
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
    let buffer_size = if buffering == 0 { 0 } else { buffering };
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
            0,
        );
        if buffer_ptr.is_null() {
            return MoltObject::none().bits();
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
        errors,
        newline,
        buffer_bits,
    );
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
    write_through: bool,
) -> u64 {
    let effective_fd = match dup_fd(fd) {
        Some(val) => val,
        None => return MoltObject::none().bits(),
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
    let buffer_size = if buffering == 0 { 0 } else { buffering };

    let state = Arc::new(MoltFileState {
        file: Mutex::new(Some(file)),
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
            0,
        );
        if buffer_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            return MoltObject::none().bits();
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
        Some("strict".to_string()),
        None,
        buffer_bits,
    );
    if buffer_bits != 0 {
        dec_ref_bits(_py, buffer_bits);
    }
    dec_ref_bits(_py, name_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_sys_stdin() -> u64 {
    crate::with_gil_entry!(_py, {
        alloc_stdio_handle(_py, 0, true, false, "<stdin>", false)
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_stdout() -> u64 {
    crate::with_gil_entry!(_py, {
        alloc_stdio_handle(_py, 1, false, true, "<stdout>", false)
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_stderr() -> u64 {
    crate::with_gil_entry!(_py, {
        alloc_stdio_handle(_py, 2, false, true, "<stderr>", true)
    })
}

#[no_mangle]
pub extern "C" fn molt_file_open(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_path_exists(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        MoltObject::from_bool(std::fs::metadata(path).is_ok()).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_path_listdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mut entries: Vec<u64> = Vec::new();
        let read_dir = match std::fs::read_dir(&path) {
            Ok(dir) => dir,
            Err(err) => {
                let msg = err.to_string();
                return match err.kind() {
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, "FileNotFoundError", &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::NotADirectory => {
                        raise_exception::<_>(_py, "NotADirectoryError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                };
            }
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    let msg = err.to_string();
                    return raise_exception::<_>(_py, "OSError", &msg);
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                for bits in entries {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            entries.push(MoltObject::from_ptr(name_ptr).bits());
        }
        let list_ptr = alloc_list(_py, entries.as_slice());
        if list_ptr.is_null() {
            for bits in entries {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        for bits in entries {
            dec_ref_bits(_py, bits);
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_path_mkdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::create_dir(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => {
                        raise_exception::<_>(_py, "FileExistsError", &msg)
                    }
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, "FileNotFoundError", &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_path_unlink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::remove_file(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, "FileNotFoundError", &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::IsADirectory => {
                        raise_exception::<_>(_py, "IsADirectoryError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_path_rmdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::remove_dir(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, "FileNotFoundError", &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::DirectoryNotEmpty => {
                        raise_exception::<_>(_py, "OSError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_path_chmod(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mode = index_i64_from_obj(_py, mode_bits, "chmod() mode must be int");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        #[cfg(unix)]
        {
            let perms = std::fs::Permissions::from_mode(mode as u32);
            match std::fs::set_permissions(&path, perms) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => {
                    let msg = err.to_string();
                    match err.kind() {
                        ErrorKind::NotFound => {
                            raise_exception::<_>(_py, "FileNotFoundError", &msg)
                        }
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, "PermissionError", &msg)
                        }
                        _ => raise_exception::<_>(_py, "OSError", &msg),
                    }
                }
            }
        }
        #[cfg(windows)]
        {
            let readonly = ((mode as u32) & 0o222) == 0;
            let meta = match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(err) => {
                    let msg = err.to_string();
                    return match err.kind() {
                        ErrorKind::NotFound => {
                            raise_exception::<_>(_py, "FileNotFoundError", &msg)
                        }
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, "PermissionError", &msg)
                        }
                        _ => raise_exception::<_>(_py, "OSError", &msg),
                    };
                }
            };
            let mut perms = meta.permissions();
            perms.set_readonly(readonly);
            match std::fs::set_permissions(&path, perms) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => {
                    let msg = err.to_string();
                    match err.kind() {
                        ErrorKind::NotFound => {
                            raise_exception::<_>(_py, "FileNotFoundError", &msg)
                        }
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, "PermissionError", &msg)
                        }
                        _ => raise_exception::<_>(_py, "OSError", &msg),
                    }
                }
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_getcwd() -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        match std::env::current_dir() {
            Ok(path) => {
                let text = path.to_string_lossy();
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::NotADirectory => {
                        raise_exception::<_>(_py, "NotADirectoryError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_os_close(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "close");
        }
        #[cfg(target_arch = "wasm32")]
        {
            let rc = unsafe { crate::molt_os_close_host(fd) };
            if rc < 0 {
                return raise_os_error_errno::<u64>(_py, (-rc) as i64, "close");
            }
            return MoltObject::none().bits();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            {
                let rc = unsafe { libc::close(fd as libc::c_int) };
                if rc == 0 {
                    return MoltObject::none().bits();
                }
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "close");
                }
                return raise_os_error::<u64>(_py, err, "close");
            }
            #[cfg(windows)]
            {
                let sock_rc = unsafe { libc::closesocket(fd as libc::SOCKET) };
                if sock_rc == 0 {
                    return MoltObject::none().bits();
                }
                let sock_err = unsafe { libc::WSAGetLastError() };
                if sock_err == libc::WSAENOTSOCK {
                    let rc = unsafe { libc::_close(fd as libc::c_int) };
                    if rc == 0 {
                        return MoltObject::none().bits();
                    }
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "close");
                    }
                    return raise_os_error::<u64>(_py, err, "close");
                }
                return raise_os_error_errno::<u64>(_py, sock_err as i64, "close");
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_os_dup(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "dup");
        }
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "dup");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let duped = dup_fd(fd);
            if let Some(new_fd) = duped {
                return int_bits_from_i64(_py, new_fd);
            }
            let err = std::io::Error::last_os_error();
            if let Some(errno) = err.raw_os_error() {
                return raise_os_error_errno::<u64>(_py, errno as i64, "dup");
            }
            raise_os_error::<u64>(_py, err, "dup")
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_os_get_inheritable(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "get_inheritable");
        }
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_inheritable");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            {
                let flags = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFD) };
                if flags < 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "get_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "get_inheritable");
                }
                let inheritable = (flags & libc::FD_CLOEXEC) == 0;
                return MoltObject::from_bool(inheritable).bits();
            }
            #[cfg(windows)]
            {
                let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
                if handle == -1 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "get_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "get_inheritable");
                }
                let mut flags: u32 = 0;
                let ok = unsafe {
                    GetHandleInformation(handle as *mut std::ffi::c_void, &mut flags)
                };
                if ok == 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "get_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "get_inheritable");
                }
                return MoltObject::from_bool((flags & HANDLE_FLAG_INHERIT) != 0).bits();
            }
            #[cfg(not(any(unix, windows)))]
            {
                raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_inheritable")
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_os_set_inheritable(fd_bits: u64, inheritable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "set_inheritable");
        }
        let inheritable = is_truthy(_py, obj_from_bits(inheritable_bits));
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "set_inheritable");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            {
                let flags = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFD) };
                if flags < 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "set_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "set_inheritable");
                }
                let mut new_flags = flags;
                if inheritable {
                    new_flags &= !libc::FD_CLOEXEC;
                } else {
                    new_flags |= libc::FD_CLOEXEC;
                }
                let rc = unsafe { libc::fcntl(fd as libc::c_int, libc::F_SETFD, new_flags) };
                if rc < 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "set_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "set_inheritable");
                }
                return MoltObject::none().bits();
            }
            #[cfg(windows)]
            {
                let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
                if handle == -1 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "set_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "set_inheritable");
                }
                let flags = if inheritable { HANDLE_FLAG_INHERIT } else { 0 };
                let ok = unsafe {
                    SetHandleInformation(
                        handle as *mut std::ffi::c_void,
                        HANDLE_FLAG_INHERIT,
                        flags,
                    )
                };
                if ok == 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "set_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "set_inheritable");
                }
                return MoltObject::none().bits();
            }
            #[cfg(not(any(unix, windows)))]
            {
                raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "set_inheritable")
            }
        }
    })
}

#[no_mangle]
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
    crate::with_gil_entry!(_py, {
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

#[derive(Debug)]
pub(crate) struct DecodeError {
    pub(crate) pos: usize,
    pub(crate) byte: u8,
    pub(crate) message: &'static str,
}

pub(crate) enum DecodeFailure {
    Byte {
        pos: usize,
        byte: u8,
        message: &'static str,
    },
    Range {
        start: usize,
        end: usize,
        message: &'static str,
    },
    UnknownErrorHandler(String),
}

#[derive(Clone, Copy, Debug)]
enum TextEncodingKind {
    Utf8,
    Ascii,
    Latin1,
}

struct TextEncodeError {
    pos: usize,
    ch: char,
    message: &'static str,
}

fn normalize_text_encoding(encoding: &str) -> Result<(String, TextEncodingKind), String> {
    let normalized = encoding.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "utf-8" | "utf8" => Ok(("utf-8".to_string(), TextEncodingKind::Utf8)),
        "ascii" => Ok(("ascii".to_string(), TextEncodingKind::Ascii)),
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" => {
            Ok(("latin-1".to_string(), TextEncodingKind::Latin1))
        }
        _ => Err(format!("unknown encoding: {encoding}")),
    }
}

fn text_encoding_kind(label: &str) -> TextEncodingKind {
    match label {
        "ascii" => TextEncodingKind::Ascii,
        "latin-1" => TextEncodingKind::Latin1,
        _ => TextEncodingKind::Utf8,
    }
}

fn validate_error_handler(errors: &str) -> Result<(), String> {
    if matches!(errors, "strict" | "ignore" | "replace") {
        Ok(())
    } else {
        Err(format!("unknown error handler name '{errors}'"))
    }
}

pub(crate) fn decode_utf8_with_errors(bytes: &[u8], errors: &str) -> Result<String, DecodeError> {
    match errors {
        "ignore" => {
            let mut out = String::new();
            let mut idx = 0usize;
            while idx < bytes.len() {
                match std::str::from_utf8(&bytes[idx..]) {
                    Ok(chunk) => {
                        out.push_str(chunk);
                        break;
                    }
                    Err(err) => {
                        let valid = err.valid_up_to();
                        if valid > 0 {
                            let chunk =
                                unsafe { std::str::from_utf8_unchecked(&bytes[idx..idx + valid]) };
                            out.push_str(chunk);
                            idx += valid;
                        }
                        let skip = err.error_len().unwrap_or(1);
                        idx = idx.saturating_add(skip);
                    }
                }
            }
            Ok(out)
        }
        "replace" => Ok(String::from_utf8_lossy(bytes).into_owned()),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => Ok(text.to_string()),
            Err(err) => {
                let pos = err.valid_up_to();
                let byte = bytes.get(pos).copied().unwrap_or(0);
                Err(DecodeError {
                    pos,
                    byte,
                    message: "invalid start byte",
                })
            }
        },
    }
}

fn decode_text_with_errors(
    bytes: &[u8],
    encoding: TextEncodingKind,
    errors: &str,
) -> Result<String, DecodeError> {
    match encoding {
        TextEncodingKind::Utf8 => decode_utf8_with_errors(bytes, errors),
        TextEncodingKind::Ascii => {
            let mut out = String::with_capacity(bytes.len());
            for (idx, &byte) in bytes.iter().enumerate() {
                if byte <= 0x7f {
                    out.push(byte as char);
                } else {
                    match errors {
                        "ignore" => {}
                        "replace" => out.push('\u{FFFD}'),
                        _ => {
                            return Err(DecodeError {
                                pos: idx,
                                byte,
                                message: "ordinal not in range(128)",
                            });
                        }
                    }
                }
            }
            Ok(out)
        }
        TextEncodingKind::Latin1 => {
            let mut out = String::with_capacity(bytes.len());
            for &byte in bytes {
                out.push(char::from(byte));
            }
            Ok(out)
        }
    }
}

fn encode_text_with_errors(
    text: &str,
    encoding: TextEncodingKind,
    errors: &str,
) -> Result<Vec<u8>, TextEncodeError> {
    match encoding {
        TextEncodingKind::Utf8 => Ok(text.as_bytes().to_vec()),
        TextEncodingKind::Ascii => {
            let mut out = Vec::with_capacity(text.len());
            for (idx, ch) in text.chars().enumerate() {
                let value = ch as u32;
                if value <= 0x7f {
                    out.push(value as u8);
                } else {
                    match errors {
                        "ignore" => {}
                        "replace" => out.push(b'?'),
                        _ => {
                            return Err(TextEncodeError {
                                pos: idx,
                                ch,
                                message: "ordinal not in range(128)",
                            });
                        }
                    }
                }
            }
            Ok(out)
        }
        TextEncodingKind::Latin1 => {
            let mut out = Vec::with_capacity(text.len());
            for (idx, ch) in text.chars().enumerate() {
                let value = ch as u32;
                if value <= 0xff {
                    out.push(value as u8);
                } else {
                    match errors {
                        "ignore" => {}
                        "replace" => out.push(b'?'),
                        _ => {
                            return Err(TextEncodeError {
                                pos: idx,
                                ch,
                                message: "ordinal not in range(256)",
                            });
                        }
                    }
                }
            }
            Ok(out)
        }
    }
}

const TEXT_COOKIE_SHIFT: u32 = 9;
const TEXT_COOKIE_PENDING_FLAG: u64 = 1 << 8;
const TEXT_COOKIE_BYTE_MASK: u64 = 0xff;

fn text_cookie_encode(pos: u64, pending: Option<u8>) -> Result<i64, String> {
    let mut value = pos
        .checked_shl(TEXT_COOKIE_SHIFT)
        .ok_or_else(|| "tell overflow".to_string())?;
    if let Some(byte) = pending {
        value |= TEXT_COOKIE_PENDING_FLAG | (byte as u64);
    }
    if value > i64::MAX as u64 {
        return Err("tell overflow".to_string());
    }
    Ok(value as i64)
}

fn text_cookie_decode(cookie: i64) -> Result<(u64, Option<u8>), String> {
    if cookie < 0 {
        return Err("negative seek position".to_string());
    }
    let raw = cookie as u64;
    let pending = if (raw & TEXT_COOKIE_PENDING_FLAG) != 0 {
        Some((raw & TEXT_COOKIE_BYTE_MASK) as u8)
    } else {
        None
    };
    let pos = raw >> TEXT_COOKIE_SHIFT;
    Ok((pos, pending))
}

fn translate_universal_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\r' => {
                if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                    idx += 2;
                } else {
                    idx += 1;
                }
                out.push(b'\n');
            }
            byte => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    out
}

fn translate_write_newlines(text: &str, newline: Option<&str>) -> String {
    let target = match newline {
        None => {
            if cfg!(windows) {
                "\r\n"
            } else {
                "\n"
            }
        }
        Some("") | Some("\n") => "\n",
        Some(value) => value,
    };
    if target == "\n" {
        return text.to_string();
    }
    text.replace('\n', target)
}

pub(crate) fn file_handle_detached_message(handle: &MoltFileHandle) -> &'static str {
    if handle.text {
        "underlying buffer has been detached"
    } else {
        "raw stream has been detached"
    }
}

pub(crate) fn file_handle_is_closed(handle: &MoltFileHandle) -> bool {
    if handle.closed {
        return true;
    }
    handle.state.file.lock().unwrap().is_none()
}

#[no_mangle]
pub extern "C" fn molt_file_read(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let mut buf = Vec::new();
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg = format!("argument should be integer or None, not '{type_name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let mut remaining = size;
            let mut at_eof = false;
            if let Some(pending) = handle.pending_byte.take() {
                if let Some(rem) = remaining {
                    if rem == 0 {
                        handle.pending_byte = Some(pending);
                    } else {
                        buf.push(pending);
                        remaining = Some(rem.saturating_sub(1));
                    }
                } else {
                    buf.push(pending);
                }
            }
            match remaining {
                Some(0) => {}
                Some(len) => {
                    if len > 0 {
                        let start = buf.len();
                        buf.resize(start + len, 0);
                        let n = match file.read(&mut buf[start..]) {
                            Ok(n) => n,
                            Err(_) => return raise_exception::<_>(_py, "OSError", "read failed"),
                        };
                        buf.truncate(start + n);
                        if n < len {
                            at_eof = true;
                        }
                    }
                }
                None => {
                    if file.read_to_end(&mut buf).is_err() {
                        return raise_exception::<_>(_py, "OSError", "read failed");
                    }
                    at_eof = true;
                }
            }
            if handle.text {
                if handle.newline.is_none() && buf.last() == Some(&b'\r') && !at_eof {
                    handle.pending_byte = Some(b'\r');
                    buf.pop();
                }
                let bytes = if handle.newline.is_none() {
                    translate_universal_newlines(&buf)
                } else {
                    buf
                };
                let errors = handle.errors.as_deref().unwrap_or("strict");
                if let Err(msg) = validate_error_handler(errors) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                let encoding = text_encoding_kind(encoding_label);
                let text = match decode_text_with_errors(&bytes, encoding, errors) {
                    Ok(text) => text,
                    Err(err) => {
                        let msg = format!(
                        "'{encoding_label}' codec can't decode byte 0x{:02x} in position {}: {}",
                        err.byte, err.pos, err.message
                    );
                        return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                    }
                };
                let out_ptr = alloc_string(_py, text.as_bytes());
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let out_ptr = alloc_bytes(_py, &buf);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

fn file_read_byte(
    pending_byte: &mut Option<u8>,
    file: &mut std::fs::File,
) -> std::io::Result<Option<u8>> {
    if let Some(pending) = pending_byte.take() {
        return Ok(Some(pending));
    }
    let mut buf = [0u8; 1];
    let read = file.read(&mut buf)?;
    if read == 0 {
        Ok(None)
    } else {
        Ok(Some(buf[0]))
    }
}

fn file_unread_byte(pending_byte: &mut Option<u8>, byte: u8) {
    *pending_byte = Some(byte);
}

fn file_readline_bytes(
    pending_byte: &mut Option<u8>,
    file: &mut std::fs::File,
    newline: Option<&str>,
    text: bool,
    size: Option<usize>,
) -> std::io::Result<Vec<u8>> {
    // TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial):
    // size limits should count decoded chars for text I/O, not raw bytes.
    let mut out: Vec<u8> = Vec::new();
    loop {
        if let Some(limit) = size {
            if out.len() >= limit {
                break;
            }
        }
        let Some(byte) = file_read_byte(pending_byte, file)? else {
            break;
        };
        if text {
            match newline {
                None => {
                    if byte == b'\n' {
                        out.push(b'\n');
                        break;
                    }
                    if byte == b'\r' {
                        if let Some(next) = file_read_byte(pending_byte, file)? {
                            if next != b'\n' {
                                file_unread_byte(pending_byte, next);
                            }
                        }
                        out.push(b'\n');
                        break;
                    }
                    out.push(byte);
                }
                Some("") => {
                    if byte == b'\n' {
                        out.push(b'\n');
                        break;
                    }
                    if byte == b'\r' {
                        if let Some(next) = file_read_byte(pending_byte, file)? {
                            if next == b'\n' {
                                out.push(b'\r');
                                out.push(b'\n');
                                break;
                            }
                            file_unread_byte(pending_byte, next);
                        }
                        out.push(b'\r');
                        break;
                    }
                    out.push(byte);
                }
                Some("\n") => {
                    out.push(byte);
                    if byte == b'\n' {
                        break;
                    }
                }
                Some("\r") => {
                    out.push(byte);
                    if byte == b'\r' {
                        break;
                    }
                }
                Some("\r\n") => {
                    if byte == b'\r' {
                        if let Some(next) = file_read_byte(pending_byte, file)? {
                            if next == b'\n' {
                                out.push(b'\r');
                                out.push(b'\n');
                                break;
                            }
                            file_unread_byte(pending_byte, next);
                        }
                    }
                    out.push(byte);
                }
                Some(_) => {
                    out.push(byte);
                }
            }
        } else {
            out.push(byte);
            if byte == b'\n' {
                break;
            }
        }
    }
    Ok(out)
}

#[no_mangle]
pub extern "C" fn molt_file_readline(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let text = handle.text;
            let newline_owned = if text {
                handle.newline.clone()
            } else {
                Some("\n".to_string())
            };
            let newline = newline_owned.as_deref();
            let mut pending_byte = handle.pending_byte.take();
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let bytes = match file_readline_bytes(&mut pending_byte, file, newline, text, size) {
                Ok(bytes) => bytes,
                Err(_) => {
                    handle.pending_byte = pending_byte;
                    return raise_exception::<_>(_py, "OSError", "read failed");
                }
            };
            handle.pending_byte = pending_byte;
            if text {
                let errors = handle.errors.as_deref().unwrap_or("strict");
                if let Err(msg) = validate_error_handler(errors) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                let encoding = text_encoding_kind(encoding_label);
                let text = match decode_text_with_errors(&bytes, encoding, errors) {
                    Ok(text) => text,
                    Err(err) => {
                        let msg = format!(
                        "'{encoding_label}' codec can't decode byte 0x{:02x} in position {}: {}",
                        err.byte, err.pos, err.message
                    );
                        return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                    }
                };
                let out_ptr = alloc_string(_py, text.as_bytes());
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let out_ptr = alloc_bytes(_py, &bytes);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_readlines(handle_bits: u64, hint_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let hint_obj = obj_from_bits(hint_bits);
            let hint = if hint_obj.is_none() {
                None
            } else {
                match to_i64(hint_obj) {
                    Some(val) if val <= 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, hint_bits));
                        let msg = format!("argument should be integer or None, not '{type_name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let text = handle.text;
            let newline_owned = if text {
                handle.newline.clone()
            } else {
                Some("\n".to_string())
            };
            let newline = newline_owned.as_deref();
            let mut pending_byte = handle.pending_byte.take();
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let mut lines: Vec<u64> = Vec::new();
            let mut total = 0usize;
            loop {
                let bytes = match file_readline_bytes(&mut pending_byte, file, newline, text, None)
                {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        handle.pending_byte = pending_byte;
                        return raise_exception::<_>(_py, "OSError", "read failed");
                    }
                };
                if bytes.is_empty() {
                    break;
                }
                total = total.saturating_add(bytes.len());
                if text {
                    let errors = handle.errors.as_deref().unwrap_or("strict");
                    if let Err(msg) = validate_error_handler(errors) {
                        return raise_exception::<_>(_py, "LookupError", &msg);
                    }
                    let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                    let encoding = text_encoding_kind(encoding_label);
                    let text = match decode_text_with_errors(&bytes, encoding, errors) {
                        Ok(text) => text,
                        Err(err) => {
                            let msg = format!(
                            "'{encoding_label}' codec can't decode byte 0x{:02x} in position {}: {}",
                            err.byte, err.pos, err.message
                        );
                            return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                        }
                    };
                    let line_ptr = alloc_string(_py, text.as_bytes());
                    if line_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    lines.push(MoltObject::from_ptr(line_ptr).bits());
                } else {
                    let line_ptr = alloc_bytes(_py, &bytes);
                    if line_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    lines.push(MoltObject::from_ptr(line_ptr).bits());
                }
                if let Some(limit) = hint {
                    if total >= limit {
                        break;
                    }
                }
            }
            handle.pending_byte = pending_byte;
            let list_ptr = alloc_list(_py, lines.as_slice());
            if list_ptr.is_null() {
                for bits in lines {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            for bits in lines {
                dec_ref_bits(_py, bits);
            }
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

fn file_readinto_impl(_py: &PyToken<'_>, handle_bits: u64, buffer_bits: u64, name: &str) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>(_py, "TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(_py, handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        }
        if !handle.readable {
            return raise_exception::<_>(_py, "UnsupportedOperation", "read");
        }
        if handle.text {
            let msg = format!("{name}() unsupported for text files");
            return raise_exception::<_>(_py, "OSError", &msg);
        }
        let mut export = BufferExport {
            ptr: 0,
            len: 0,
            readonly: 0,
            stride: 0,
            itemsize: 0,
        };
        if molt_buffer_export(buffer_bits, &mut export) != 0 || export.readonly != 0 {
            let msg = format!("{name}() argument must be a writable bytes-like object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if export.itemsize != 1 || export.stride != 1 {
            let msg = format!("{name}() argument must be a writable bytes-like object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let len = export.len as usize;
        if len == 0 {
            return MoltObject::from_int(0).bits();
        }
        let buf = std::slice::from_raw_parts_mut(export.ptr as *mut u8, len);
        let mut guard = handle.state.file.lock().unwrap();
        let Some(file) = guard.as_mut() else {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        };
        let n = match file.read(buf) {
            Ok(n) => n,
            Err(_) => return raise_exception::<_>(_py, "OSError", "read failed"),
        };
        MoltObject::from_int(n as i64).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_file_readinto(handle_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        file_readinto_impl(_py, handle_bits, buffer_bits, "readinto")
    })
}

#[no_mangle]
pub extern "C" fn molt_file_readinto1(handle_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        file_readinto_impl(_py, handle_bits, buffer_bits, "readinto1")
    })
}

#[no_mangle]
pub extern "C" fn molt_file_detach(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            if handle.detached {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    file_handle_detached_message(handle),
                );
            }
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if handle.text {
                let buffer_bits = handle.buffer_bits;
                if buffer_bits == 0 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        file_handle_detached_message(handle),
                    );
                }
                let buffer_obj = obj_from_bits(buffer_bits);
                if let Some(buffer_ptr) = buffer_obj.as_ptr() {
                    if object_type_id(buffer_ptr) == TYPE_ID_FILE_HANDLE {
                        let buffer_handle_ptr = file_handle_ptr(buffer_ptr);
                        if !buffer_handle_ptr.is_null() {
                            let buffer_handle = &mut *buffer_handle_ptr;
                            buffer_handle.pending_byte = handle.pending_byte.take();
                        }
                    }
                }
                handle.buffer_bits = MoltObject::none().bits();
                handle.detached = true;
                handle.owns_fd = false;
                return buffer_bits;
            }
            let raw_ptr = alloc_file_handle_with_state(
                _py,
                Arc::clone(&handle.state),
                handle.readable,
                handle.writable,
                false,
                handle.closefd,
                handle.owns_fd,
                handle.line_buffering,
                handle.write_through,
                handle.buffer_size,
                handle.class_bits,
                handle.name_bits,
                handle.mode.clone(),
                None,
                None,
                None,
                0,
            );
            if raw_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let raw_handle_ptr = file_handle_ptr(raw_ptr);
            if !raw_handle_ptr.is_null() {
                let raw_handle = &mut *raw_handle_ptr;
                raw_handle.pending_byte = handle.pending_byte.take();
            }
            handle.detached = true;
            handle.owns_fd = false;
            MoltObject::from_ptr(raw_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_reconfigure(
    handle_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    line_buffering_bits: u64,
    write_through_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.text {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not a text file");
            }
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if file.flush().is_err() {
                return raise_exception::<_>(_py, "OSError", "flush failed");
            }
            drop(guard);

            let missing = missing_bits(_py);
            let mut new_encoding = handle.encoding.clone();
            if encoding_bits != missing {
                if let Some(encoding) = reconfigure_arg_type(_py, encoding_bits, "encoding") {
                    let (label, _kind) = match normalize_text_encoding(&encoding) {
                        Ok(val) => val,
                        Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
                    };
                    new_encoding = Some(label);
                }
            }
            let mut new_errors = handle.errors.clone();
            if errors_bits != missing {
                if let Some(errors) = reconfigure_arg_type(_py, errors_bits, "errors") {
                    new_errors = Some(errors);
                }
            }
            let mut new_newline = handle.newline.clone();
            if newline_bits != missing {
                new_newline = reconfigure_arg_newline(_py, newline_bits);
            }
            let mut new_line_buffering = handle.line_buffering;
            if line_buffering_bits != missing {
                let obj = obj_from_bits(line_buffering_bits);
                if !obj.is_none() {
                    let val = match to_i64(obj) {
                        Some(val) => val != 0,
                        None => {
                            let type_name =
                                class_name_for_error(type_of_bits(_py, line_buffering_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    new_line_buffering = val;
                }
            }
            let mut new_write_through = handle.write_through;
            if write_through_bits != missing {
                let obj = obj_from_bits(write_through_bits);
                if !obj.is_none() {
                    let val = match to_i64(obj) {
                        Some(val) => val != 0,
                        None => {
                            let type_name =
                                class_name_for_error(type_of_bits(_py, write_through_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    new_write_through = val;
                }
            }

            handle.encoding = new_encoding;
            handle.errors = new_errors;
            if newline_bits != missing {
                handle.pending_byte = None;
            }
            handle.newline = new_newline;
            handle.line_buffering = new_line_buffering;
            handle.write_through = new_write_through;
            MoltObject::none().bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_seek(handle_bits: u64, offset_bits: u64, whence_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let offset = match to_i64(obj_from_bits(offset_bits)) {
                Some(val) => val,
                None => {
                    let type_name = class_name_for_error(type_of_bits(_py, offset_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let whence = match to_i64(obj_from_bits(whence_bits)) {
                Some(val) => val,
                None => {
                    let type_name = class_name_for_error(type_of_bits(_py, whence_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text && whence == 0 {
                let (pos, pending) = match text_cookie_decode(offset) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
                };
                let pos = match file.seek(std::io::SeekFrom::Start(pos)) {
                    Ok(pos) => pos,
                    Err(_) => return raise_exception::<_>(_py, "OSError", "seek failed"),
                };
                handle.pending_byte = pending;
                let cookie = match text_cookie_encode(pos, pending) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
                };
                return MoltObject::from_int(cookie).bits();
            }
            let from = match whence {
                0 => {
                    if offset < 0 {
                        let msg = format!("negative seek position {offset}");
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    std::io::SeekFrom::Start(offset as u64)
                }
                1 => std::io::SeekFrom::Current(offset),
                2 => std::io::SeekFrom::End(offset),
                _ => return raise_exception::<_>(_py, "ValueError", "invalid whence"),
            };
            let pos = match file.seek(from) {
                Ok(pos) => pos,
                Err(_) => return raise_exception::<_>(_py, "OSError", "seek failed"),
            };
            handle.pending_byte = None;
            if handle.text {
                let cookie = match text_cookie_encode(pos, None) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
                };
                MoltObject::from_int(cookie).bits()
            } else {
                MoltObject::from_int(pos as i64).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_tell(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let pos = match file.stream_position() {
                Ok(pos) => pos,
                Err(_) => return raise_exception::<_>(_py, "OSError", "tell failed"),
            };
            if handle.text {
                let cookie = match text_cookie_encode(pos, handle.pending_byte) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
                };
                MoltObject::from_int(cookie).bits()
            } else {
                MoltObject::from_int(pos as i64).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_fileno(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_ref() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                MoltObject::from_int(file.as_raw_fd() as i64).bits()
            }
            #[cfg(windows)]
            {
                // TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial):
                // return CRT fd on Windows instead of raw handle for fileno parity.
                use std::os::windows::io::AsRawHandle;
                MoltObject::from_int(file.as_raw_handle() as i64).bits()
            }
            #[cfg(not(any(unix, windows)))]
            {
                return raise_exception::<_>(
                    _py,
                    "OSError",
                    "fileno is unsupported on this platform",
                );
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_truncate(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "truncate");
            }
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let size = if obj_from_bits(size_bits).is_none() {
                match file.stream_position() {
                    Ok(pos) => pos,
                    Err(_) => return raise_exception::<_>(_py, "OSError", "tell failed"),
                }
            } else {
                let val = match to_i64(obj_from_bits(size_bits)) {
                    Some(val) => val,
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if val < 0 {
                    return raise_exception::<_>(_py, "OSError", "Invalid argument");
                }
                val as u64
            };
            if file.set_len(size).is_err() {
                return raise_exception::<_>(_py, "OSError", "truncate failed");
            }
            MoltObject::from_int(size as i64).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_readable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            MoltObject::from_bool(handle.readable).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_writable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            MoltObject::from_bool(handle.writable).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_seekable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let seekable = file.stream_position().is_ok();
            MoltObject::from_bool(seekable).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_isatty(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_ref() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                let isatty = libc::isatty(file.as_raw_fd()) == 1;
                MoltObject::from_bool(isatty).bits()
            }
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                let handle = file.as_raw_handle();
                let isatty = windows_handle_isatty(handle as *mut std::ffi::c_void);
                MoltObject::from_bool(isatty).bits()
            }
            #[cfg(not(any(unix, windows)))]
            {
                let _ = file;
                MoltObject::from_bool(false).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_iter(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
        }
        inc_ref_bits(_py, handle_bits);
        handle_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_file_next(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let line_bits = molt_file_readline(handle_bits, MoltObject::from_int(-1).bits());
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let line_obj = obj_from_bits(line_bits);
        let empty = if let Some(ptr) = line_obj.as_ptr() {
            unsafe {
                match object_type_id(ptr) {
                    TYPE_ID_STRING => string_len(ptr) == 0,
                    TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => bytes_len(ptr) == 0,
                    _ => false,
                }
            }
        } else {
            false
        };
        if empty {
            dec_ref_bits(_py, line_bits);
            return raise_exception::<_>(_py, "StopIteration", "");
        }
        line_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_file_enter(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            file_handle_enter(_py, ptr)
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_exit(handle_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            file_handle_exit(_py, ptr, exc_bits)
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_exit_method(
    handle_bits: u64,
    _exc_type_bits: u64,
    exc_bits: u64,
    _tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { molt_file_exit(handle_bits, exc_bits) })
}

#[no_mangle]
pub extern "C" fn molt_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not writable");
            }
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let data_obj = obj_from_bits(data_bits);
            let (bytes, written_len): (Vec<u8>, usize) = if handle.text {
                let text = match string_obj_to_owned(data_obj) {
                    Some(text) => text,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "write expects str for text mode",
                        )
                    }
                };
                let errors = handle.errors.as_deref().unwrap_or("strict");
                let newline = handle.newline.as_deref();
                if let Err(msg) = validate_error_handler(errors) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let translated = translate_write_newlines(&text, newline);
                let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                let encoding = text_encoding_kind(encoding_label);
                let bytes = match encode_text_with_errors(&translated, encoding, errors) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        let msg = format!(
                        "'{encoding_label}' codec can't encode character '{}' in position {}: {}",
                        err.ch, err.pos, err.message
                    );
                        return raise_exception::<_>(_py, "UnicodeEncodeError", &msg);
                    }
                };
                (bytes, text.chars().count())
            } else {
                let Some(data_ptr) = data_obj.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects bytes or bytearray",
                    );
                };
                let type_id = object_type_id(data_ptr);
                if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects bytes or bytearray",
                    );
                }
                let len = bytes_len(data_ptr);
                let raw = std::slice::from_raw_parts(bytes_data(data_ptr), len);
                (raw.to_vec(), len)
            };
            if file.write_all(&bytes).is_err() {
                return raise_exception::<_>(_py, "OSError", "write failed");
            }
            let should_flush =
                handle.write_through || (handle.line_buffering && bytes.contains(&b'\n'));
            if should_flush && file.flush().is_err() {
                return raise_exception::<_>(_py, "OSError", "flush failed");
            }
            MoltObject::from_int(written_len as i64).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_file_writelines(handle_bits: u64, lines_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not writable");
            }
        }
        let iter_bits = molt_iter(lines_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "writelines() argument must be iterable",
            );
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let line_bits = elems[0];
                let _ = molt_file_write(handle_bits, line_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_file_flush(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let mut guard = handle.state.file.lock().unwrap();
            let Some(file) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if file.flush().is_err() {
                return raise_exception::<_>(_py, "OSError", "flush failed");
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_file_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
        }
        file_handle_close_ptr(ptr);
        MoltObject::none().bits()
    })
}
