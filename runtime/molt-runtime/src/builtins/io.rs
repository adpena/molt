use crate::PyToken;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(windows)]
use crate::windows_abi::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, FILE_NAME_NORMALIZED, FILE_TYPE_CHAR,
    GetConsoleMode, GetCurrentProcess, GetFileType, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
};

// Re-export path/glob/os functions so that `io::*` includes them
#[allow(unused_imports)]
pub use super::io_path::*;
pub(crate) use super::io_path_utils::*;
use crate::object::{
    MoltFileBackend, MoltMemoryBackend, MoltTextBackend, NEWLINE_KIND_CR, NEWLINE_KIND_CRLF,
    NEWLINE_KIND_LF,
};
use crate::*;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Seek, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[path = "io/buffered.rs"]
mod buffered;
#[path = "io/buffered_reader.rs"]
mod buffered_reader;
#[path = "io/raw_base.rs"]
mod raw_base;
pub(crate) use raw_base::{raw_io_base_read, raw_io_base_readall, raw_io_base_readinto};

#[path = "io/buffered_writer.rs"]
mod buffered_writer;

#[path = "io/buffer.rs"]
mod buffer;
#[cfg(test)]
use buffer::file_remaining_bytes_hint;
use buffer::{
    backend_flush, backend_read_bytes, backend_seek, backend_tell, backend_truncate,
    backend_write_bytes, buffered_read_bytes, buffered_read_into, clear_read_buffer,
    clear_write_buffer, file_read1_bytes, flush_write_buffer, handle_read_byte, host_file_seek,
    memory_backend_bits, memory_backend_vec_ref, prepend_read_buffer, rewind_read_buffer,
    unread_bytes,
};
#[path = "io/handle.rs"]
mod handle;
use handle::{
    VfsWritebackEntry, alloc_file_handle_with_state, file_handle_mode, file_handle_physical_state,
    file_handle_raw_bits, resolve_file_handle_ptr, vfs_writeback_register, vfs_writeback_take,
};
pub(crate) use handle::{
    file_handle_close_detached, file_handle_close_ptr, file_handle_detached_message,
    file_handle_is_closed, file_handle_require_attached,
};
#[path = "io/text.rs"]
mod text;
use text::*;
#[path = "io/open.rs"]
mod open;
pub(crate) use open::dup_fd;
#[cfg(windows)]
use open::windows_handle_isatty;
pub use open::{
    molt_file_io_init, molt_file_io_new, molt_file_open, molt_file_open_ex, molt_open_builtin,
    molt_sys_stderr, molt_sys_stdin, molt_sys_stdout,
};
use open::{open_arg_newline, reconfigure_arg_newline, reconfigure_arg_type};
#[cfg(windows)]
pub(crate) use open::{windows_crt_fd_from_handle, windows_path_from_handle};
#[path = "io/construct.rs"]
mod construct;
pub(crate) use construct::{buffered_reader_new_method, buffered_writer_new_method};
pub use construct::{
    molt_buffered_init, molt_buffered_new, molt_bytesio_init, molt_bytesio_new, molt_io_class,
    molt_io_default_buffer_size, molt_io_text_encoding, molt_stringio_init, molt_stringio_new,
    molt_text_io_wrapper_init, molt_text_io_wrapper_new,
};
#[path = "io/read.rs"]
mod read;
pub use read::{
    molt_file_getbuffer, molt_file_getvalue, molt_file_peek, molt_file_read, molt_file_read1,
    molt_file_readall, molt_file_readinto, molt_file_readinto1, molt_file_readline,
    molt_file_readlines,
};
#[path = "io/write.rs"]
mod write;
pub(crate) use write::{io_base_close, io_base_flush};
pub use write::{molt_file_close, molt_file_flush, molt_file_write, molt_file_writelines};
#[path = "io/control.rs"]
mod control;
pub(crate) use control::{io_base_check_closed, io_base_exit, io_base_finalize};
pub use control::{
    molt_file_detach, molt_file_enter, molt_file_exit, molt_file_exit_method, molt_file_fileno,
    molt_file_isatty, molt_file_iter, molt_file_next, molt_file_readable, molt_file_reconfigure,
    molt_file_seek, molt_file_seekable, molt_file_tell, molt_file_truncate, molt_file_writable,
};

#[path = "io/attributes.rs"]
mod attributes;
pub(crate) use attributes::io_publish_members;
pub use attributes::molt_io_member_get;

// IOBase's marker is an ordinary attribute: its presence, even with a false
// value, closes the base protocol independently of a concrete stream's backend.
fn io_base_is_closed(py: &PyToken<'_>, object: u64) -> Result<bool, u64> {
    let Some(name) = attr_name_bits_from_bytes(py, b"__IOBase_closed") else {
        return Err(MoltObject::none().bits());
    };
    let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
    let value = crate::builtins::platform::getattr_optional_bits(py, object, name)?;
    if let Some(value) = value {
        dec_ref_bits(py, value);
        Ok(true)
    } else {
        Ok(false)
    }
}

// Ordinary attribute lookup deliberately honors instance and subclass overrides.
// The callable is owned, and no file backing borrow or lock crosses the call.
fn io_call_noargs(py: &PyToken<'_>, object: u64, name: &[u8]) -> u64 {
    let Some(name) = attr_name_bits_from_bytes(py, name) else {
        return MoltObject::none().bits();
    };
    let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
    let callable = molt_getattr_builtin(object, name, missing_bits(py));
    let _callable = obj_from_bits(callable).as_ptr().map(PtrDropGuard::new);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    unsafe { call_callable0(py, callable) }
}

// Close has two fallible phases. The second failure owns the raised indicator
// and chains the first; otherwise restore the first error after retirement.
fn io_chain_close_error(py: &PyToken<'_>, previous: crate::builtins::exceptions::RaisedSnapshot) {
    use crate::builtins::exceptions::{
        ExceptionFieldSlot, RaisedSnapshot, exception_replace_field_bits, resolve_raised,
        take_raised,
    };
    let prior = match previous {
        RaisedSnapshot::None => return,
        RaisedSnapshot::Thread(bits) | RaisedSnapshot::Task(_, bits) => bits,
    };
    let current = take_raised(py);
    match current {
        RaisedSnapshot::None => resolve_raised(py, &mut Some(previous)),
        RaisedSnapshot::Thread(bits) | RaisedSnapshot::Task(_, bits) => {
            // CPython's close chain retains even an explicitly repeated error
            // as its own context; both values are canonical raised owners.
            exception_replace_field_bits(py, bits, ExceptionFieldSlot::Context, prior)
                .expect("raised IO errors must admit exception context");
            resolve_raised(py, &mut Some(current));
            dec_ref_bits(py, prior);
        }
    }
}

// CPython Modules/_io/_iomodule.h increased this default in Python 3.14.
// Native constructors, open/stdio and both Python facades share this authority.
pub(crate) fn default_buffer_size(py: &PyToken<'_>) -> i64 {
    if crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
        128 * 1024
    } else {
        8 * 1024
    }
}

pub(crate) struct IoRuntimeState {
    pub(crate) sys_stdin_handle_bits: AtomicU64,
    pub(crate) sys_stdout_handle_bits: AtomicU64,
    pub(crate) sys_stderr_handle_bits: AtomicU64,
    vfs_writebacks: Mutex<HashMap<usize, VfsWritebackEntry>>,
}

impl IoRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            sys_stdin_handle_bits: AtomicU64::new(0),
            sys_stdout_handle_bits: AtomicU64::new(0),
            sys_stderr_handle_bits: AtomicU64::new(0),
            vfs_writebacks: Mutex::new(HashMap::new()),
        }
    }

    fn stdio_slots(&self) -> [&AtomicU64; 3] {
        [
            &self.sys_stdin_handle_bits,
            &self.sys_stdout_handle_bits,
            &self.sys_stderr_handle_bits,
        ]
    }
}

pub(crate) fn io_clear_runtime_state(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::gil_assert();
    let handles = state
        .io
        .stdio_slots()
        .map(|slot| slot.swap(0, Ordering::AcqRel));
    let writebacks = {
        let mut registry = state.io.vfs_writebacks.lock().unwrap();
        std::mem::take(&mut *registry)
    };
    let changed = handles.iter().any(|&bits| bits != 0) || !writebacks.is_empty();
    // Detach the entire owner cohort before flush/decref or backend destruction
    // can publish a replacement for the next shutdown fixed-point pass.
    for bits in handles {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            let _ = molt_file_flush(bits);
            if exception_pending(_py) {
                clear_exception(_py);
            }
            dec_ref_bits(_py, bits);
        }
    }
    drop(writebacks);
    changed
}

/// One selected mount route. A mounted miss never retries the host filesystem.
pub(crate) type VfsPathRoute = (String, Arc<dyn crate::vfs::VfsBackend>, String);

pub(crate) fn selected_vfs_route(
    py: &PyToken<'_>,
    path: &std::path::Path,
) -> Result<Option<VfsPathRoute>, u64> {
    let Some(vfs) = runtime_state(py).get_vfs() else {
        return Ok(None);
    };
    if let Some(path) = path.to_str() {
        return Ok(vfs.resolve(path));
    }
    // The VFS namespace is UTF-8, while native POSIX paths are arbitrary bytes.
    // Normalize in bytes before selecting a mount; never replace invalid bytes
    // with U+FFFD and accidentally select a different mounted filename.
    let raw = molt_runtime_platform::path_text::path_normpath_raw(
        path.as_os_str().as_encoded_bytes(),
        b'/',
    );
    match std::str::from_utf8(&raw) {
        Ok(path) => Ok(vfs.resolve(path)),
        Err(error) => {
            let prefix =
                std::str::from_utf8(&raw[..error.valid_up_to()]).expect("valid UTF-8 prefix");
            if vfs.resolve(prefix).is_some() {
                Err(raise_exception::<u64>(
                    py,
                    "UnicodeError",
                    "mounted VFS paths must be valid UTF-8",
                ))
            } else {
                Ok(None)
            }
        }
    }
}

fn admit_metadata_routes(
    py: &PyToken<'_>,
    operation: OperationId,
    routes: [Option<&VfsPathRoute>; 2],
) -> Result<(), u64> {
    let mut capabilities: [&'static str; 4] = [""; 4];
    let mut count = 0;
    for (prefix, _, _) in routes.into_iter().flatten() {
        let requirement = crate::vfs::caps::mount_requirements(prefix, true, false)
            .map_err(|error| raise_exception::<u64>(py, "PermissionError", &error.to_string()))?;
        for &capability in requirement.capabilities() {
            capabilities[count] = capability;
            count += 1;
        }
    }
    crate::require_operation_with_capabilities(
        py,
        operation,
        crate::audit::AuditArgs::None,
        &capabilities[..count],
    )
}

/// Canonical POSIX errno after the host/VFS boundary. Native Windows errors
/// cross the existing Win32 mapping exactly once; VFS errors are already errno.
#[derive(Debug)]
pub(crate) struct PathIoError {
    pub errno: i32,
    message: String,
}
impl PathIoError {
    pub(crate) fn canonical(errno: i32, message: &str) -> Self {
        Self {
            errno,
            message: message.into(),
        }
    }
    pub(crate) fn from_native(error: std::io::Error) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let raw = error.raw_os_error();
        #[cfg(windows)]
        let errno = raw
            .map(crate::windows_abi::winerror_to_errno)
            .unwrap_or(libc::EIO);
        #[cfg(target_arch = "wasm32")]
        let errno = molt_runtime_platform::synthetic_errno::from_io_error(&error).code();
        #[cfg(all(not(windows), not(target_arch = "wasm32")))]
        let errno = raw.unwrap_or(libc::EIO);
        Self {
            errno,
            message: error.to_string(),
        }
    }
    pub(crate) fn raise(&self, py: &PyToken<'_>, operation: &str) -> u64 {
        raise_os_error_errno::<u64>(
            py,
            i64::from(self.errno),
            &format!("{operation}: {}", self.message),
        )
    }
}

fn metadata_vfs_error(error: crate::vfs::VfsError) -> PathIoError {
    let message = error.to_string();
    let errno = match error {
        crate::vfs::VfsError::NotFound => libc::ENOENT,
        crate::vfs::VfsError::PermissionDenied => libc::EACCES,
        crate::vfs::VfsError::ReadOnly => libc::EROFS,
        crate::vfs::VfsError::NotDirectory => libc::ENOTDIR,
        crate::vfs::VfsError::IsDirectory => libc::EISDIR,
        crate::vfs::VfsError::AlreadyExists => libc::EEXIST,
        _ => libc::EIO,
    };
    PathIoError { errno, message }
}

fn metadata_on_route(
    path: &std::path::Path,
    route: Option<&VfsPathRoute>,
    follow: bool,
) -> Result<molt_runtime_platform::stat_support::PathMetadata, PathIoError> {
    use molt_runtime_platform::stat_support::PathMetadata;
    match route {
        Some((_, backend, relative)) => backend
            .stat(relative)
            .map(|value| {
                PathMetadata::from_virtual(
                    value.is_file,
                    value.is_dir,
                    value.readonly,
                    value.size,
                    value.mtime,
                )
            })
            .map_err(metadata_vfs_error),
        None => {
            #[cfg(windows)]
            {
                PathMetadata::from_windows_path(path, follow).map_err(PathIoError::from_native)
            }
            #[cfg(not(windows))]
            {
                if follow {
                    std::fs::metadata(path)
                } else {
                    std::fs::symlink_metadata(path)
                }
                .map(|metadata| PathMetadata::from_host(&metadata))
                .map_err(PathIoError::from_native)
            }
        }
    }
}

fn validate_metadata_path(py: &PyToken<'_>, path: &std::path::Path) -> Result<(), u64> {
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        Err(raise_exception::<u64>(
            py,
            "ValueError",
            "embedded null byte",
        ))
    } else {
        Ok(())
    }
}

/// Admission errors remain outside the OS-result domain: predicate suppression
/// may consume a filesystem failure, never a missing capability.
pub(crate) fn path_metadata(
    py: &PyToken<'_>,
    path: &std::path::Path,
    follow: bool,
    operation: OperationId,
) -> Result<Result<molt_runtime_platform::stat_support::PathMetadata, PathIoError>, u64> {
    validate_metadata_path(py, path)?;
    let route = selected_vfs_route(py, path)?;
    admit_metadata_routes(py, operation, [route.as_ref(), None])?;
    Ok(metadata_on_route(path, route.as_ref(), follow))
}

pub(crate) fn path_read_link(
    py: &PyToken<'_>,
    path: &std::path::Path,
    operation: OperationId,
) -> Result<Result<std::path::PathBuf, PathIoError>, u64> {
    validate_metadata_path(py, path)?;
    let route = selected_vfs_route(py, path)?;
    admit_metadata_routes(py, operation, [route.as_ref(), None])?;
    Ok(match route {
        Some((_, backend, relative)) => match backend.stat(&relative) {
            Ok(_) => Err(PathIoError {
                errno: libc::EINVAL,
                message: "not a symbolic link".into(),
            }),
            Err(error) => Err(metadata_vfs_error(error)),
        },
        None => std::fs::read_link(path).map_err(PathIoError::from_native),
    })
}

pub(crate) fn paths_same_file(
    py: &PyToken<'_>,
    left: &std::path::Path,
    right: &std::path::Path,
    operation: OperationId,
) -> Result<Result<bool, PathIoError>, u64> {
    validate_metadata_path(py, left)?;
    validate_metadata_path(py, right)?;
    let left_route = selected_vfs_route(py, left)?;
    let right_route = selected_vfs_route(py, right)?;
    admit_metadata_routes(py, operation, [left_route.as_ref(), right_route.as_ref()])?;
    let compare = || {
        let left_metadata = metadata_on_route(left, left_route.as_ref(), true)?;
        let right_metadata = metadata_on_route(right, right_route.as_ref(), true)?;
        match (&left_route, &right_route) {
            (Some((_, left_backend, left_name)), Some((_, right_backend, right_name))) => {
                Ok(Arc::ptr_eq(left_backend, right_backend) && left_name == right_name)
            }
            (None, None) => {
                #[cfg(unix)]
                {
                    Ok(left_metadata.fields[1..3] == right_metadata.fields[1..3])
                }
                #[cfg(not(unix))]
                {
                    let _ = (left_metadata, right_metadata);
                    Ok(
                        std::fs::canonicalize(left).map_err(PathIoError::from_native)?
                            == std::fs::canonicalize(right).map_err(PathIoError::from_native)?,
                    )
                }
            }
            _ => Ok(false),
        }
    };
    Ok(compare())
}

fn raise_vfs_error(py: &PyToken<'_>, error: crate::vfs::VfsError, path: &str) -> u64 {
    let message = format!("{error}: '{path}'");
    let kind = match error {
        crate::vfs::VfsError::NotFound => "FileNotFoundError",
        crate::vfs::VfsError::AlreadyExists => "FileExistsError",
        crate::vfs::VfsError::PermissionDenied | crate::vfs::VfsError::ReadOnly => {
            "PermissionError"
        }
        crate::vfs::VfsError::IsDirectory => "IsADirectoryError",
        _ => "OSError",
    };
    raise_exception::<u64>(py, kind, &message)
}

#[cfg(test)]
#[path = "io/tests.rs"]
mod tests;
