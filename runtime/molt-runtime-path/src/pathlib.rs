#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/pathlib.rs ===
//
// pathlib intrinsics: PurePath / Path operations delegated to Rust std::path.
//
// Every public Python-visible method on PurePosixPath, PureWindowsPath, and
// Path is backed by a Rust intrinsic so the stdlib module contains zero
// Python-only logic.

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

use crate::bridge::*;
use molt_obj_model::MoltObject;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[inline]
fn str_bits(py: &CoreGilToken, s: &str) -> u64 {
    let ptr = alloc_string(py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

#[inline]
fn bool_bits(val: bool) -> u64 {
    MoltObject::from_bool(val).bits()
}

#[inline]
fn audit_path_arg(bits: u64) -> AuditArg {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(s) => AuditArg::Path(s),
        None => AuditArg::None,
    }
}

fn require_str(py: &CoreGilToken, bits: u64, label: &str) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(s) => Ok(s),
        None => Err(raise_exception::<u64>(
            py,
            "TypeError",
            &format!("{label} must be str"),
        )),
    }
}

fn list_of_strings(py: &CoreGilToken, items: &[String]) -> u64 {
    let bits: Vec<u64> = items.iter().map(|s| str_bits(py, s)).collect();
    let ptr = alloc_list(py, &bits);
    if ptr.is_null() {
        raise_exception::<u64>(py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn tuple_of_strings(py: &CoreGilToken, items: &[String]) -> u64 {
    let bits: Vec<u64> = items.iter().map(|s| str_bits(py, s)).collect();
    let ptr = alloc_tuple(py, &bits);
    if ptr.is_null() {
        raise_exception::<u64>(py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// Normalize a Windows path string to forward slashes for internal representation.
fn normalize_win_separators(s: &str) -> String {
    s.replace('\\', "/")
}

/// Split a path into (drive, root, tail) per CPython _splitroot semantics.
fn splitroot(path: &str, posix: bool) -> (String, String, String) {
    if posix {
        if path.starts_with("//") && !path.starts_with("///") {
            // POSIX two-slash root
            let rest = &path[2..];
            let idx = rest.find('/').unwrap_or(rest.len());
            let drive = String::new();
            let root = format!("//{}", &rest[..idx]);
            let tail = if idx < rest.len() {
                rest[idx..].to_string()
            } else {
                String::new()
            };
            return (drive, root, tail);
        }
        if let Some(stripped) = path.strip_prefix('/') {
            return (String::new(), "/".to_string(), stripped.to_string());
        }
        return (String::new(), String::new(), path.to_string());
    }
    // Windows flavor
    let norm = normalize_win_separators(path);
    let p = norm.as_str();
    // UNC path: //server/share
    if let Some(rest) = p.strip_prefix("//") {
        let idx = rest.find('/').unwrap_or(rest.len());
        let server = &rest[..idx];
        let after_server = if idx < rest.len() {
            &rest[idx + 1..]
        } else {
            ""
        };
        let idx2 = after_server.find('/').unwrap_or(after_server.len());
        let share = &after_server[..idx2];
        let drive = format!("//{server}/{share}");
        let tail_start = if idx2 < after_server.len() {
            &after_server[idx2..]
        } else {
            ""
        };
        let (root, tail) = if let Some(stripped) = tail_start.strip_prefix('/') {
            ("/".to_string(), stripped.to_string())
        } else {
            (String::new(), tail_start.to_string())
        };
        return (drive, root, tail);
    }
    // Drive letter: C:/
    if p.len() >= 2 && p.as_bytes()[0].is_ascii_alphabetic() && p.as_bytes()[1] == b':' {
        let drive = p[..2].to_string();
        let rest = &p[2..];
        if let Some(stripped) = rest.strip_prefix('/') {
            return (drive, "/".to_string(), stripped.to_string());
        }
        return (drive, String::new(), rest.to_string());
    }
    // Relative path
    if let Some(stripped) = p.strip_prefix('/') {
        return (String::new(), "/".to_string(), stripped.to_string());
    }
    (String::new(), String::new(), p.to_string())
}

fn path_parts(path: &str, posix: bool) -> Vec<String> {
    let (drive, root, tail) = splitroot(path, posix);
    let mut parts = Vec::new();
    let anchor = format!("{drive}{root}");
    if !anchor.is_empty() {
        parts.push(anchor);
    }
    for seg in tail.split('/') {
        if !seg.is_empty() {
            parts.push(seg.to_string());
        }
    }
    parts
}

// ---------------------------------------------------------------------------
// Public intrinsics -- Pure path operations
// ---------------------------------------------------------------------------

/// `pathlib._molt_path_str(path)` -> str  (normalized)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_str(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        if s.is_empty() {
            return str_bits(_py, ".");
        }
        str_bits(_py, &s)
    })
}

/// Returns the parts tuple for a path.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_parts(path_bits: u64, posix_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let parts = path_parts(&s, posix);
        tuple_of_strings(_py, &parts)
    })
}

/// `_splitroot(path, posix)` -> tuple[str, str, str]  (drive, root, tail)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_splitroot(path_bits: u64, posix_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let (drive, root, tail) = splitroot(&s, posix);
        let elems = [
            str_bits(_py, &drive),
            str_bits(_py, &root),
            str_bits(_py, &tail),
        ];
        let ptr = alloc_tuple(_py, &elems);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `path.__hash__()` -> int  (hash of the string representation)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_hash(path_bits: u64) -> u64 {
    // Hash the underlying string representation using the runtime hash.
    molt_object_hash(path_bits)
}

/// `path.__eq__(other)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_eq(path_bits: u64, other_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let other = match string_obj_to_owned(obj_from_bits(other_bits)) {
            Some(o) => o,
            None => return bool_bits(false),
        };
        bool_bits(s == other)
    })
}

/// `path.__lt__(other)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_lt(path_bits: u64, other_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let other = match require_str(_py, other_bits, "other") {
            Ok(o) => o,
            Err(bits) => return bits,
        };
        bool_bits(s < other)
    })
}

/// `path.as_posix()` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_as_posix(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        str_bits(_py, &s.replace('\\', "/"))
    })
}

// ---------------------------------------------------------------------------
// Concrete Path operations (require filesystem access)
// ---------------------------------------------------------------------------

/// `Path.cwd()` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_cwd() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        match std::env::current_dir() {
            Ok(p) => str_bits(_py, &p.to_string_lossy()),
            Err(err) => raise_os_error::<u64>(_py, err, "cwd"),
        }
    })
}

/// `Path.home()` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_home() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        #[cfg(unix)]
        let home = std::env::var("HOME").ok();
        #[cfg(windows)]
        let home = std::env::var("USERPROFILE").ok();
        #[cfg(not(any(unix, windows)))]
        let home: Option<String> = None;

        match home {
            Some(h) => str_bits(_py, &h),
            None => {
                raise_exception::<u64>(_py, "RuntimeError", "Could not determine home directory")
            }
        }
    })
}

/// `path.resolve()` -> str  (absolute, normalized path)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_resolve(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(_py, "pathlib.resolve", "fs.read", audit_path_arg(path_bits)) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = if s.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(&s)
        };
        match fs::canonicalize(&p) {
            Ok(abs) => str_bits(_py, &abs.to_string_lossy()),
            Err(_) => {
                // If the path doesn't exist, just make it absolute
                match std::env::current_dir() {
                    Ok(cwd) => str_bits(_py, &cwd.join(&p).to_string_lossy()),
                    Err(err) => raise_os_error::<u64>(_py, err, "resolve"),
                }
            }
        }
    })
}

/// `path.expanduser()` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_expanduser(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        if !s.starts_with('~') {
            return str_bits(_py, &s);
        }
        #[cfg(unix)]
        let home = std::env::var("HOME").ok();
        #[cfg(windows)]
        let home = std::env::var("USERPROFILE").ok();
        #[cfg(not(any(unix, windows)))]
        let home: Option<String> = None;

        match home {
            Some(h) => {
                if s == "~" {
                    str_bits(_py, &h)
                } else if s.starts_with("~/") || s.starts_with("~\\") {
                    str_bits(_py, &format!("{h}{}", &s[1..]))
                } else {
                    str_bits(_py, &s)
                }
            }
            None => {
                raise_exception::<u64>(_py, "RuntimeError", "Could not determine home directory")
            }
        }
    })
}

/// `path.is_mount()` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_is_mount(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(
            _py,
            "pathlib.is_mount",
            "fs.read",
            audit_path_arg(path_bits),
        ) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        if !p.is_dir() {
            return bool_bits(false);
        }
        // A mount point has a different device than its parent
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = match fs::metadata(p) {
                Ok(m) => m,
                Err(_) => return bool_bits(false),
            };
            let parent = match p.parent() {
                Some(pp) => pp,
                None => return bool_bits(true),
            };
            let parent_meta = match fs::metadata(parent) {
                Ok(m) => m,
                Err(_) => return bool_bits(false),
            };
            bool_bits(meta.dev() != parent_meta.dev() || meta.ino() == parent_meta.ino())
        }
        #[cfg(not(unix))]
        {
            bool_bits(false)
        }
    })
}

/// `path.iterdir()` -> list[str]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_iterdir(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(_py, "pathlib.iterdir", "fs.read", audit_path_arg(path_bits)) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let entries = match fs::read_dir(&s) {
            Ok(rd) => rd,
            Err(err) => return raise_os_error::<u64>(_py, err, "iterdir"),
        };
        let mut names = Vec::new();
        for entry in entries {
            match entry {
                Ok(e) => names.push(e.path().to_string_lossy().into_owned()),
                Err(err) => return raise_os_error::<u64>(_py, err, "iterdir"),
            }
        }
        list_of_strings(_py, &names)
    })
}

/// `path.rglob(pattern)` -> list[str]  (recursive glob)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_rglob(path_bits: u64, pattern_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(_py, "pathlib.rglob", "fs.read", audit_path_arg(path_bits)) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let pattern = match require_str(_py, pattern_bits, "pattern") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let full_pattern = if s == "." || s.is_empty() {
            format!("**/{pattern}")
        } else {
            format!("{s}/**/{pattern}")
        };
        #[cfg(feature = "stdlib_fs_extra")]
        {
            let mut results: Vec<String> = Vec::new();
            match glob::glob(&full_pattern) {
                Ok(paths) => {
                    for entry in paths {
                        match entry {
                            Ok(p) => results.push(p.to_string_lossy().into_owned()),
                            Err(_) => continue,
                        }
                    }
                }
                Err(err) => {
                    return raise_exception::<u64>(
                        _py,
                        "ValueError",
                        &format!("invalid glob pattern: {err}"),
                    );
                }
            }
            list_of_strings(_py, &results)
        }
        #[cfg(not(feature = "stdlib_fs_extra"))]
        {
            let _ = &full_pattern;
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "pathlib.rglob requires the stdlib_fs_extra feature",
            )
        }
    })
}

/// `path.touch(exist_ok=True)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_touch(path_bits: u64, exist_ok_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(_py, "pathlib.touch", "fs.write", audit_path_arg(path_bits)) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let exist_ok = is_truthy(_py, obj_from_bits(exist_ok_bits));
        let p = Path::new(&s);
        if p.exists() {
            if !exist_ok {
                return raise_exception::<u64>(
                    _py,
                    "FileExistsError",
                    &format!("File exists: '{s}'"),
                );
            }
            // Update mtime
            let _ = fs::OpenOptions::new().write(true).open(p);
            MoltObject::none().bits()
        } else {
            match fs::File::create(p) {
                Ok(_) => MoltObject::none().bits(),
                Err(err) => raise_os_error::<u64>(_py, err, "touch"),
            }
        }
    })
}

/// `path.hardlink_to(target)` -> None
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_hardlink_to(path_bits: u64, target_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(
            _py,
            "pathlib.hardlink_to",
            "fs.write",
            audit_path_arg(path_bits),
        ) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let target = match require_str(_py, target_bits, "target") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        match fs::hard_link(&target, &s) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "hardlink_to"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_hardlink_to(_path_bits: u64, _target_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "hardlink_to")
    })
}

/// `path.read_text(encoding=None)` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_read_text(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(
            _py,
            "pathlib.read_text",
            "fs.read",
            audit_path_arg(path_bits),
        ) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        match fs::read_to_string(&s) {
            Ok(text) => str_bits(_py, &text),
            Err(err) => raise_os_error::<u64>(_py, err, "read_text"),
        }
    })
}

/// `path.read_bytes()` -> bytes
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_read_bytes(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(
            _py,
            "pathlib.read_bytes",
            "fs.read",
            audit_path_arg(path_bits),
        ) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        match fs::read(&s) {
            Ok(data) => {
                let ptr = alloc_bytes(_py, &data);
                if ptr.is_null() {
                    raise_exception::<u64>(_py, "MemoryError", "out of memory")
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(err) => raise_os_error::<u64>(_py, err, "read_bytes"),
        }
    })
}

/// `path.write_text(data)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_write_text(path_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(
            _py,
            "pathlib.write_text",
            "fs.write",
            audit_path_arg(path_bits),
        ) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let data = match require_str(_py, data_bits, "data") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        match fs::write(&s, &data) {
            Ok(()) => MoltObject::from_int(data.len() as i64).bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "write_text"),
        }
    })
}

/// `path.write_bytes(data)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_write_bytes(path_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(
            _py,
            "pathlib.write_bytes",
            "fs.write",
            audit_path_arg(path_bits),
        ) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let data = match obj_from_bits(data_bits).as_ptr() {
            Some(ptr) => match unsafe { bytes_like_slice(ptr) } {
                Some(sl) => sl.to_vec(),
                None => {
                    return raise_exception::<u64>(_py, "TypeError", "data must be bytes-like");
                }
            },
            None => {
                return raise_exception::<u64>(_py, "TypeError", "data must be bytes-like");
            }
        };
        match fs::write(&s, &data) {
            Ok(()) => MoltObject::from_int(data.len() as i64).bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "write_bytes"),
        }
    })
}

/// `path.owner()` -> str (Unix only)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_owner(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(_py, "pathlib.owner", "fs.read", audit_path_arg(path_bits)) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = match fs::metadata(&s) {
                Ok(m) => m,
                Err(err) => return raise_os_error::<u64>(_py, err, "owner"),
            };
            let uid = meta.uid();
            let pw = unsafe { libc::getpwuid(uid) };
            if pw.is_null() {
                return raise_exception::<u64>(_py, "KeyError", &format!("no user with uid {uid}"));
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*pw).pw_name) };
            str_bits(_py, &name.to_string_lossy())
        }
        #[cfg(not(unix))]
        {
            let _ = s;
            raise_exception::<u64>(
                _py,
                "NotImplementedError",
                "owner() not available on this platform",
            )
        }
    })
}

/// `path.group()` -> str (Unix only)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_group(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(_py, "pathlib.group", "fs.read", audit_path_arg(path_bits)) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = match fs::metadata(&s) {
                Ok(m) => m,
                Err(err) => return raise_os_error::<u64>(_py, err, "group"),
            };
            let gid = meta.gid();
            let gr = unsafe { libc::getgrgid(gid) };
            if gr.is_null() {
                return raise_exception::<u64>(
                    _py,
                    "KeyError",
                    &format!("no group with gid {gid}"),
                );
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*gr).gr_name) };
            str_bits(_py, &name.to_string_lossy())
        }
        #[cfg(not(unix))]
        {
            let _ = s;
            raise_exception::<u64>(
                _py,
                "NotImplementedError",
                "group() not available on this platform",
            )
        }
    })
}

/// `path.samefile(other_path)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_samefile(path_bits: u64, other_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !audit_capability(
            _py,
            "pathlib.samefile",
            "fs.read",
            audit_path_arg(path_bits),
        ) {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let other = match require_str(_py, other_bits, "other") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let m1 = match fs::metadata(&s) {
                Ok(m) => m,
                Err(err) => return raise_os_error::<u64>(_py, err, "samefile"),
            };
            let m2 = match fs::metadata(&other) {
                Ok(m) => m,
                Err(err) => return raise_os_error::<u64>(_py, err, "samefile"),
            };
            bool_bits(m1.dev() == m2.dev() && m1.ino() == m2.ino())
        }
        #[cfg(not(unix))]
        {
            // Fall back to canonical path comparison
            let c1 = fs::canonicalize(&s);
            let c2 = fs::canonicalize(&other);
            match (c1, c2) {
                (Ok(a), Ok(b)) => bool_bits(a == b),
                (Err(err), _) | (_, Err(err)) => raise_os_error::<u64>(_py, err, "samefile"),
            }
        }
    })
}
