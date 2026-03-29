//! Path, glob, and OS filesystem operations.
//!
//! Split from io.rs to reduce file size. Contains all `molt_path_*`,
//! `molt_glob*`, `molt_os_*`, and `molt_getcwd` extern functions.

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

#[cfg(unix)]
use super::io::{
    PathFlavor, alloc_path_list_bits, alloc_string_list_bits, bytes_sequence_from_bits,
    bytes_slice_from_bits, collect_bytes_like, create_symlink_path, dup_fd,
    filesystem_encode_errors, filesystem_encoding, fspath_bits_with_flavor,
    glob_dir_fd_arg_from_bits, glob_escape_text, glob_has_magic_text, glob_matches_text,
    glob_translate_text, path_abspath_text, path_as_uri_text, path_basename_text,
    path_compare_text, path_dirname_text, path_expandvars_text, path_expandvars_with_lookup,
    path_from_bits, path_glob_matches, path_isabs_text, path_join_many_text, path_join_raw,
    path_join_text, path_match_text, path_name_text, path_normpath_text, path_parents_text,
    path_parts_text, path_relative_to_text, path_relpath_text, path_resolve_text, path_sep_char,
    path_sequence_from_bits, path_splitext_text, path_splitroot_text, path_stem_text,
    path_str_arg_from_bits, path_string_from_bits, path_string_with_flavor_from_bits,
    path_suffix_text, path_suffixes_text, raise_io_error_for_glob, raw_from_bytes_text,
};
use crate::PyToken;
use crate::audit::{AuditArgs, audit_capability_decision};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::randomness::fill_os_random;
use crate::*;
use std::collections::HashMap;
use std::io::ErrorKind;

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_exists(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        MoltObject::from_bool(std::fs::metadata(path).is_ok()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let is_dir = std::fs::metadata(path)
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        MoltObject::from_bool(is_dir).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isfile(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let is_file = std::fs::metadata(path)
            .map(|meta| meta.is_file())
            .unwrap_or(false);
        MoltObject::from_bool(is_file).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_islink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let is_link = std::fs::symlink_metadata(path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        MoltObject::from_bool(is_link).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_readlink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::read_link(&path) {
            Ok(target) => {
                let text = target.to_string_lossy();
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
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_symlink(
    src_bits: u64,
    dst_bits: u64,
    target_is_directory_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let target_is_directory = is_truthy(_py, obj_from_bits(target_is_directory_bits));
        match create_symlink_path(&src, &dst, target_is_directory) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => raise_exception::<_>(_py, "FileExistsError", &msg),
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::Unsupported => {
                        raise_exception::<_>(_py, "NotImplementedError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_listdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
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
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_mkdir(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mode = index_i64_from_obj(_py, mode_bits, "mkdir() mode must be int");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let mode = mode as u32;
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            builder.mode(mode);
        }
        #[cfg(windows)]
        {
            let _ = mode;
        }
        match builder.create(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => raise_exception::<_>(_py, "FileExistsError", &msg),
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_unlink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
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
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::IsADirectory => raise_exception::<_>(_py, "IsADirectoryError", &msg),
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_rmdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
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
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::DirectoryNotEmpty => raise_exception::<_>(_py, "OSError", &msg),
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_join(base_bits: u64, part_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Detect bytes inputs — CPython returns bytes when given bytes.
        if let Some(base_ptr) = obj_from_bits(base_bits).as_ptr()
            && unsafe { object_type_id(base_ptr) } == TYPE_ID_BYTES
        {
            let base_len = unsafe { bytes_len(base_ptr) };
            let base_raw = unsafe { std::slice::from_raw_parts(bytes_data(base_ptr), base_len) };
            let part_raw = match bytes_slice_from_bits(part_bits) {
                Some(s) => s,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "join: expected bytes for path component",
                    );
                }
            };
            let out = path_join_raw(base_raw, &part_raw, b'/');
            let ptr = alloc_bytes(_py, &out);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        }
        let sep = path_sep_char();
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let part = match path_string_from_bits(_py, part_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_join_text(base, &part, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_join_many(base_bits: u64, parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Detect bytes inputs — CPython returns bytes when given bytes.
        if let Some(base_ptr) = obj_from_bits(base_bits).as_ptr()
            && unsafe { object_type_id(base_ptr) } == TYPE_ID_BYTES
        {
            let base_len = unsafe { bytes_len(base_ptr) };
            let base_raw = unsafe { std::slice::from_raw_parts(bytes_data(base_ptr), base_len) };
            let raw_parts = match bytes_sequence_from_bits(_py, parts_bits, "parts") {
                Ok(parts) => parts,
                Err(bits) => return bits,
            };
            let mut out = base_raw.to_vec();
            for part in &raw_parts {
                out = path_join_raw(&out, part, b'/');
            }
            let ptr = alloc_bytes(_py, &out);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        }
        let sep = path_sep_char();
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let parts = match path_sequence_from_bits(_py, parts_bits, "parts") {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let out = path_join_many_text(base, &parts, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isabs(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
pub extern "C" fn molt_path_resolve(path_bits: u64, strict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let strict = is_truthy(_py, obj_from_bits(strict_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let out = match path_resolve_text(_py, &path, sep, strict) {
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
    crate::with_gil_entry!(_py, {
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
pub extern "C" fn molt_path_expandvars(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_expandvars_text(_py, &path) {
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
pub extern "C" fn molt_path_expandvars_env(path_bits: u64, env_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let Some(env_ptr) = obj_from_bits(env_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "env must be dict[str, str]");
        };
        if unsafe { object_type_id(env_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "env must be dict[str, str]");
        }
        let mut env_map: HashMap<String, String> = HashMap::new();
        let pairs = unsafe { dict_order(env_ptr) };
        for chunk in pairs.chunks(2) {
            if chunk.len() < 2 {
                continue;
            }
            let Some(key) = string_obj_to_owned(obj_from_bits(chunk[0])) else {
                return raise_exception::<_>(_py, "TypeError", "env keys must be str");
            };
            let Some(value) = string_obj_to_owned(obj_from_bits(chunk[1])) else {
                return raise_exception::<_>(_py, "TypeError", "env values must be str");
            };
            env_map.insert(key, value);
        }
        let out = path_expandvars_with_lookup(&path, |name| env_map.get(name).cloned());
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

    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mode = index_i64_from_obj(_py, mode_bits, "makedirs() mode must be int");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
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
                    return raise_exception::<_>(_py, "FileExistsError", &msg);
                }
                let msg = format!("File exists: {}", path.to_string_lossy());
                return raise_exception::<_>(_py, "FileExistsError", &msg);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                let msg = err.to_string();
                return match err.kind() {
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
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
                ErrorKind::AlreadyExists => raise_exception::<_>(_py, "FileExistsError", &msg),
                ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                ErrorKind::PermissionDenied => raise_exception::<_>(_py, "PermissionError", &msg),
                _ => raise_exception::<_>(_py, "OSError", &msg),
            };
        }
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            builder.mode(mode);
        }
        #[cfg(windows)]
        {
            let _ = mode;
        }
        match builder.create(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => {
                        if exist_ok {
                            match std::fs::metadata(&path) {
                                Ok(meta) if meta.is_dir() => MoltObject::none().bits(),
                                _ => raise_exception::<_>(_py, "FileExistsError", &msg),
                            }
                        } else {
                            raise_exception::<_>(_py, "FileExistsError", &msg)
                        }
                    }
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_parts(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let parts = path_parts_text(&path, sep);
        let mut out_bits: Vec<u64> = Vec::with_capacity(parts.len());
        for part in parts {
            let ptr = alloc_string(_py, part.as_bytes());
            if ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, out_bits.as_slice());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_splitroot(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let (drive, root, tail) = path_splitroot_text(&path, sep);
        let drive_ptr = alloc_string(_py, drive.as_bytes());
        if drive_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let root_ptr = alloc_string(_py, root.as_bytes());
        if root_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(drive_ptr).bits());
            return MoltObject::none().bits();
        }
        let tail_ptr = alloc_string(_py, tail.as_bytes());
        if tail_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(drive_ptr).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(root_ptr).bits());
            return MoltObject::none().bits();
        }
        let drive_bits = MoltObject::from_ptr(drive_ptr).bits();
        let root_bits = MoltObject::from_ptr(root_ptr).bits();
        let tail_bits = MoltObject::from_ptr(tail_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[drive_bits, root_bits, tail_bits]);
        dec_ref_bits(_py, drive_bits);
        dec_ref_bits(_py, root_bits);
        dec_ref_bits(_py, tail_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_compare(lhs_bits: u64, rhs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let lhs = match path_string_from_bits(_py, lhs_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let rhs = match path_string_from_bits(_py, rhs_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        MoltObject::from_int(path_compare_text(&lhs, &rhs, sep)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_parents(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let parents = path_parents_text(&path, sep);
        let mut out_bits: Vec<u64> = Vec::with_capacity(parents.len());
        for parent in parents {
            let ptr = alloc_string(_py, parent.as_bytes());
            if ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, out_bits.as_slice());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_relative_to(path_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_relative_to_text(&path, &base, sep) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
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
pub extern "C" fn molt_path_relative_to_many(
    path_bits: u64,
    base_bits: u64,
    parts_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let parts = match path_sequence_from_bits(_py, parts_bits, "parts") {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let joined_base = path_join_many_text(base, &parts, sep);
        let out = match path_relative_to_text(&path, &joined_base, sep) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
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
pub extern "C" fn molt_path_with_name(path_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let name = match path_str_arg_from_bits(_py, name_bits, "name") {
            Ok(name) => name,
            Err(bits) => return bits,
        };
        #[cfg(windows)]
        let invalid_sep = name.contains('/') || name.contains('\\');
        #[cfg(not(windows))]
        let invalid_sep = name.contains(sep);
        if name.is_empty() || name == "." || invalid_sep {
            let msg = format!("Invalid name {name:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let current = path_basename_text(&path, sep);
        if current.is_empty() || current == "." {
            let msg = format!("{path:?} has an empty name");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let parent = path_dirname_text(&path, sep);
        let out = if parent.is_empty() || parent == "." {
            name
        } else {
            path_join_text(parent, &name, sep)
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
pub extern "C" fn molt_path_with_suffix(path_bits: u64, suffix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let suffix = match path_str_arg_from_bits(_py, suffix_bits, "suffix") {
            Ok(suffix) => suffix,
            Err(bits) => return bits,
        };
        if !suffix.is_empty() && !suffix.starts_with('.') {
            let msg = format!("Invalid suffix {suffix:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let name = path_basename_text(&path, sep);
        let (stem, _) = path_splitext_text(&name, sep);
        if stem.is_empty() {
            let msg = format!("{path:?} has an empty name");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let new_name = format!("{stem}{suffix}");
        let parent = path_dirname_text(&path, sep);
        let out = if parent.is_empty() || parent == "." {
            new_name
        } else {
            path_join_text(parent, &new_name, sep)
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
pub extern "C" fn molt_path_with_stem(path_bits: u64, stem_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let stem = match path_str_arg_from_bits(_py, stem_bits, "stem") {
            Ok(stem) => stem,
            Err(bits) => return bits,
        };
        let name = path_basename_text(&path, sep);
        if name.is_empty() || name == "." {
            let msg = format!("{path:?} has an empty name");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let suffix = path_suffix_text(&name, sep);
        let new_name = format!("{stem}{suffix}");
        #[cfg(windows)]
        let invalid_sep = new_name.contains('/') || new_name.contains('\\');
        #[cfg(not(windows))]
        let invalid_sep = new_name.contains(sep);
        if new_name.is_empty() || new_name == "." || invalid_sep {
            let msg = format!("Invalid name {new_name:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let parent = path_dirname_text(&path, sep);
        let out = if parent.is_empty() || parent == "." {
            new_name
        } else {
            path_join_text(parent, &new_name, sep)
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
pub extern "C" fn molt_path_is_relative_to(path_bits: u64, base_bits: u64, parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let target_base = if obj_from_bits(parts_bits).is_none() {
            base
        } else {
            let parts = match path_sequence_from_bits(_py, parts_bits, "parts") {
                Ok(parts) => parts,
                Err(bits) => return bits,
            };
            path_join_many_text(base, &parts, sep)
        };
        let is_relative = path_relative_to_text(&path, &target_base, sep).is_ok();
        MoltObject::from_bool(is_relative).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_expanduser(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        if !path.starts_with('~') {
            let ptr = alloc_string(_py, path.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let rest = if path == "~" {
            ""
        } else if path.starts_with(&format!("~{sep}")) {
            &path[2..]
        } else {
            let ptr = alloc_string(_py, path.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        };
        let env_allowed = has_capability(_py, "env.read");
        audit_capability_decision("env.expanduser", "env.read", AuditArgs::None, env_allowed);
        if !env_allowed {
            return raise_capability_denied(_py, "env.read");
        }
        let mut home = std::env::var("HOME").ok();
        if home.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
            home = std::env::var("USERPROFILE").ok();
        }
        if home.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
            let drive = std::env::var("HOMEDRIVE").ok();
            let homepath = std::env::var("HOMEPATH").ok();
            if let (Some(drive), Some(homepath)) = (drive, homepath)
                && !drive.is_empty()
                && !homepath.is_empty()
            {
                home = Some(format!("{drive}{homepath}"));
            }
        }
        let out = if let Some(mut home) = home {
            if !rest.is_empty() {
                home = home.trim_end_matches(sep).to_string();
                home.push(sep);
                home.push_str(rest);
            }
            home
        } else {
            path
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
pub extern "C" fn molt_path_name(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_name_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_suffix(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_suffix_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_stem(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_stem_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_suffixes(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let suffixes = path_suffixes_text(&path, sep);
        let mut out_bits: Vec<u64> = Vec::with_capacity(suffixes.len());
        for suffix in suffixes {
            let ptr = alloc_string(_py, suffix.as_bytes());
            if ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, out_bits.as_slice());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_as_uri(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_as_uri_text(&path, sep) {
            Ok(out) => out,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
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
    crate::with_gil_entry!(_py, {
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
pub extern "C" fn molt_path_glob(path_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        let dir = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let pattern = match path_str_arg_from_bits(_py, pattern_bits, "pattern") {
            Ok(pattern) => pattern,
            Err(bits) => return bits,
        };
        let sep = path_sep_char();
        let matches = match path_glob_matches(&dir, &pattern, sep) {
            Ok(values) => values,
            Err(err) => return raise_io_error_for_glob(_py, err),
        };
        alloc_string_list_bits(_py, &matches)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_has_magic(pathname_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let pathname = match path_string_from_bits(_py, pathname_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(glob_has_magic_text(&pathname)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_escape(pathname_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let (pathname, flavor) = match path_string_with_flavor_from_bits(_py, pathname_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let escaped = glob_escape_text(&pathname, sep);
        if flavor == PathFlavor::Bytes {
            let raw = raw_from_bytes_text(&escaped).unwrap_or_else(|| escaped.as_bytes().to_vec());
            let ptr = alloc_bytes(_py, raw.as_slice());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        } else {
            let ptr = alloc_string(_py, escaped.as_bytes());
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
    crate::with_gil_entry!(_py, {
        let pattern = if let Some(text) = string_obj_to_owned(obj_from_bits(pathname_bits)) {
            text
        } else {
            let type_id = obj_from_bits(pathname_bits)
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) });
            if matches!(type_id, Some(TYPE_ID_BYTES) | Some(TYPE_ID_BYTEARRAY)) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot use a string pattern on a bytes-like object",
                );
            }
            let type_name = class_name_for_error(type_of_bits(_py, pathname_bits));
            let msg = format!("expected string or bytes-like object, got '{type_name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
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
            return raise_exception::<_>(_py, "TypeError", "seps must be str or None");
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob(
    pathname_bits: u64,
    root_dir_bits: u64,
    dir_fd_bits: u64,
    recursive_bits: u64,
    include_hidden_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
        }
        let sep = path_sep_char();

        #[cfg(windows)]
        let (pathname, pathname_flavor) =
            match path_string_with_flavor_from_bits(_py, pathname_bits) {
                Ok((path, flavor)) => (path.replace('/', "\\"), flavor),
                Err(bits) => return bits,
            };
        #[cfg(not(windows))]
        let (pathname, pathname_flavor) =
            match path_string_with_flavor_from_bits(_py, pathname_bits) {
                Ok((path, flavor)) => (path, flavor),
                Err(bits) => return bits,
            };

        let root_dir = if obj_from_bits(root_dir_bits).is_none() {
            None
        } else {
            #[cfg(windows)]
            {
                match path_string_with_flavor_from_bits(_py, root_dir_bits) {
                    Ok((path, flavor)) => Some((path.replace('/', "\\"), flavor)),
                    Err(bits) => return bits,
                }
            }
            #[cfg(not(windows))]
            {
                match path_string_with_flavor_from_bits(_py, root_dir_bits) {
                    Ok((path, flavor)) => Some((path, flavor)),
                    Err(bits) => return bits,
                }
            }
        };

        if let Some((_, root_dir_flavor)) = root_dir.as_ref()
            && *root_dir_flavor != pathname_flavor
        {
            let msg = if path_isabs_text(&pathname, sep) {
                "Can't mix strings and bytes in path components"
            } else if pathname_flavor == PathFlavor::Bytes {
                "cannot use a bytes pattern on a string-like object"
            } else {
                "cannot use a string pattern on a bytes-like object"
            };
            return raise_exception::<_>(_py, "TypeError", msg);
        }

        let dir_fd = match glob_dir_fd_arg_from_bits(_py, dir_fd_bits) {
            Err(bits) => return bits,
            Ok(value) => value,
        };

        let bytes_mode = pathname_flavor == PathFlavor::Bytes;
        #[cfg(target_arch = "wasm32")]
        {
            let root_dir_is_absolute = root_dir
                .as_ref()
                .is_some_and(|(path, _)| path_isabs_text(path, sep));
            if let GlobDirFdArg::Int(fd) = dir_fd
                && glob_dir_fd_root_text(fd, bytes_mode).is_none()
                && !path_isabs_text(&pathname, sep)
                && !root_dir_is_absolute
            {
                return raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    "glob(dir_fd=...) requires fd-backed path resolution on this wasm host",
                );
            }
        }

        let recursive = is_truthy(_py, obj_from_bits(recursive_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let include_hidden = is_truthy(_py, obj_from_bits(include_hidden_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let root_ref = if let Some((root, _)) = root_dir.as_ref() {
            Some(root.as_str())
        } else {
            None
        };

        let out = match glob_matches_text(
            _py,
            &pathname,
            root_ref,
            &dir_fd,
            recursive,
            include_hidden,
            bytes_mode,
            sep,
        ) {
            Ok(values) => values,
            Err(bits) => return bits,
        };
        alloc_path_list_bits(_py, &out, bytes_mode)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_chmod(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_capability_denied(_py, "fs.write");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mode = index_i64_from_obj(_py, mode_bits, "chmod() mode must be int");
        if exception_pending(_py) {
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
                        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
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
                        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
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
                        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, "PermissionError", &msg)
                        }
                        _ => raise_exception::<_>(_py, "OSError", &msg),
                    }
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = mode;
            raise_exception::<_>(
                _py,
                "NotImplementedError",
                "chmod is unsupported on this platform",
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getcwd() -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_capability_denied(_py, "fs.read");
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

#[cfg(not(unix))]
fn unix_seconds_from_system_time(value: std::time::SystemTime) -> i128 {
    use std::time::UNIX_EPOCH;
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::from(duration.as_secs() as u64),
        Err(err) => -i128::from(err.duration().as_secs() as u64),
    }
}

#[cfg(not(unix))]
fn metadata_time_seconds(value: Result<std::time::SystemTime, std::io::Error>) -> i128 {
    match value {
        Ok(time) => unix_seconds_from_system_time(time),
        Err(_) => 0,
    }
}

fn stat_tuple_from_values(_py: &PyToken<'_>, fields: [i128; 10]) -> u64 {
    let tuple_fields = fields.map(|value| int_bits_from_i128(_py, value));
    let tuple_ptr = alloc_tuple(_py, &tuple_fields);
    if tuple_ptr.is_null() {
        return raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

#[cfg(unix)]
fn stat_tuple_from_metadata(_py: &PyToken<'_>, metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    stat_tuple_from_values(
        _py,
        [
            i128::from(metadata.mode()),
            metadata.ino() as i128,
            metadata.dev() as i128,
            metadata.nlink() as i128,
            i128::from(metadata.uid()),
            i128::from(metadata.gid()),
            metadata.size() as i128,
            i128::from(metadata.atime()),
            i128::from(metadata.mtime()),
            i128::from(metadata.ctime()),
        ],
    )
}

#[cfg(not(unix))]
fn stat_tuple_from_metadata(_py: &PyToken<'_>, metadata: &std::fs::Metadata) -> u64 {
    let is_dir = metadata.is_dir();
    #[cfg(windows)]
    let kind = if is_dir {
        i128::from(libc::S_IFDIR as i64)
    } else {
        i128::from(libc::S_IFREG as i64)
    };
    #[cfg(not(windows))]
    let kind = 0i128;
    let mode_bits = if metadata.permissions().readonly() {
        if is_dir { 0o555 } else { 0o444 }
    } else if is_dir {
        0o777
    } else {
        0o666
    };
    stat_tuple_from_values(
        _py,
        [
            kind | i128::from(mode_bits),
            0,
            0,
            0,
            0,
            0,
            i128::from(metadata.len()),
            metadata_time_seconds(metadata.accessed()),
            metadata_time_seconds(metadata.modified()),
            metadata_time_seconds(metadata.created()),
        ],
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_stat(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::metadata(&path) {
            Ok(metadata) => stat_tuple_from_metadata(_py, &metadata),
            Err(err) => raise_os_error::<u64>(_py, err, "stat"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_lstat(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => stat_tuple_from_metadata(_py, &metadata),
            Err(err) => raise_os_error::<u64>(_py, err, "lstat"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_fstat(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "fstat");
        }
        #[cfg(unix)]
        {
            use std::mem::ManuallyDrop;
            use std::os::fd::FromRawFd;

            let raw_fd: libc::c_int = match libc::c_int::try_from(fd) {
                Ok(raw_fd) => raw_fd,
                Err(_) => return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "fstat"),
            };
            // SAFETY: we only borrow the descriptor for metadata lookup and prevent close via
            // ManuallyDrop so ownership of `raw_fd` stays with the caller.
            let file = unsafe { ManuallyDrop::new(std::fs::File::from_raw_fd(raw_fd)) };
            match file.metadata() {
                Ok(metadata) => stat_tuple_from_metadata(_py, &metadata),
                Err(err) => raise_os_error::<u64>(_py, err, "fstat"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = fd;
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "fstat")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_rename(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::rename(&src, &dst) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "rename"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_replace(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::rename(&src, &dst) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "replace"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_fsencode(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (fspath_bits, flavor) = match fspath_bits_with_flavor(_py, path_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if flavor == PathFlavor::Bytes {
            return fspath_bits;
        }

        let obj = obj_from_bits(fspath_bits);
        let Some(ptr) = obj.as_ptr() else {
            dec_ref_bits(_py, fspath_bits);
            return raise_exception::<_>(_py, "RuntimeError", "os fsencode received invalid path");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                dec_ref_bits(_py, fspath_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "os fsencode received invalid path",
                );
            }
            let raw = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
            let encoded = match crate::object::ops::encode_string_with_errors(
                raw,
                filesystem_encoding(),
                Some(filesystem_encode_errors()),
            ) {
                Ok(bytes) => bytes,
                Err(crate::object::ops::EncodeError::UnknownEncoding(name)) => {
                    dec_ref_bits(_py, fspath_bits);
                    let msg = format!("unknown encoding: {name}");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                Err(crate::object::ops::EncodeError::UnknownErrorHandler(name)) => {
                    dec_ref_bits(_py, fspath_bits);
                    let msg = format!("unknown error handler name '{name}'");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                Err(crate::object::ops::EncodeError::InvalidChar {
                    encoding,
                    code,
                    pos,
                    limit,
                }) => {
                    let reason = crate::object::ops::encode_error_reason(encoding, code, limit);
                    let exc_bits = raise_unicode_encode_error::<_>(
                        _py,
                        encoding,
                        fspath_bits,
                        pos,
                        pos + 1,
                        &reason,
                    );
                    dec_ref_bits(_py, fspath_bits);
                    return exc_bits;
                }
            };
            dec_ref_bits(_py, fspath_bits);
            let bytes_ptr = alloc_bytes(_py, encoded.as_slice());
            if bytes_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(bytes_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getfilesystemencodeerrors() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_string(_py, filesystem_encode_errors().as_bytes());
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_open_flags() -> u64 {
    crate::with_gil_entry!(_py, {
        let flags: &[i64] = &[
            libc::O_RDONLY as i64,
            libc::O_WRONLY as i64,
            libc::O_RDWR as i64,
            libc::O_APPEND as i64,
            libc::O_CREAT as i64,
            libc::O_TRUNC as i64,
            libc::O_EXCL as i64,
            libc::O_NONBLOCK as i64,
            #[cfg(not(target_arch = "wasm32"))]
            {
                libc::O_CLOEXEC as i64
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
    crate::with_gil_entry!(_py, {
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be an integer");
        };
        let Some(mode) = to_i64(obj_from_bits(mode_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "mode must be an integer");
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
                            "ValueError",
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
                libc::_wopen(wide.as_ptr(), flags as libc::c_int, mode as libc::c_int)
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_read(fd_bits: u64, len_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let Some(len) = to_i64(obj_from_bits(len_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, len_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "read");
        }
        if len < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EINVAL as i64, "read");
        }
        let mut buf = vec![0u8; len as usize];
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "read")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            let rc = unsafe {
                libc::read(
                    fd as libc::c_int,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            #[cfg(windows)]
            let rc = unsafe {
                libc::_read(
                    fd as libc::c_int,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len().min(i32::MAX as usize) as u32,
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
            buf.truncate(rc as usize);
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
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "write");
        }
        let bytes = match unsafe { collect_bytes_like(_py, data_bits) } {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "write")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            let rc = unsafe {
                libc::write(
                    fd as libc::c_int,
                    bytes.as_ptr() as *const libc::c_void,
                    bytes.len(),
                )
            };
            #[cfg(windows)]
            let rc = unsafe {
                libc::_write(
                    fd as libc::c_int,
                    bytes.as_ptr() as *const libc::c_void,
                    bytes.len().min(i32::MAX as usize) as u32,
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
    crate::with_gil_entry!(_py, {
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
                return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "pipe");
            }
        }
    })
}

#[unsafe(no_mangle)]
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
                let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
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
                return MoltObject::from_bool((flags & HANDLE_FLAG_INHERIT) != 0).bits();
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_urandom(len_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
            return raise_exception::<_>(_py, "ValueError", "negative argument not allowed");
        }
        let len = match usize::try_from(len) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                );
            }
        };
        let mut buf = Vec::new();
        if buf.try_reserve_exact(len).is_err() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        buf.resize(len, 0);
        if let Err(err) = fill_os_random(&mut buf) {
            let msg = format!("urandom failed: {err}");
            return raise_exception::<_>(_py, "OSError", &msg);
        }
        let ptr = alloc_bytes(_py, &buf);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
