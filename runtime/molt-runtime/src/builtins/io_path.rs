//! Path, glob, and OS filesystem operations.
//!
//! Split from io.rs to reduce file size. Contains all `molt_path_*`,
//! `molt_glob*` and `molt_os_*` extern functions.

use crate::BuiltinExc;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

#[cfg(target_arch = "wasm32")]
use super::io::glob_dir_fd_root_path;
use super::io::{
    PathFlavor, alloc_path_list_bits, create_symlink_path, dup_fd, glob_dir_fd_arg_from_bits,
    glob_iter_alloc_object, glob_iter_new_state, glob_translate_text, path_abspath_text,
    path_as_uri_text, path_basename_text, path_dirname_text, path_from_bits,
    path_from_bits_with_flavor, path_glob_matches, path_isabs_text, path_match_text,
    path_normpath_text, path_relpath_text, path_sep_char, path_splitext_text,
    path_str_arg_from_bits, path_string_from_bits, path_value_with_flavor_from_bits,
};
use crate::PyToken;
use crate::audit::{AuditArgs, audit_capability_decision};
use crate::builtins::compatibility_error::CompatibilityError;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::object::ops_format::string_obj_bytes;
use crate::randomness::fill_os_random;
#[cfg(windows)]
use crate::windows_abi::{
    GetHandleInformation, HANDLE_FLAG_INHERIT, SetHandleInformation, WSAENOTSOCK, WSAGetLastError,
    closesocket,
};
use crate::*;
use std::io::ErrorKind;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

enum MetadataInput {
    Path(std::path::PathBuf),
    Fd(libc::c_int),
}

fn metadata_input(py: &PyToken<'_>, bits: u64, allow_fd: bool) -> Result<MetadataInput, u64> {
    let path_storage = obj_from_bits(bits).as_ptr().is_some_and(|pointer| unsafe {
        matches!(object_type_id(pointer), TYPE_ID_STRING | TYPE_ID_BYTES)
    });
    if allow_fd && !path_storage {
        if let Some(index) = crate::builtins::numbers::index_from_number_protocol(py, bits) {
            let _owner = unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), index)
            };
            let Some(value) = crate::builtins::numbers::index_c_int_from_obj(py, index) else {
                return Err(MoltObject::none().bits());
            };
            return Ok(MetadataInput::Fd(value));
        }
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
    }
    let path = path_from_bits(py, bits)?;
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(raise_exception::<u64>(
            py,
            BuiltinExc::ValueError,
            "embedded null byte",
        ));
    }
    Ok(MetadataInput::Path(path))
}

fn descriptor_metadata(
    fd: libc::c_int,
) -> Result<molt_runtime_platform::stat_support::PathMetadata, super::io::PathIoError> {
    if fd < 0 {
        return Err(super::io::PathIoError::canonical(
            libc::EBADF,
            "bad file descriptor",
        ));
    }
    use molt_runtime_platform::stat_support::PathMetadata;
    #[cfg(unix)]
    {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, metadata.as_mut_ptr()) } != 0 {
            return Err(super::io::PathIoError::from_native(
                std::io::Error::last_os_error(),
            ));
        }
        Ok(PathMetadata::from_unix_stat(unsafe {
            metadata.assume_init_ref()
        }))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::FromRawHandle;
        let handle = unsafe { libc::get_osfhandle(fd) };
        if handle == -1 {
            return Err(super::io::PathIoError::canonical(
                libc::EBADF,
                "bad file descriptor",
            ));
        }
        // Borrow the CRT descriptor's handle; never duplicate or close it.
        let file = std::mem::ManuallyDrop::new(unsafe {
            std::fs::File::from_raw_handle(handle as *mut _)
        });
        PathMetadata::from_windows_file(&file).map_err(super::io::PathIoError::from_native)
    }
    #[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
    {
        use std::os::fd::FromRawFd;
        let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
        file.metadata()
            .map(|metadata| PathMetadata::from_host(&metadata))
            .map_err(super::io::PathIoError::from_native)
    }
    #[cfg(not(any(unix, windows, all(target_arch = "wasm32", target_os = "wasi"))))]
    {
        let _ = fd;
        Err(super::io::PathIoError::canonical(
            libc::ENOSYS,
            "descriptor metadata unavailable",
        ))
    }
}

fn metadata_for_input(
    py: &PyToken<'_>,
    input: &MetadataInput,
    follow: bool,
    operation: OperationId,
) -> Result<Result<molt_runtime_platform::stat_support::PathMetadata, super::io::PathIoError>, u64>
{
    match input {
        MetadataInput::Path(path) => super::io::path_metadata(py, path, follow, operation),
        MetadataInput::Fd(fd) => {
            crate::require_operation::<u64>(py, operation, crate::audit::AuditArgs::None)?;
            Ok(descriptor_metadata(*fd))
        }
    }
}

fn suppress_predicate_conversion_error(py: &PyToken<'_>, is_link: bool) -> bool {
    use crate::builtins::exceptions::{ExceptionValue, exception_matches_kind};
    if !exception_pending(py) {
        return false;
    }
    let pending = ExceptionValue::adopt(
        py,
        crate::builtins::exceptions::molt_exception_last_pending(),
    );
    let suppress = exception_matches_kind(py, pending.bits(), crate::BuiltinExc::OSError)
        || exception_matches_kind(py, pending.bits(), crate::BuiltinExc::ValueError)
        || (is_link
            && exception_matches_kind(py, pending.bits(), crate::BuiltinExc::AttributeError));
    if suppress {
        clear_exception(py);
    }
    suppress
}

#[derive(Clone, Copy)]
enum MetadataPredicate {
    Exists,
    Directory,
    RegularFile,
    Symlink,
}

fn metadata_predicate(py: &PyToken<'_>, bits: u64, predicate: MetadataPredicate) -> u64 {
    let is_link = matches!(predicate, MetadataPredicate::Symlink);
    let input = match metadata_input(py, bits, !is_link) {
        Ok(input) => input,
        Err(error) => {
            return if suppress_predicate_conversion_error(py, is_link) {
                MoltObject::from_bool(false).bits()
            } else {
                error
            };
        }
    };
    // Admission is outside genericpath's ordinary error-suppression domain.
    match metadata_for_input(py, &input, !is_link, OperationId::OsStat) {
        Err(error) => error,
        Ok(Err(_)) => MoltObject::from_bool(false).bits(),
        Ok(Ok(metadata)) => MoltObject::from_bool(match predicate {
            MetadataPredicate::Exists => true,
            MetadataPredicate::Directory => metadata.is_dir(),
            MetadataPredicate::RegularFile => metadata.is_file(),
            MetadataPredicate::Symlink => metadata.is_symlink(),
        })
        .bits(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_exists(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        metadata_predicate(py, path_bits, MetadataPredicate::Exists)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isdir(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        metadata_predicate(py, path_bits, MetadataPredicate::Directory)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isfile(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        metadata_predicate(py, path_bits, MetadataPredicate::RegularFile)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_islink(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        metadata_predicate(py, path_bits, MetadataPredicate::Symlink)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_readlink(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let (path, flavor) = match path_from_bits_with_flavor(py, path_bits) {
            Ok(input) => input,
            Err(error) => return error,
        };
        match super::io::path_read_link(py, &path, OperationId::OsReadlink) {
            Err(error) => error,
            Ok(Ok(target)) => super::io::path_result_bits(py, &target, flavor),
            Ok(Err(error)) => error.raise(py, "readlink"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_symlink(
    src_bits: u64,
    dst_bits: u64,
    target_is_directory_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let target_is_directory = is_truthy(_py, obj_from_bits(target_is_directory_bits));
        match create_symlink_path(&src, &dst, target_is_directory) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => {
                        raise_exception::<_>(_py, BuiltinExc::FileExistsError, &msg)
                    }
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                    }
                    ErrorKind::Unsupported => {
                        raise_exception::<_>(_py, BuiltinExc::NotImplementedError, &msg)
                    }
                    _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_mkdir(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        // pathlib.Path.mkdir passes `mode` to os.mkdir: CPython's `int` converter.
        let Some(mode) = crate::builtins::numbers::index_c_int_from_obj(_py, mode_bits) else {
            return MoltObject::none().bits();
        };
        let mode = mode as u32;
        #[cfg(unix)]
        let builder = {
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(mode);
            builder
        };
        #[cfg(not(unix))]
        let builder = {
            let _ = mode;
            std::fs::DirBuilder::new()
        };
        match builder.create(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => {
                        raise_exception::<_>(_py, BuiltinExc::FileExistsError, &msg)
                    }
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                    }
                    _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_unlink(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        match std::fs::remove_file(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                    }
                    ErrorKind::IsADirectory => {
                        raise_exception::<_>(_py, BuiltinExc::IsADirectoryError, &msg)
                    }
                    _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_rmdir(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        match std::fs::remove_dir(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                    }
                    ErrorKind::DirectoryNotEmpty => {
                        raise_exception::<_>(_py, BuiltinExc::OSError, &msg)
                    }
                    _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
                }
            }
        }
    })
}

fn path_join_components(py: &PyToken<'_>, base_bits: u64, parts: &[u64], posix: bool) -> u64 {
    use molt_runtime_core::OwnedRuntimeValue;
    use molt_runtime_platform::path_text::PathJoin;

    let (base, flavor) = match super::io::fspath_bits_with_flavor(py, base_bits) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let base = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), base) };
    let classes = builtin_classes(py);
    let exact =
        |bits| matches!(type_of_bits(py, bits), ty if ty == classes.str || ty == classes.bytes);
    let tuple = |items: &[u64]| {
        let ptr = alloc_tuple(py, items);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    };
    if !exact(base.bits()) {
        return tuple(&[
            MoltObject::from_int(0).bits(),
            MoltObject::none().bits(),
            MoltObject::none().bits(),
        ]);
    }
    let sep = if posix { '/' } else { '\\' };
    if parts.is_empty() && sep == '/' {
        return base.into_bits();
    }
    // Owners survive fspath callbacks; the accumulator never borrows Python storage.
    let raw = |bits| unsafe {
        let ptr = obj_from_bits(bits)
            .as_ptr()
            .expect("fspath string/bytes storage");
        if flavor == PathFlavor::Bytes {
            std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr))
        } else {
            std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr))
        }
    };
    let coordinate = if posix {
        if flavor == PathFlavor::Bytes {
            molt_runtime_platform::path_text::PathCoordinate::Bytes
        } else {
            molt_runtime_platform::path_text::PathCoordinate::PythonCodePoints
        }
    } else {
        super::io::path_root_coordinate(py, flavor == PathFlavor::Bytes)
    };
    let allocate = |value: &[u8]| {
        let ptr = if flavor == PathFlavor::Bytes {
            alloc_bytes(py, value)
        } else {
            alloc_string(py, value)
        };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(ptr).bits(),
                )
            })
        }
    };
    let (base_storage, base_root) = match super::io::root_path_parse(
        py,
        raw(base.bits()),
        sep as u8,
        flavor == PathFlavor::Bytes,
        coordinate,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut joined = PathJoin::new(
        base_storage.into_owned(),
        sep as u8,
        flavor == PathFlavor::Str,
        base_root.0,
    );
    // POSIX retains the actual operand through absolute replacement and empty
    // concatenation. A later subclass can observe that identity in __radd__.
    let mut identity = Some(base);
    for (index, &part) in parts.iter().enumerate() {
        let (value, part_flavor) = match super::io::fspath_bits_with_flavor(py, part) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let value = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), value) };
        if !exact(value.bits()) || (sep == '\\' && part_flavor != flavor) {
            // One-way handoff before any subclass operation. os.path.join resumes
            // in its own frame using this already-converted operand exactly once.
            let state = if sep == '/' {
                match identity {
                    Some(value) => value,
                    None => match allocate(&joined.finish()) {
                        Some(value) => value,
                        None => return MoltObject::none().bits(),
                    },
                }
            } else {
                let (drive, root, tail) = joined.into_components();
                let Some(drive) = allocate(&drive) else {
                    return MoltObject::none().bits();
                };
                let Some(root) = allocate(&root) else {
                    return MoltObject::none().bits();
                };
                let Some(tail) = allocate(&tail) else {
                    return MoltObject::none().bits();
                };
                let bits = tuple(&[drive.bits(), root.bits(), tail.bits()]);
                if exception_pending(py) {
                    return bits;
                }
                unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) }
            };
            return tuple(&[
                MoltObject::from_int(index as i64).bits(),
                value.bits(),
                state.bits(),
            ]);
        }
        if part_flavor != flavor {
            return raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "Can't mix strings and bytes in path components",
            );
        }
        let part_raw = raw(value.bits());
        let empty_base = identity
            .as_ref()
            .is_some_and(|value| raw(value.bits()).is_empty());
        let unchanged = part_raw.is_empty()
            && identity
                .as_ref()
                .is_some_and(|value| raw(value.bits()).ends_with(b"/"));
        let (part_storage, part_root) = match super::io::root_path_parse(
            py,
            part_raw,
            sep as u8,
            flavor == PathFlavor::Bytes,
            coordinate,
        ) {
            Ok(value) => value,
            Err(error) => return error,
        };
        joined.push(&part_storage, part_root);
        if sep == '/' && (part_raw.first() == Some(&b'/') || empty_base) {
            identity = Some(value);
        } else if sep != '/' || !unchanged {
            identity = None;
        }
    }
    if sep == '/'
        && let Some(identity) = identity
    {
        return identity.into_bits();
    }
    allocate(&joined.finish()).map_or(MoltObject::none().bits(), |value| value.into_bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_join_many(base_bits: u64, parts_bits: u64, posix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = obj_from_bits(parts_bits).as_ptr() else {
            return raise_exception::<_>(py, BuiltinExc::TypeError, "parts must be tuple");
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
            return raise_exception::<_>(py, BuiltinExc::TypeError, "parts must be tuple");
        }
        // The public variadic tuple is immutable. Retain its owner across fspath
        // callbacks without allocating or retaining a second copy of every item.
        let parts = unsafe { crate::object::seq_access::pin_tuple(py, ptr) }
            .expect("type-checked tuple exposes immutable storage");
        path_join_components(
            py,
            base_bits,
            &parts,
            is_truthy(py, obj_from_bits(posix_bits)),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isabs(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(path_isabs_text(&path, sep)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_dirname(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_dirname_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_basename(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_basename_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_split(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let head = path_dirname_text(&path, sep);
        let tail = path_basename_text(&path, sep);
        let head_ptr = alloc_string(_py, head.as_bytes());
        if head_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let head_bits = MoltObject::from_ptr(head_ptr).bits();
        let tail_ptr = alloc_string(_py, tail.as_bytes());
        if tail_ptr.is_null() {
            dec_ref_bits(_py, head_bits);
            return MoltObject::none().bits();
        }
        let tail_bits = MoltObject::from_ptr(tail_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[head_bits, tail_bits]);
        dec_ref_bits(_py, head_bits);
        dec_ref_bits(_py, tail_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_splitext(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let (root, ext) = path_splitext_text(&path, sep);
        let root_ptr = alloc_string(_py, root.as_bytes());
        if root_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let root_bits = MoltObject::from_ptr(root_ptr).bits();
        let ext_ptr = alloc_string(_py, ext.as_bytes());
        if ext_ptr.is_null() {
            dec_ref_bits(_py, root_bits);
            return MoltObject::none().bits();
        }
        let ext_bits = MoltObject::from_ptr(ext_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[root_bits, ext_bits]);
        dec_ref_bits(_py, root_bits);
        dec_ref_bits(_py, ext_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_normpath(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_normpath_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_abspath(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_abspath_text(_py, &path, sep) {
            Ok(out) => out,
            Err(bits) => return bits,
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_relpath(path_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let start = if obj_from_bits(start_bits).is_none() {
            ".".to_string()
        } else {
            match path_string_from_bits(_py, start_bits) {
                Ok(path) => path,
                Err(bits) => return bits,
            }
        };
        let out = match path_relpath_text(_py, &path, &start, sep) {
            Ok(out) => out,
            Err(bits) => return bits,
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_makedirs(path_bits: u64, mode_bits: u64, exist_ok_bits: u64) -> u64 {
    fn create_parent_dir(path: &std::path::Path) -> std::io::Result<()> {
        if path.as_os_str().is_empty() {
            return Ok(());
        }
        match std::fs::metadata(path) {
            Ok(meta) => {
                if meta.is_dir() {
                    return Ok(());
                }
                return Err(std::io::Error::new(
                    ErrorKind::AlreadyExists,
                    format!("File exists: {}", path.to_string_lossy()),
                ));
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && parent != path
        {
            create_parent_dir(parent)?;
        }
        let builder = std::fs::DirBuilder::new();
        match builder.create(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                let meta = std::fs::metadata(path)?;
                if meta.is_dir() { Ok(()) } else { Err(err) }
            }
            Err(err) => Err(err),
        }
    }

    crate::with_gil_entry_nopanic!(_py, {
        if let Err(error) =
            crate::require_operation(_py, crate::OperationId::PathMakedirs, AuditArgs::None)
        {
            return error;
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        // Path.mkdir(parents=True) tries os.mkdir(self, mode) first, so `mode`
        // converts (CPython's `int` converter) before any directory is created.
        let Some(mode) = crate::builtins::numbers::index_c_int_from_obj(_py, mode_bits) else {
            return MoltObject::none().bits();
        };
        let mode = mode as u32;
        if path.as_os_str().is_empty() {
            return MoltObject::none().bits();
        }
        let exist_ok = is_truthy(_py, obj_from_bits(exist_ok_bits));
        match std::fs::metadata(&path) {
            Ok(meta) => {
                if meta.is_dir() {
                    if exist_ok {
                        return MoltObject::none().bits();
                    }
                    let msg = format!("File exists: {}", path.to_string_lossy());
                    return raise_exception::<_>(_py, BuiltinExc::FileExistsError, &msg);
                }
                let msg = format!("File exists: {}", path.to_string_lossy());
                return raise_exception::<_>(_py, BuiltinExc::FileExistsError, &msg);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                let msg = err.to_string();
                return match err.kind() {
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                    }
                    _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
                };
            }
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && parent != path
            && let Err(err) = create_parent_dir(parent)
        {
            let msg = err.to_string();
            return match err.kind() {
                ErrorKind::AlreadyExists => {
                    raise_exception::<_>(_py, BuiltinExc::FileExistsError, &msg)
                }
                ErrorKind::NotFound => {
                    raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                }
                ErrorKind::PermissionDenied => {
                    raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                }
                _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
            };
        }
        #[cfg(unix)]
        let builder = {
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(mode);
            builder
        };
        #[cfg(not(unix))]
        let builder = {
            let _ = mode;
            std::fs::DirBuilder::new()
        };
        match builder.create(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => {
                        if exist_ok {
                            match std::fs::metadata(&path) {
                                Ok(meta) if meta.is_dir() => MoltObject::none().bits(),
                                _ => raise_exception::<_>(_py, BuiltinExc::FileExistsError, &msg),
                            }
                        } else {
                            raise_exception::<_>(_py, BuiltinExc::FileExistsError, &msg)
                        }
                    }
                    ErrorKind::NotFound => {
                        raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                    }
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                    }
                    _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_as_uri(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_as_uri_text(&path, sep) {
            Ok(out) => out,
            Err(msg) => return raise_exception::<_>(_py, BuiltinExc::ValueError, &msg),
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_match(path_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let pattern = match path_str_arg_from_bits(_py, pattern_bits, "pattern") {
            Ok(pattern) => pattern,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(path_match_text(&path, &pattern, sep)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_glob(
    path_bits: u64,
    pattern_bits: u64,
    recursive_bits: u64,
    original_pattern_bits: u64,
    legacy_parsed_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        let (pattern, bytes_pattern) =
            if let Some(pattern) = string_obj_bytes(obj_from_bits(pattern_bits)) {
                (pattern, false)
            } else if let Some(pointer) = obj_from_bits(pattern_bits).as_ptr()
                && unsafe { object_type_id(pointer) } == TYPE_ID_BYTES
            {
                (
                    unsafe { std::slice::from_raw_parts(bytes_data(pointer), bytes_len(pointer)) }
                        .to_vec(),
                    true,
                )
            } else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "pattern must be str or bytes",
                );
            };
        let recursive = is_truthy(_py, obj_from_bits(recursive_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let sep = path_sep_char();
        let modern = crate::object::ops_sys::runtime_target_at_least(_py, 3, 13);
        let native_pattern = crate::object::ops_sys::runtime_target_at_least(_py, 3, 14);
        let mut parts: Vec<Vec<u32>> = if !obj_from_bits(legacy_parsed_bits).is_none() {
            // Only the legacy callback parser supplies this result. Keep its
            // actual roots alive across truth callbacks; never reparse the
            // original spelling or discard callback-produced tail components.
            if modern {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "legacy glob parse is not valid for this target",
                );
            }
            let Some(pointer) = obj_from_bits(legacy_parsed_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "legacy glob parse must be a tuple",
                );
            };
            let Some(parsed) = (unsafe { crate::object::seq_access::pin_tuple(_py, pointer) })
            else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "legacy glob parse must be a tuple",
                );
            };
            if parsed.len() != 3 {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "legacy glob parse must have three fields",
                );
            }
            let drive = is_truthy(_py, obj_from_bits(parsed[0]));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let non_relative = drive || is_truthy(_py, obj_from_bits(parsed[1]));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if non_relative {
                return CompatibilityError::PathGlobNonRelative { non_relative }.raise(_py);
            }
            let Some(pointer) = obj_from_bits(parsed[2]).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "legacy glob parts must be a list",
                );
            };
            if unsafe { object_type_id(pointer) } != TYPE_ID_LIST {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "legacy glob parts must be a list",
                );
            }
            let Some(values) = (unsafe {
                crate::object::seq_access::snapshot(_py, pointer, "glob parts snapshot")
            }) else {
                return MoltObject::none().bits();
            };
            let mut parts = Vec::with_capacity(values.len());
            for &value in values.iter() {
                let Some(text) = string_obj_bytes(obj_from_bits(value)) else {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "legacy glob parts must contain str",
                    );
                };
                parts.push(molt_obj_model::text_storage::code_points(&text).collect());
            }
            parts
        } else {
            // Ordinary bytes fail during legacy parsing/construction. Only
            // 3.14 glob splits their root before its str-separated tail fails.
            if bytes_pattern && (!native_pattern || recursive) {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "a bytes-like object is required, not 'str'",
                );
            }
            let coordinate = if cfg!(windows) && modern {
                molt_runtime_platform::path_text::PathCoordinate::WindowsUtf16
            } else {
                molt_runtime_platform::path_text::PathCoordinate::PythonCodePoints
            };
            let decoded;
            let storage = if bytes_pattern && cfg!(windows) {
                decoded = match super::io::os_string_from_bytes(_py, &pattern) {
                    Ok(value) => value.into_encoded_bytes(),
                    Err(error) => return error,
                };
                decoded.as_slice()
            } else {
                pattern.as_slice()
            };
            let (parsed, (_, root)) =
                match super::io::root_path_parse(_py, storage, sep as u8, false, coordinate) {
                    Ok(value) => value,
                    Err(error) => return error,
                };
            let non_relative = root != 0;
            if non_relative {
                return CompatibilityError::PathGlobNonRelative { non_relative }.raise(_py);
            }
            if bytes_pattern {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "a bytes-like object is required, not 'str'",
                );
            }
            if !recursive && parsed.is_empty() {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::ValueError,
                    "Unacceptable pattern: ''",
                );
            }
            parsed
                .split(|byte| *byte == sep as u8 || (sep == '\\' && *byte == b'/'))
                .filter(|part| !part.is_empty() && *part != b".")
                .map(|part| molt_obj_model::text_storage::code_points(part).collect())
                .collect()
        };
        if modern && parts.is_empty() {
            return raise_exception::<_>(_py, BuiltinExc::ValueError, "Unacceptable pattern");
        }
        let mut trailing_separator = pattern
            .last()
            .is_some_and(|byte| *byte == sep as u8 || (sep == '\\' && *byte == b'/'));
        if !crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
            let inspect_tail =
                modern || !recursive || is_truthy(_py, obj_from_bits(original_pattern_bits));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if inspect_tail && type_of_bits(_py, original_pattern_bits) != builtin_classes(_py).str
            {
                use crate::object::ops_compare::{CompareBoolOutcome, compare_object_eq_bool};
                use molt_runtime_core::ProtectedRuntimeValue;

                let tail_bits = crate::molt_getitem_method(
                    original_pattern_bits,
                    MoltObject::from_int(-1).bits(),
                );
                let tail =
                    unsafe { ProtectedRuntimeValue::from_owned_bits(_py.core_token(), tail_bits) };
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                trailing_separator = false;
                // Tuple membership compares each separator to the callback's
                // result, with identity-or-equality and observable truth tests.
                for separator in [Some(sep as u8), if sep == '\\' { Some(b'/') } else { None }] {
                    let bits = if let Some(byte) = separator {
                        let pointer = alloc_string(_py, &[byte]);
                        if pointer.is_null() {
                            return MoltObject::none().bits();
                        }
                        MoltObject::from_ptr(pointer).bits()
                    } else {
                        MoltObject::none().bits()
                    };
                    let separator =
                        unsafe { ProtectedRuntimeValue::from_owned_bits(_py.core_token(), bits) };
                    match compare_object_eq_bool(
                        _py,
                        obj_from_bits(separator.bits()),
                        obj_from_bits(tail.bits()),
                    ) {
                        CompareBoolOutcome::True => {
                            trailing_separator = true;
                            break;
                        }
                        CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => {}
                        CompareBoolOutcome::Error => return MoltObject::none().bits(),
                    }
                }
            } else if !inspect_tail {
                trailing_separator = false;
            }
        }
        if !modern
            && parts.iter().any(|part| {
                part.windows(2).any(|pair| pair == [42, 42]) && part.as_slice() != [42, 42]
            })
        {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "Invalid pattern: '**' can only be an entire path component",
            );
        }
        if recursive {
            parts.insert(0, vec![42, 42]);
        }
        if trailing_separator {
            parts.push(Vec::new());
        }
        if parts.is_empty() {
            if modern {
                return raise_exception::<_>(_py, BuiltinExc::ValueError, "Unacceptable pattern");
            }
            return raise_exception::<_>(_py, BuiltinExc::IndexError, "tuple index out of range");
        }
        // Pattern errors and callbacks precede base filesystem encoding.
        let dir = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let matches = match path_glob_matches(&dir, &parts, modern, |name| {
            let text = super::io::os_string_text(_py, name)?;
            Ok::<_, u64>(molt_obj_model::text_storage::code_points(&text).collect())
        }) {
            Ok(values) => values,
            Err(error) => return error,
        };
        let mut values = Vec::with_capacity(matches.len());
        for path in matches {
            let value = super::io::path_result_bits(_py, &path, PathFlavor::Str);
            if exception_pending(_py) {
                for value in values {
                    dec_ref_bits(_py, value);
                }
                return MoltObject::none().bits();
            }
            values.push(value);
        }
        let pointer = alloc_list(_py, &values);
        for value in values {
            dec_ref_bits(_py, value);
        }
        if pointer.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(pointer).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_has_magic(pathname_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (pathname, _) = match path_value_with_flavor_from_bits(_py, pathname_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(molt_runtime_platform::path_text::glob_has_magic_raw(
            &pathname,
        ))
        .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_escape(pathname_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let sep = path_sep_char();
        let (pathname, flavor) = match path_value_with_flavor_from_bits(_py, pathname_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = flavor == PathFlavor::Bytes;
        let coordinate = super::io::path_root_coordinate(_py, bytes);
        let (storage, (drive, _)) =
            match super::io::root_path_parse(_py, &pathname, sep as u8, bytes, coordinate) {
                Ok(value) => value,
                Err(error) => return error,
            };
        let escaped = molt_runtime_platform::path_text::glob_escape_with_drive(&storage, drive);
        if flavor == PathFlavor::Bytes {
            let ptr = alloc_bytes(_py, &escaped);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        } else {
            let ptr = alloc_string(_py, &escaped);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_translate(
    pathname_bits: u64,
    recursive_bits: u64,
    include_hidden_bits: u64,
    seps_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let pattern = if let Some(text) = string_obj_to_owned(obj_from_bits(pathname_bits)) {
            text
        } else {
            let type_id = obj_from_bits(pathname_bits)
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) });
            if matches!(type_id, Some(TYPE_ID_BYTES) | Some(TYPE_ID_BYTEARRAY)) {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "cannot use a string pattern on a bytes-like object",
                );
            }
            let type_name = class_name_for_error(type_of_bits(_py, pathname_bits));
            let msg = format!("expected string or bytes-like object, got '{type_name}'");
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
        };

        let recursive = is_truthy(_py, obj_from_bits(recursive_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let include_hidden = is_truthy(_py, obj_from_bits(include_hidden_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let seps = if obj_from_bits(seps_bits).is_none() {
            None
        } else if let Some(text) = string_obj_to_owned(obj_from_bits(seps_bits)) {
            Some(text)
        } else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "seps must be str or None");
        };

        let out = glob_translate_text(&pattern, recursive, include_hidden, seps.as_deref());
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// Parsed, validated `glob`/`iglob` arguments (shared by the eager list path
/// and the lazy iterator path so semantics stay identical).
pub(crate) struct GlobParsedArgs {
    pub pathname: Vec<u8>,
    pub root_dir: Option<Vec<u8>>,
    pub dir_fd: GlobDirFdArg,
    pub recursive: bool,
    pub include_hidden: bool,
    pub bytes_mode: bool,
    pub sep: char,
}

/// Validate and normalize the shared `glob(pathname, root_dir, dir_fd,
/// recursive, include_hidden)` arguments. Must be called inside a GIL entry;
/// returns raised-exception bits in `Err` (capability, bytes/str mixing,
/// dir_fd TypeError/OverflowError, wasm dir_fd guard).
pub(crate) fn glob_parse_args(
    _py: &PyToken<'_>,
    pathname_bits: u64,
    root_dir_bits: u64,
    dir_fd_bits: u64,
    recursive_bits: u64,
    include_hidden_bits: u64,
) -> Result<GlobParsedArgs, u64> {
    if !has_capability(_py, "fs.read") {
        return Err(raise_capability_denied(_py, "fs.read"));
    }
    let sep = path_sep_char();

    #[cfg(windows)]
    let (pathname, pathname_flavor) = match path_value_with_flavor_from_bits(_py, pathname_bits) {
        Ok((path, flavor)) => (
            path.into_iter()
                .map(|byte| if byte == b'/' { b'\\' } else { byte })
                .collect::<Vec<u8>>(),
            flavor,
        ),
        Err(bits) => return Err(bits),
    };
    #[cfg(not(windows))]
    let (pathname, pathname_flavor) = match path_value_with_flavor_from_bits(_py, pathname_bits) {
        Ok((path, flavor)) => (path, flavor),
        Err(bits) => return Err(bits),
    };

    let root_dir = if obj_from_bits(root_dir_bits).is_none() {
        None
    } else {
        #[cfg(windows)]
        {
            match path_value_with_flavor_from_bits(_py, root_dir_bits) {
                Ok((path, flavor)) => Some((
                    path.into_iter()
                        .map(|byte| if byte == b'/' { b'\\' } else { byte })
                        .collect(),
                    flavor,
                )),
                Err(bits) => return Err(bits),
            }
        }
        #[cfg(not(windows))]
        {
            match path_value_with_flavor_from_bits(_py, root_dir_bits) {
                Ok((path, flavor)) => Some((path, flavor)),
                Err(bits) => return Err(bits),
            }
        }
    };

    if let Some((_, root_dir_flavor)) = root_dir.as_ref()
        && *root_dir_flavor != pathname_flavor
    {
        let msg = if molt_runtime_platform::path_text::path_isabs_raw(
            &pathname,
            sep,
            pathname_flavor == PathFlavor::Str,
        ) {
            "Can't mix strings and bytes in path components"
        } else if pathname_flavor == PathFlavor::Bytes {
            "cannot use a bytes pattern on a string-like object"
        } else {
            "cannot use a string pattern on a bytes-like object"
        };
        return Err(raise_exception::<_>(_py, BuiltinExc::TypeError, msg));
    }

    let dir_fd = glob_dir_fd_arg_from_bits(_py, dir_fd_bits)?;

    let bytes_mode = pathname_flavor == PathFlavor::Bytes;
    #[cfg(target_arch = "wasm32")]
    {
        let root_dir_is_absolute = root_dir.as_ref().is_some_and(|(path, _)| {
            molt_runtime_platform::path_text::path_isabs_raw(
                path,
                sep,
                pathname_flavor == PathFlavor::Str,
            )
        });
        if let GlobDirFdArg::Int(fd) = dir_fd
            && glob_dir_fd_root_path(fd).is_none()
            && !molt_runtime_platform::path_text::path_isabs_raw(
                &pathname,
                sep,
                pathname_flavor == PathFlavor::Str,
            )
            && !root_dir_is_absolute
        {
            return Err(raise_exception::<_>(
                _py,
                BuiltinExc::NotImplementedError,
                "glob(dir_fd=...) requires fd-backed path resolution on this wasm host",
            ));
        }
    }

    let recursive = is_truthy(_py, obj_from_bits(recursive_bits));
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let include_hidden = is_truthy(_py, obj_from_bits(include_hidden_bits));
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }

    Ok(GlobParsedArgs {
        pathname,
        root_dir: root_dir.map(|(root, _)| root),
        dir_fd,
        recursive,
        include_hidden,
        bytes_mode,
        sep,
    })
}

/// `glob.glob(pathname, *, root_dir, dir_fd, recursive, include_hidden)`.
/// Eager: materializes the full match list. Defined as `list(iglob(...))` —
/// it drains the same lazy streaming iterator the `iglob` path returns, so the
/// two are guaranteed to agree (one source of truth: `glob_iter_new_state`).
#[unsafe(no_mangle)]
pub extern "C" fn molt_glob(
    pathname_bits: u64,
    root_dir_bits: u64,
    dir_fd_bits: u64,
    recursive_bits: u64,
    include_hidden_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args = match glob_parse_args(
            _py,
            pathname_bits,
            root_dir_bits,
            dir_fd_bits,
            recursive_bits,
            include_hidden_bits,
        ) {
            Ok(a) => a,
            Err(bits) => return bits,
        };
        let mut state = match glob_iter_new_state(
            _py,
            &args.pathname,
            args.root_dir.as_deref(),
            args.dir_fd,
            args.recursive,
            args.include_hidden,
            args.bytes_mode,
            args.sep,
        ) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        // Drain the lazy iterator: `glob(...) == list(iglob(...))`.
        let mut out: Vec<Vec<u8>> = Vec::new();
        loop {
            match state.next_path_public(_py) {
                Err(bits) => return bits,
                Ok(None) => break,
                Ok(Some(path)) => out.push(path),
            }
        }
        alloc_path_list_bits(_py, &out, args.bytes_mode)
    })
}

/// `glob.iglob(...)` — returns a lazy `TYPE_ID_GLOB_ITER` that streams matching
/// paths one per `__next__` at bounded RSS (CPython-faithful generator chain
/// reified as a native work-stack; see `GlobIterState`).
#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_iter(
    pathname_bits: u64,
    root_dir_bits: u64,
    dir_fd_bits: u64,
    recursive_bits: u64,
    include_hidden_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args = match glob_parse_args(
            _py,
            pathname_bits,
            root_dir_bits,
            dir_fd_bits,
            recursive_bits,
            include_hidden_bits,
        ) {
            Ok(a) => a,
            Err(bits) => return bits,
        };
        let state = match glob_iter_new_state(
            _py,
            &args.pathname,
            args.root_dir.as_deref(),
            args.dir_fd,
            args.recursive,
            args.include_hidden,
            args.bytes_mode,
            args.sep,
        ) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        glob_iter_alloc_object(_py, state)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_chmod(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        // pathlib.Path.chmod passes `mode` to os.chmod: CPython's `int` converter.
        let Some(mode) = crate::builtins::numbers::index_c_int_from_obj(_py, mode_bits) else {
            return MoltObject::none().bits();
        };
        #[cfg(unix)]
        {
            let perms = std::fs::Permissions::from_mode(mode as u32);
            match std::fs::set_permissions(&path, perms) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => {
                    let msg = err.to_string();
                    match err.kind() {
                        ErrorKind::NotFound => {
                            raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                        }
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                        }
                        _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
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
                            raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                        }
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                        }
                        _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
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
                            raise_exception::<_>(_py, BuiltinExc::FileNotFoundError, &msg)
                        }
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, BuiltinExc::PermissionError, &msg)
                        }
                        _ => raise_exception::<_>(_py, BuiltinExc::OSError, &msg),
                    }
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = mode;
            raise_exception::<_>(
                _py,
                BuiltinExc::NotImplementedError,
                "chmod is unsupported on this platform",
            )
        }
    })
}

/// One compile-target field order governs both the public type and every
/// stat/lstat/fstat/at payload. Visible timestamps are integers; attributes keep
/// the separately acquired fractional seconds and exact nanosecond integers.
#[derive(Clone, Copy)]
enum StatField {
    Mode,
    Ino,
    Dev,
    Nlink,
    Uid,
    Gid,
    Size,
    AtimeSeconds,
    MtimeSeconds,
    CtimeSeconds,
    Atime,
    Mtime,
    Ctime,
    AtimeNs,
    MtimeNs,
    CtimeNs,
    #[cfg(any(unix, target_os = "wasi"))]
    Blksize,
    #[cfg(any(unix, target_os = "wasi"))]
    Blocks,
    #[cfg(any(unix, target_os = "wasi"))]
    Rdev,
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    Flags,
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    Gen,
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        windows
    ))]
    Birthtime,
    #[cfg(windows)]
    BirthtimeNs,
    #[cfg(windows)]
    FileAttributes,
    #[cfg(any(target_os = "solaris", target_os = "illumos"))]
    Fstype,
    #[cfg(windows)]
    ReparseTag,
}
impl StatField {
    const ALL: &'static [Self] = &[
        Self::Mode,
        Self::Ino,
        Self::Dev,
        Self::Nlink,
        Self::Uid,
        Self::Gid,
        Self::Size,
        Self::AtimeSeconds,
        Self::MtimeSeconds,
        Self::CtimeSeconds,
        Self::Atime,
        Self::Mtime,
        Self::Ctime,
        Self::AtimeNs,
        Self::MtimeNs,
        Self::CtimeNs,
        #[cfg(any(unix, target_os = "wasi"))]
        Self::Blksize,
        #[cfg(any(unix, target_os = "wasi"))]
        Self::Blocks,
        #[cfg(any(unix, target_os = "wasi"))]
        Self::Rdev,
        #[cfg(any(
            target_vendor = "apple",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        Self::Flags,
        #[cfg(any(
            target_vendor = "apple",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        Self::Gen,
        #[cfg(any(
            target_vendor = "apple",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            windows
        ))]
        Self::Birthtime,
        #[cfg(windows)]
        Self::BirthtimeNs,
        #[cfg(windows)]
        Self::FileAttributes,
        #[cfg(any(target_os = "solaris", target_os = "illumos"))]
        Self::Fstype,
        #[cfg(windows)]
        Self::ReparseTag,
    ];
    fn description(self) -> (Option<&'static std::ffi::CStr>, &'static std::ffi::CStr) {
        use StatField::*;
        match self {
            Mode => (Some(c"st_mode"), c"protection bits"),
            Ino => (Some(c"st_ino"), c"inode"),
            Dev => (Some(c"st_dev"), c"device"),
            Nlink => (Some(c"st_nlink"), c"number of hard links"),
            Uid => (Some(c"st_uid"), c"user ID of owner"),
            Gid => (Some(c"st_gid"), c"group ID of owner"),
            Size => (Some(c"st_size"), c"total size, in bytes"),
            AtimeSeconds => (None, c"integer time of last access"),
            MtimeSeconds => (None, c"integer time of last modification"),
            CtimeSeconds => (None, c"integer time of last change"),
            Atime => (Some(c"st_atime"), c"time of last access"),
            Mtime => (Some(c"st_mtime"), c"time of last modification"),
            Ctime => (Some(c"st_ctime"), c"time of last change"),
            AtimeNs => (Some(c"st_atime_ns"), c"time of last access in nanoseconds"),
            MtimeNs => (
                Some(c"st_mtime_ns"),
                c"time of last modification in nanoseconds",
            ),
            CtimeNs => (Some(c"st_ctime_ns"), c"time of last change in nanoseconds"),
            #[cfg(any(unix, target_os = "wasi"))]
            Blksize => (Some(c"st_blksize"), c"blocksize for filesystem I/O"),
            #[cfg(any(unix, target_os = "wasi"))]
            Blocks => (Some(c"st_blocks"), c"number of blocks allocated"),
            #[cfg(any(unix, target_os = "wasi"))]
            Rdev => (Some(c"st_rdev"), c"device type (if inode device)"),
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Flags => (Some(c"st_flags"), c"user defined flags for file"),
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Gen => (Some(c"st_gen"), c"generation number"),
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                windows
            ))]
            Birthtime => (Some(c"st_birthtime"), c"time of creation"),
            #[cfg(windows)]
            BirthtimeNs => (Some(c"st_birthtime_ns"), c"time of creation in nanoseconds"),
            #[cfg(windows)]
            FileAttributes => (Some(c"st_file_attributes"), c"Windows file attribute bits"),
            #[cfg(any(target_os = "solaris", target_os = "illumos"))]
            Fstype => (Some(c"st_fstype"), c"Type of filesystem"),
            #[cfg(windows)]
            ReparseTag => (Some(c"st_reparse_tag"), c"Windows reparse tag"),
        }
    }
}

// Cold constructor copies the C description into the sealed native layout.
// The cache, rather than the public os namespace, owns the resulting class.
#[cold]
fn build_stat_result_class(py: &PyToken<'_>) -> u64 {
    use molt_cpython_abi::abi_types::{PyStructSequence_Desc, PyStructSequence_Field};
    let mut fields: [PyStructSequence_Field; StatField::ALL.len() + 1] =
        std::array::from_fn(|_| PyStructSequence_Field {
            name: std::ptr::null(),
            doc: std::ptr::null(),
        });
    for (index, field) in StatField::ALL.iter().enumerate() {
        let (name, doc) = field.description();
        fields[index] = PyStructSequence_Field {
            name: name.map_or(
                unsafe { molt_cpython_abi::api::sequences::PyStructSequence_UnnamedField },
                std::ffi::CStr::as_ptr,
            ),
            doc: doc.as_ptr(),
        };
    }
    let desc = PyStructSequence_Desc {
        name: c"os.stat_result".as_ptr(),
        doc: c"stat_result: Result from stat, fstat, or lstat.\n\nThis object may be accessed either as a tuple of\n  (mode, ino, dev, nlink, uid, gid, size, atime, mtime, ctime)\nor via the attributes st_mode, st_ino, st_dev, st_nlink, st_uid, and so on.\n\nPosix/windows: If your platform supports st_blksize, st_blocks, st_rdev,\nor st_flags, they are available as attributes only.\n\nSee os.stat for more information.".as_ptr(),
        fields: fields.as_mut_ptr(), n_in_sequence: 10,
    };
    let class = unsafe { crate::builtins::structseq::from_stat_description(py, &desc) };
    if exception_pending(py) || obj_from_bits(class).is_none() {
        0
    } else {
        class
    }
}

/// Borrow the interpreter's canonical stat-result class. A warm read is one
/// cache load, with no Python namespace lookup or runtime/GIL reentry.
pub(crate) fn stat_result_class(py: &PyToken<'_>) -> u64 {
    init_atomic_bits(py, &runtime_state(py).types.stat_result_class, || {
        build_stat_result_class(py)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_stat_result_type() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        crate::state::cache::retain_cached_result(py, stat_result_class(py))
    })
}

fn stat_result_from_metadata(
    py: &PyToken<'_>,
    metadata: &molt_runtime_platform::stat_support::PathMetadata,
) -> u64 {
    use StatField::*;
    use molt_runtime_core::OwnedRuntimeValue;
    let class = stat_result_class(py);
    if class == 0 {
        return MoltObject::none().bits();
    }
    let mut owners: [Option<OwnedRuntimeValue<'_>>; StatField::ALL.len()] =
        std::array::from_fn(|_| None);
    let mut bits = [MoltObject::none().bits(); StatField::ALL.len()];
    for (index, &field) in StatField::ALL.iter().enumerate() {
        let value = match field {
            Mode => int_bits_from_i128(py, metadata.fields[0]),
            Ino => crate::builtins::numbers::int_bits_from_u128(py, metadata.ino()),
            Dev => int_bits_from_i128(py, metadata.fields[2]),
            Nlink => int_bits_from_i128(py, metadata.fields[3]),
            Uid => int_bits_from_i128(py, metadata.fields[4]),
            Gid => int_bits_from_i128(py, metadata.fields[5]),
            Size => int_bits_from_i128(py, metadata.fields[6]),
            AtimeSeconds => int_bits_from_i128(py, metadata.times[0].seconds()),
            MtimeSeconds => int_bits_from_i128(py, metadata.times[1].seconds()),
            CtimeSeconds => int_bits_from_i128(py, metadata.times[2].seconds()),
            Atime => MoltObject::from_float(metadata.times[0].as_float()).bits(),
            Mtime => MoltObject::from_float(metadata.times[1].as_float()).bits(),
            Ctime => MoltObject::from_float(metadata.times[2].as_float()).bits(),
            AtimeNs => int_bits_from_i128(py, metadata.times[0].nanoseconds()),
            MtimeNs => int_bits_from_i128(py, metadata.times[1].nanoseconds()),
            CtimeNs => int_bits_from_i128(py, metadata.times[2].nanoseconds()),
            #[cfg(any(unix, target_os = "wasi"))]
            Blksize => int_bits_from_i128(py, metadata.block_size as i128),
            #[cfg(any(unix, target_os = "wasi"))]
            Blocks => int_bits_from_i128(py, metadata.blocks as i128),
            #[cfg(any(unix, target_os = "wasi"))]
            Rdev => {
                use molt_runtime_platform::stat_support::{
                    DeviceNumberConvention, device_number_integer,
                };
                let convention = if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
                    DeviceNumberConvention::PythonDev
                } else {
                    DeviceNumberConvention::CLong
                };
                int_bits_from_i128(py, device_number_integer(metadata.rdev, convention))
            }
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Flags => int_bits_from_i128(py, metadata.flags as i128),
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Gen => int_bits_from_i128(py, metadata.generation as i128),
            #[cfg(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                windows
            ))]
            Birthtime => MoltObject::from_float(metadata.birthtime.as_float()).bits(),
            #[cfg(windows)]
            BirthtimeNs => int_bits_from_i128(py, metadata.birthtime.nanoseconds()),
            #[cfg(windows)]
            FileAttributes => int_bits_from_i128(py, metadata.file_attributes as i128),
            #[cfg(windows)]
            ReparseTag => int_bits_from_i128(py, metadata.reparse_tag as i128),
            #[cfg(any(target_os = "solaris", target_os = "illumos"))]
            Fstype => {
                let text = alloc_string(
                    py,
                    &metadata.fstype[..metadata
                        .fstype
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(metadata.fstype.len())],
                );
                if text.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(text).bits()
                }
            }
        };
        owners[index] = Some(unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), value) });
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        bits[index] = value;
    }
    // Publish the final record once; hidden native owners never pass through a
    // second Python constructor or an intermediate payload tuple.
    crate::builtins::structseq::from_values(py, class, &bits)
}

/// Convert a path to a NUL-terminated `CString` for the `*at` POSIX syscall
/// family, raising `ValueError` (CPython parity) on an interior NUL.
///
/// On unix the raw path bytes pass through unchanged (`OsStrExt`); on other
/// platforms the lossy UTF-8 view is used. The returned error bits already
/// carry a recorded exception, so callers just propagate them. Always compiled
/// (regardless of the `stdlib_path` feature) so it is the single shared home
/// for both os_ext sources.
#[cfg(unix)]
#[inline]
pub(crate) fn at_path_to_cstring(
    _py: &PyToken<'_>,
    path: &std::path::Path,
) -> Result<std::ffi::CString, u64> {
    let bytes: Vec<u8> = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    match std::ffi::CString::new(bytes) {
        Ok(c) => Ok(c),
        Err(_) => Err(raise_exception::<u64>(
            _py,
            BuiltinExc::ValueError,
            "embedded null byte",
        )),
    }
}

/// Resolve a `dir_fd` NaN-boxed argument to a libc fd for the `*at` family.
///
/// `None` maps to the platform `libc::AT_FDCWD` constant (−100 Linux, −2 macOS
/// — never hardcoded); any non-integer raises `TypeError` with CPython's exact
/// message. The returned error bits already carry a recorded exception.
#[cfg(all(not(target_arch = "wasm32"), unix))]
#[inline]
pub(crate) fn at_resolve_dir_fd(_py: &PyToken<'_>, dir_fd_bits: u64) -> Result<libc::c_int, u64> {
    let obj = obj_from_bits(dir_fd_bits);
    if obj.is_none() {
        return Ok(libc::AT_FDCWD);
    }
    match to_i64(obj) {
        Some(fd) => Ok(fd as libc::c_int),
        None => Err(raise_exception::<u64>(
            _py,
            BuiltinExc::TypeError,
            "an integer is required",
        )),
    }
}

/// Project the same canonical metadata value used by routed and descriptor queries.
#[cfg(unix)]
pub(crate) fn stat_result_from_libc_stat(py: &PyToken<'_>, st: &libc::stat) -> u64 {
    stat_result_from_metadata(
        py,
        &molt_runtime_platform::stat_support::PathMetadata::from_unix_stat(st),
    )
}

fn metadata_stat(py: &PyToken<'_>, bits: u64, follow: bool) -> u64 {
    let input = match metadata_input(py, bits, follow) {
        Ok(input) => input,
        Err(error) => return error,
    };
    match metadata_for_input(py, &input, follow, OperationId::OsStat) {
        Err(error) => error,
        Ok(Ok(metadata)) => stat_result_from_metadata(py, &metadata),
        Ok(Err(error)) => error.raise(py, if follow { "stat" } else { "lstat" }),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_stat(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { metadata_stat(py, path_bits, true) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_lstat(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { metadata_stat(py, path_bits, false) })
}

/// Both metadata aliases select a virtual/host path route before a genuinely
/// descriptor-relative operation can access a host directory descriptor.
fn metadata_stat_at(
    py: &PyToken<'_>,
    path_bits: u64,
    dir_fd_bits: u64,
    follow_bits: u64,
    operation: OperationId,
) -> u64 {
    let path = match metadata_input(py, path_bits, false) {
        Ok(MetadataInput::Path(path)) => path,
        Ok(MetadataInput::Fd(_)) => unreachable!("path-only conversion"),
        Err(error) => return error,
    };
    let dir_fd = if obj_from_bits(dir_fd_bits).is_none() {
        None
    } else {
        match crate::builtins::numbers::index_c_int_from_obj(py, dir_fd_bits) {
            Some(fd) => Some(fd),
            None => return MoltObject::none().bits(),
        }
    };
    let follow = is_truthy(py, obj_from_bits(follow_bits));
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let default_directory = dir_fd.is_none();
    #[cfg(all(not(target_arch = "wasm32"), unix))]
    let default_directory = default_directory || dir_fd == Some(libc::AT_FDCWD);
    if path.is_absolute() || default_directory {
        return match super::io::path_metadata(py, &path, follow, OperationId::OsStat) {
            Err(error) => error,
            Ok(Ok(metadata)) => stat_result_from_metadata(py, &metadata),
            Ok(Err(error)) => error.raise(py, if follow { "stat" } else { "lstat" }),
        };
    }
    if let Err(error) = crate::require_operation::<u64>(py, operation, AuditArgs::None) {
        return error;
    }
    #[cfg(all(not(target_arch = "wasm32"), unix))]
    {
        let c_path = match at_path_to_cstring(py, &path) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
        let result = unsafe {
            libc::fstatat(
                dir_fd.expect("relative descriptor"),
                c_path.as_ptr(),
                metadata.as_mut_ptr(),
                flags,
            )
        };
        if result != 0 {
            return super::io::PathIoError::from_native(std::io::Error::last_os_error())
                .raise(py, "stat");
        }
        stat_result_from_libc_stat(py, unsafe { metadata.assume_init_ref() })
    }
    #[cfg(not(all(not(target_arch = "wasm32"), unix)))]
    {
        let _ = dir_fd;
        raise_os_error_errno::<u64>(py, libc::ENOSYS as i64, "stat")
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_lstat_at(path_bits: u64, dir_fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        metadata_stat_at(
            py,
            path_bits,
            dir_fd_bits,
            MoltObject::from_bool(false).bits(),
            OperationId::OsLstatAt,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_fstat(fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(fd) = crate::builtins::numbers::index_c_int_from_obj(py, fd_bits) else {
            return MoltObject::none().bits();
        };
        match metadata_for_input(py, &MetadataInput::Fd(fd), true, OperationId::OsStat) {
            Err(error) => error,
            Ok(Ok(metadata)) => stat_result_from_metadata(py, &metadata),
            Ok(Err(error)) => error.raise(py, "fstat"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_rename(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        match std::fs::rename(&src, &dst) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "rename"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_replace(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        match std::fs::rename(&src, &dst) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "replace"),
        }
    })
}

// ---------------------------------------------------------------------------
// dir_fd-relative variants (`*at` POSIX family). `dir_fd=None` -> AT_FDCWD.
// WASI preview-1 cannot mint general dir fds dynamically, so the wasm32 stubs
// raise OSError(ENOSYS); Windows is covered by the `#[cfg(not(unix))]` arm.
// ---------------------------------------------------------------------------

/// Directory-relative stat shares the path route and descriptor custody of lstat.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_stat_at(path_bits: u64, dir_fd_bits: u64, follow_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        metadata_stat_at(
            py,
            path_bits,
            dir_fd_bits,
            follow_bits,
            OperationId::OsStatAt,
        )
    })
}

/// `os.rename(src, src_dir_fd=..., dst, dst_dir_fd=...)` → None.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_rename_at(
    src_bits: u64,
    src_dir_fd_bits: u64,
    dst_bits: u64,
    dst_dir_fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("os.rename_at", "fs.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.write capability",
            );
        }
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        #[cfg(unix)]
        {
            let src_dir_fd = match at_resolve_dir_fd(_py, src_dir_fd_bits) {
                Ok(fd) => fd,
                Err(bits) => return bits,
            };
            let dst_dir_fd = match at_resolve_dir_fd(_py, dst_dir_fd_bits) {
                Ok(fd) => fd,
                Err(bits) => return bits,
            };
            let c_src = match at_path_to_cstring(_py, &src) {
                Ok(c) => c,
                Err(bits) => return bits,
            };
            let c_dst = match at_path_to_cstring(_py, &dst) {
                Ok(c) => c,
                Err(bits) => return bits,
            };
            let rc =
                unsafe { libc::renameat(src_dir_fd, c_src.as_ptr(), dst_dir_fd, c_dst.as_ptr()) };
            if rc < 0 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "rename");
            }
            MoltObject::none().bits()
        }
        #[cfg(not(unix))]
        {
            let _ = (src, dst, src_dir_fd_bits, dst_dir_fd_bits);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "rename")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_rename_at(_s: u64, _sd: u64, _d: u64, _dd: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "rename")
    })
}

/// `os.replace(src, src_dir_fd=..., dst, dst_dir_fd=...)` → None.
///
/// POSIX `rename` IS atomic replace, so this shares `renameat` with `rename_at`;
/// the distinct symbol exists so the Python wrapper uses the semantically
/// correct name for diagnostics.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_replace_at(
    src_bits: u64,
    src_dir_fd_bits: u64,
    dst_bits: u64,
    dst_dir_fd_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("os.replace_at", "fs.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.write capability",
            );
        }
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        #[cfg(unix)]
        {
            let src_dir_fd = match at_resolve_dir_fd(_py, src_dir_fd_bits) {
                Ok(fd) => fd,
                Err(bits) => return bits,
            };
            let dst_dir_fd = match at_resolve_dir_fd(_py, dst_dir_fd_bits) {
                Ok(fd) => fd,
                Err(bits) => return bits,
            };
            let c_src = match at_path_to_cstring(_py, &src) {
                Ok(c) => c,
                Err(bits) => return bits,
            };
            let c_dst = match at_path_to_cstring(_py, &dst) {
                Ok(c) => c,
                Err(bits) => return bits,
            };
            let rc =
                unsafe { libc::renameat(src_dir_fd, c_src.as_ptr(), dst_dir_fd, c_dst.as_ptr()) };
            if rc < 0 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "replace");
            }
            MoltObject::none().bits()
        }
        #[cfg(not(unix))]
        {
            let _ = (src, dst, src_dir_fd_bits, dst_dir_fd_bits);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "replace")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_replace_at(_s: u64, _sd: u64, _d: u64, _dd: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "replace")
    })
}

/// `os.link(src, src_dir_fd=..., dst, dst_dir_fd=..., follow_symlinks=...)` → None.
///
/// `linkat(olddirfd, oldpath, newdirfd, newpath, flags)`; `follow_symlinks=True`
/// (CPython default) sets `AT_SYMLINK_FOLLOW`. On Linux, an empty `src` path
/// would need `AT_EMPTY_PATH` + `CAP_DAC_READ_SEARCH`; we do not pre-raise —
/// the kernel's `EPERM`/`ENOENT` is propagated as the matching OSError subclass.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_link_at(
    src_bits: u64,
    src_dir_fd_bits: u64,
    dst_bits: u64,
    dst_dir_fd_bits: u64,
    follow_symlinks_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("os.link_at", "fs.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.write capability",
            );
        }
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        #[cfg(unix)]
        {
            let src_dir_fd = match at_resolve_dir_fd(_py, src_dir_fd_bits) {
                Ok(fd) => fd,
                Err(bits) => return bits,
            };
            let dst_dir_fd = match at_resolve_dir_fd(_py, dst_dir_fd_bits) {
                Ok(fd) => fd,
                Err(bits) => return bits,
            };
            let follow = is_truthy(_py, obj_from_bits(follow_symlinks_bits));
            let flags = if follow { libc::AT_SYMLINK_FOLLOW } else { 0 };
            let c_src = match at_path_to_cstring(_py, &src) {
                Ok(c) => c,
                Err(bits) => return bits,
            };
            let c_dst = match at_path_to_cstring(_py, &dst) {
                Ok(c) => c,
                Err(bits) => return bits,
            };
            let rc = unsafe {
                libc::linkat(
                    src_dir_fd,
                    c_src.as_ptr(),
                    dst_dir_fd,
                    c_dst.as_ptr(),
                    flags,
                )
            };
            if rc < 0 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "link");
            }
            MoltObject::none().bits()
        }
        #[cfg(not(unix))]
        {
            let _ = (
                src,
                dst,
                src_dir_fd_bits,
                dst_dir_fd_bits,
                follow_symlinks_bits,
            );
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "link")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_link_at(_s: u64, _sd: u64, _d: u64, _dd: u64, _f: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "link")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getfilesystemencodeerrors() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_string(
            _py,
            crate::builtins::platform::startup_encoding(_py)
                .filesystem_errors
                .as_bytes(),
        );
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_open_flags() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let flags: &[i64] = &[
            libc::O_RDONLY as i64,
            libc::O_WRONLY as i64,
            libc::O_RDWR as i64,
            libc::O_APPEND as i64,
            libc::O_CREAT as i64,
            libc::O_TRUNC as i64,
            libc::O_EXCL as i64,
            #[cfg(any(unix, target_arch = "wasm32"))]
            {
                libc::O_NONBLOCK as i64
            },
            #[cfg(windows)]
            {
                0i64
            },
            #[cfg(all(not(windows), not(target_arch = "wasm32")))]
            {
                libc::O_CLOEXEC as i64
            },
            #[cfg(windows)]
            {
                libc::O_NOINHERIT as i64
            },
            #[cfg(target_arch = "wasm32")]
            {
                0i64
            },
        ];
        let mut bits = Vec::with_capacity(flags.len());
        for &f in flags {
            bits.push(int_bits_from_i64(_py, f));
        }
        let ptr = alloc_tuple(_py, &bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_open(path_bits: u64, flags_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        // os.open(path, flags: int, mode: int = 0o777): CPython's `int` converter.
        let Some(flags) = crate::builtins::numbers::index_c_int_from_obj(_py, flags_bits) else {
            return MoltObject::none().bits();
        };
        let Some(mode) = crate::builtins::numbers::index_c_int_from_obj(_py, mode_bits) else {
            return MoltObject::none().bits();
        };
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "open")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            let rc = unsafe {
                use std::os::unix::ffi::OsStrExt;
                let c_path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
                    Ok(val) => val,
                    Err(_) => {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::ValueError,
                            "embedded null byte in path",
                        );
                    }
                };
                libc::open(c_path.as_ptr(), flags as libc::c_int, mode as libc::c_uint)
            };
            #[cfg(windows)]
            let rc = unsafe {
                let wide: Vec<u16> = path
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
                libc::wopen(
                    wide.as_ptr() as *const libc::wchar_t,
                    flags as libc::c_int,
                    mode as libc::c_int,
                )
            };
            #[cfg(not(any(unix, windows)))]
            let rc = -1i32;
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "open");
                }
                return raise_os_error::<u64>(_py, err, "open");
            }
            int_bits_from_i64(_py, rc as i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_close(fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
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
            MoltObject::none().bits()
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
                raise_os_error::<u64>(_py, err, "close")
            }
            #[cfg(windows)]
            {
                let sock_rc = unsafe { closesocket(fd as usize) };
                if sock_rc == 0 {
                    return MoltObject::none().bits();
                }
                let sock_err = unsafe { WSAGetLastError() };
                if sock_err == WSAENOTSOCK {
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
                raise_os_error_errno::<u64>(_py, sock_err as i64, "close")
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_read(fd_bits: u64, len_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // os.read(fd: int, length: Py_ssize_t): CPython's argument converters.
        let Some(fd) = crate::builtins::numbers::index_c_int_from_obj(_py, fd_bits) else {
            return MoltObject::none().bits();
        };
        let len = crate::builtins::numbers::index_ssize_from_obj(_py, len_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "read");
        }
        if len < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EINVAL as i64, "read");
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "read")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Like CPython's PyBytes_FromStringAndSize(NULL, length): an
            // unsatisfiable request is MemoryError, and the buffer is not
            // zero-filled before read(2) writes it.
            let mut buf: Vec<u8> = Vec::new();
            if buf.try_reserve_exact(len as usize).is_err() {
                crate::record_memory_error_without_allocation(_py);
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
            }
            #[cfg(unix)]
            let rc = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, len as usize) };
            #[cfg(windows)]
            let rc = unsafe {
                libc::read(
                    fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    (len as usize).min(u32::MAX as usize) as libc::c_uint,
                )
            } as isize;
            #[cfg(not(any(unix, windows)))]
            let rc = -1isize;
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "read");
                }
                return raise_os_error::<u64>(_py, err, "read");
            }
            // SAFETY: read(2) initialized the first `rc` bytes of the reserved
            // capacity, and `rc <= len <= capacity`.
            unsafe { buf.set_len(rc as usize) };
            let ptr = alloc_bytes(_py, &buf);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_write(fd_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fd) = crate::builtins::numbers::index_c_int_from_obj(_py, fd_bits) else {
            return MoltObject::none().bits();
        };
        let input = match crate::object::buffer_exports::ScopedBuffer::simple(_py, data_bits) {
            Ok(input) => input,
            Err(error) => return error.raise(_py, "a bytes-like object is required"),
        };
        let len = match input.contiguous_len() {
            Ok(len) => len,
            Err(error) => return error.raise(_py, "a bytes-like object is required"),
        };
        let data = input.view().data;
        // Clinic converts the descriptor, then exports data. EBADF is an
        // operation failure, so exporter callbacks precede a negative fd error.
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "write");
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "write")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            let rc = unsafe { libc::write(fd as libc::c_int, data as *const libc::c_void, len) };
            #[cfg(windows)]
            let rc = unsafe {
                libc::write(
                    fd as libc::c_int,
                    data as *const libc::c_void,
                    len.min(u32::MAX as usize) as libc::c_uint,
                )
            } as isize;
            #[cfg(not(any(unix, windows)))]
            let rc = -1isize;
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "write");
                }
                return raise_os_error::<u64>(_py, err, "write");
            }
            int_bits_from_i64(_py, rc as i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_pipe() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "pipe")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            {
                let mut fds = [0 as libc::c_int; 2];
                if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "pipe");
                    }
                    return raise_os_error::<u64>(_py, err, "pipe");
                }

                let set_cloexec = |fd: libc::c_int| -> Result<(), std::io::Error> {
                    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
                    if flags < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if (flags & libc::FD_CLOEXEC) != 0 {
                        return Ok(());
                    }
                    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                };

                if let Err(err) = set_cloexec(fds[0]).and_then(|_| set_cloexec(fds[1])) {
                    let _ = unsafe { libc::close(fds[0]) };
                    let _ = unsafe { libc::close(fds[1]) };
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "pipe");
                    }
                    return raise_os_error::<u64>(_py, err, "pipe");
                }

                let read_bits = int_bits_from_i64(_py, fds[0] as i64);
                let write_bits = int_bits_from_i64(_py, fds[1] as i64);
                let tuple_ptr = alloc_tuple(_py, &[read_bits, write_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
            #[cfg(not(unix))]
            {
                raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "pipe")
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_dup(fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "dup");
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "dup")
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_get_inheritable(fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "get_inheritable");
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_inheritable")
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
                MoltObject::from_bool(inheritable).bits()
            }
            #[cfg(windows)]
            {
                let handle = unsafe { libc::get_osfhandle(fd as libc::c_int) };
                if handle == -1 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "get_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "get_inheritable");
                }
                let mut flags: u32 = 0;
                let ok =
                    unsafe { GetHandleInformation(handle as *mut std::ffi::c_void, &mut flags) };
                if ok == 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "get_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "get_inheritable");
                }
                MoltObject::from_bool((flags & HANDLE_FLAG_INHERIT) != 0).bits()
            }
            #[cfg(not(any(unix, windows)))]
            {
                raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_inheritable")
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_set_inheritable(fd_bits: u64, inheritable_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "set_inheritable");
        }
        let inheritable = is_truthy(_py, obj_from_bits(inheritable_bits));
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "set_inheritable")
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
                MoltObject::none().bits()
            }
            #[cfg(windows)]
            {
                let handle = unsafe { libc::get_osfhandle(fd as libc::c_int) };
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
                MoltObject::none().bits()
            }
            #[cfg(not(any(unix, windows)))]
            {
                raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "set_inheritable")
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_urandom(len_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, len_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(len) = index_i64_with_overflow(
            _py,
            len_bits,
            &msg,
            Some("Python int too large to convert to C ssize_t"),
        ) else {
            return MoltObject::none().bits();
        };
        if len < 0 {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "negative argument not allowed",
            );
        }
        let len = match usize::try_from(len) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::OverflowError,
                    "Python int too large to convert to C ssize_t",
                );
            }
        };
        if let Err(err) =
            crate::require_operation(_py, crate::OperationId::RandomEntropy, AuditArgs::None)
        {
            return err;
        }
        let mut buf = Vec::new();
        if buf.try_reserve_exact(len).is_err() {
            return {
                crate::record_memory_error_without_allocation(_py);
                crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
            };
        }
        buf.resize(len, 0);
        if let Err(err) = fill_os_random(&mut buf) {
            let msg = format!("urandom failed: {err}");
            return raise_exception::<_>(_py, BuiltinExc::OSError, &msg);
        }
        let ptr = alloc_bytes(_py, &buf);
        if ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
