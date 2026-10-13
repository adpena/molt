//! Secure temporary creation in explicit directories. Python owns default selection/cache.

use crate::audit::AuditArgs;
use crate::builtins::io::{
    PathFlavor, path_abspath_path, path_abspath_text, path_from_bits_with_flavor, path_sep_char,
};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::*;
use std::io::Write;
use std::path::{Path, PathBuf};

fn path_result(py: &PyToken<'_>, path: &Path, flavor: PathFlavor) -> u64 {
    let pointer = if flavor == PathFlavor::Bytes {
        #[cfg(unix)]
        use std::os::unix::ffi::OsStrExt;
        #[cfg(target_os = "wasi")]
        use std::os::wasi::ffi::OsStrExt;
        #[cfg(any(unix, target_os = "wasi"))]
        let bytes = path.as_os_str().as_bytes();
        #[cfg(not(any(unix, target_os = "wasi")))]
        let bytes = path.as_os_str().as_encoded_bytes();
        alloc_bytes(py, bytes)
    } else {
        let Some(text) = path.to_str() else {
            return raise_exception::<u64>(
                py,
                "UnicodeError",
                "temporary path is not valid Unicode",
            );
        };
        alloc_string(py, text.as_bytes())
    };
    if pointer.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(pointer).bits()
    }
}

fn creation_paths(
    py: &PyToken<'_>,
    suffix: u64,
    prefix: u64,
    dir: u64,
) -> Result<(PathBuf, PathBuf, PathBuf, PathFlavor), u64> {
    for (value, label) in [(prefix, "prefix"), (suffix, "suffix")] {
        if !obj_from_bits(value).as_ptr().is_some_and(|pointer| unsafe {
            matches!(object_type_id(pointer), TYPE_ID_STRING | TYPE_ID_BYTES)
        }) {
            return Err(raise_exception::<u64>(
                py,
                "TypeError",
                &format!("{label} must be str or bytes"),
            ));
        }
    }
    let (suffix, suffix_kind) = path_from_bits_with_flavor(py, suffix)?;
    let (prefix, prefix_kind) = path_from_bits_with_flavor(py, prefix)?;
    let (dir, kind) = path_from_bits_with_flavor(py, dir)?;
    if suffix_kind != kind || prefix_kind != kind {
        return Err(raise_exception::<u64>(
            py,
            "TypeError",
            "Can't mix bytes and non-bytes in path components.",
        ));
    }
    Ok((suffix, prefix, dir, kind))
}

/// Probe the Python-selected candidate order without caching or consulting host defaults.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_gettempdir(candidates_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation(py, OperationId::TempfileGettempdir, AuditArgs::None)
        {
            return error;
        }
        let Some(pointer) = obj_from_bits(candidates_bits).as_ptr() else {
            return raise_exception::<u64>(
                py,
                "TypeError",
                "temporary directory candidates must be a list",
            );
        };
        if unsafe { object_type_id(pointer) } != TYPE_ID_LIST {
            return raise_exception::<u64>(
                py,
                "TypeError",
                "temporary directory candidates must be a list",
            );
        }
        let Some(candidates) = (unsafe {
            crate::object::seq_access::snapshot(
                py,
                pointer,
                "temporary directory candidates allocation failed",
            )
        }) else {
            return MoltObject::none().bits();
        };
        for &candidate in candidates.iter() {
            let (path, _) = match path_from_bits_with_flavor(py, candidate) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let path = if path.as_os_str() != std::ffi::OsStr::new(".") {
                let Some(text) = path.to_str() else {
                    return raise_exception::<u64>(
                        py,
                        "UnicodeError",
                        "temporary directory candidate is not Unicode",
                    );
                };
                match path_abspath_text(py, text, path_sep_char()) {
                    Ok(absolute) => PathBuf::from(absolute),
                    Err(error) => return error,
                }
            } else {
                path
            };
            let Ok(mut probe) = tempfile::Builder::new().tempfile_in(&path) else {
                continue;
            };
            // Test writability, and require deletion to succeed before publishing the choice.
            let written = probe.write_all(b"blat");
            let removed = probe.close();
            if written.is_ok() && removed.is_ok() {
                return path_result(py, &path, PathFlavor::Str);
            }
        }
        // The Python policy reports its original candidate list and leaves tempdir unset.
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_mkdtemp(suffix: u64, prefix: u64, dir: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation(py, OperationId::TempfileMkdtemp, AuditArgs::None) {
            return error;
        }
        let (suffix, prefix, dir, kind) = match creation_paths(py, suffix, prefix, dir) {
            Ok(value) => value,
            Err(error) => return error,
        };
        match tempfile::Builder::new()
            .prefix(prefix.as_os_str())
            .suffix(suffix.as_os_str())
            .tempdir_in(&dir)
        {
            Ok(directory) => {
                // CPython creates mkdtemp at the requested path before
                // normalizing the returned name (unlike mkstemp below).
                let absolute = match path_abspath_path(py, directory.path()) {
                    Ok(path) => path,
                    Err(error) => return error,
                };
                let result = path_result(py, &absolute, kind);
                if obj_from_bits(result).is_none() {
                    return result;
                }
                // Only transfer filesystem ownership after the Python result is allocated.
                let _ = directory.keep();
                result
            }
            Err(error) => raise_os_error::<u64>(py, error, "mkdtemp"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_mkstemp(suffix: u64, prefix: u64, dir: u64, text: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation(py, OperationId::TempfileMkstemp, AuditArgs::None) {
            return error;
        }
        let text = is_truthy(py, obj_from_bits(text));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let (suffix, prefix, dir, kind) = match creation_paths(py, suffix, prefix, dir) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let dir = match path_abspath_path(py, &dir) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let named = match tempfile::Builder::new()
            .prefix(prefix.as_os_str())
            .suffix(suffix.as_os_str())
            .tempfile_in(&dir)
        {
            Ok(named) => named,
            Err(error) => return raise_os_error::<u64>(py, error, "mkstemp"),
        };
        let path_bits = path_result(py, named.path(), kind);
        if obj_from_bits(path_bits).is_none() {
            return path_bits;
        }
        let path_owner = unsafe {
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), path_bits)
        };
        #[cfg(unix)]
        let fd = {
            use std::os::fd::AsRawFd;
            let _ = text;
            named.as_file().as_raw_fd() as i64
        };
        #[cfg(windows)]
        let fd = {
            use std::os::windows::io::AsRawHandle;
            match crate::builtins::io::windows_crt_fd_from_handle(
                named.as_file().as_raw_handle(),
                true,
                true,
                text,
            ) {
                Some(fd) => fd,
                None => {
                    return raise_os_error::<u64>(py, std::io::Error::last_os_error(), "mkstemp");
                }
            }
        };
        let result = alloc_tuple(py, &[MoltObject::from_int(fd).bits(), path_owner.bits()]);
        if result.is_null() {
            #[cfg(windows)]
            unsafe {
                libc::close(fd as libc::c_int);
            }
            return MoltObject::none().bits();
        }
        match named.keep() {
            Ok((file, _path)) => {
                #[cfg(unix)]
                {
                    use std::os::fd::IntoRawFd;
                    let _ = file.into_raw_fd();
                }
                #[cfg(windows)]
                drop(file); // the CRT descriptor owns its duplicated HANDLE
                MoltObject::from_ptr(result).bits()
            }
            Err(error) => {
                #[cfg(windows)]
                unsafe {
                    libc::close(fd as libc::c_int);
                }
                dec_ref_bits(py, MoltObject::from_ptr(result).bits());
                raise_os_error::<u64>(py, error.error, "mkstemp")
            }
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_mkstemp(_suffix: u64, _prefix: u64, _dir: u64, _text: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation(py, OperationId::TempfileMkstemp, AuditArgs::None) {
            return error;
        }
        // Guest WASI descriptors cannot escape into os.close's host descriptor namespace.
        raise_os_error_errno::<u64>(py, libc::ENOSYS as i64, "mkstemp")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tempfile_cleanup(path: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation(py, OperationId::TempfileCleanup, AuditArgs::None) {
            return error;
        }
        let (path, _) = match path_from_bits_with_flavor(py, path) {
            Ok(value) => value,
            Err(error) => return error,
        };
        match std::fs::remove_dir_all(path) {
            Ok(()) => MoltObject::none().bits(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => MoltObject::none().bits(),
            Err(error) => raise_os_error::<u64>(py, error, "TemporaryDirectory cleanup"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(py: &PyToken<'_>, value: &str) -> u64 {
        let pointer = alloc_string(py, value.as_bytes());
        assert!(!pointer.is_null());
        MoltObject::from_ptr(pointer).bits()
    }

    #[test]
    fn candidate_probe_selects_writable_directory_and_retires_probe() {
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
            "env.read,fs.read,fs.write",
            |py| {
                let root = tempfile::tempdir().unwrap();
                let file = root.path().join("not-a-directory");
                std::fs::write(&file, b"sentinel").unwrap();
                let items = [
                    text(py, root.path().join("missing").to_str().unwrap()),
                    text(py, file.to_str().unwrap()),
                    text(py, root.path().to_str().unwrap()),
                ];
                let list = alloc_list(py, &items);
                assert!(!list.is_null());
                let result = molt_tempfile_gettempdir(MoltObject::from_ptr(list).bits());
                assert!(!exception_pending(py));
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(result)),
                    string_obj_to_owned(obj_from_bits(items[2]))
                );
                assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
                assert_eq!(std::fs::read(&file).unwrap(), b"sentinel");
                dec_ref_bits(py, result);
                dec_ref_bits(py, MoltObject::from_ptr(list).bits());
                for item in items {
                    dec_ref_bits(py, item);
                }
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn creation_normalizes_opaque_byte_paths() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
            "fs.read,fs.write",
            |py| {
                let root = tempfile::tempdir().unwrap();
                let mut dirname = root.path().as_os_str().as_bytes().to_vec();
                dirname.extend_from_slice(b"/opaque-\xff");
                let directory =
                    std::path::PathBuf::from(std::ffi::OsString::from_vec(dirname.clone()));
                std::fs::create_dir(&directory).unwrap();
                let mut dotted = dirname.clone();
                dotted.extend_from_slice(b"/../opaque-\xff/.");
                let dir = MoltObject::from_ptr(alloc_bytes(py, &dotted)).bits();
                let empty = MoltObject::from_ptr(alloc_bytes(py, b"")).bits();
                let result = molt_tempfile_mkdtemp(empty, empty, dir);
                assert!(!exception_pending(py));
                let (created, flavor) = path_from_bits_with_flavor(py, result).unwrap();
                assert_eq!(flavor, PathFlavor::Bytes);
                assert_eq!(created.parent(), Some(directory.as_path()));
                assert!(created.exists());
                std::fs::remove_dir(created).unwrap();
                dec_ref_bits(py, result);
                let result =
                    molt_tempfile_mkstemp(empty, empty, dir, MoltObject::from_bool(false).bits());
                assert!(!exception_pending(py));
                let rows = unsafe {
                    crate::object::seq_access::snapshot(
                        py,
                        obj_from_bits(result).as_ptr().unwrap(),
                        "test tuple",
                    )
                }
                .unwrap();
                let (created, flavor) = path_from_bits_with_flavor(py, rows[1]).unwrap();
                assert_eq!(flavor, PathFlavor::Bytes);
                assert_eq!(created.parent(), Some(directory.as_path()));
                let fd = to_i64(obj_from_bits(rows[0])).unwrap();
                assert_eq!(unsafe { libc::close(fd as libc::c_int) }, 0);
                std::fs::remove_file(created).unwrap();
                for value in [result, empty, dir] {
                    dec_ref_bits(py, value);
                }
            },
        );
    }

    #[test]
    fn candidate_probe_denial_reports_complete_missing_set_before_path_access() {
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("", |py| {
            // An invalid candidate container would raise TypeError if inspected first.
            molt_tempfile_gettempdir(MoltObject::none().bits());
            assert!(exception_pending(py));
            let error = molt_exception_last();
            molt_exception_clear();
            let message = crate::builtins::exceptions::molt_exception_message(error);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(message)).unwrap(),
                "missing [env.read, fs.read, fs.write] capabilities for 'tempfile.gettempdir'"
            );
            dec_ref_bits(py, message);
            dec_ref_bits(py, error);
        });
    }
}
