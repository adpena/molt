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

use crate::*;
use crate::audit::{AuditArgs, audit_capability_decision};
use std::fs;
use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[inline]
fn str_bits(py: &PyToken<'_>, s: &str) -> u64 {
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

fn require_str(py: &PyToken<'_>, bits: u64, label: &str) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(s) => Ok(s),
        None => Err(raise_exception::<u64>(
            py,
            "TypeError",
            &format!("{label} must be str"),
        )),
    }
}

fn list_of_strings(py: &PyToken<'_>, items: &[String]) -> u64 {
    let bits: Vec<u64> = items.iter().map(|s| str_bits(py, s)).collect();
    let ptr = alloc_list(py, &bits);
    if ptr.is_null() {
        raise_exception::<u64>(py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn tuple_of_strings(py: &PyToken<'_>, items: &[String]) -> u64 {
    let bits: Vec<u64> = items.iter().map(|s| str_bits(py, s)).collect();
    let ptr = alloc_tuple(py, &bits);
    if ptr.is_null() {
        raise_exception::<u64>(py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// Determine if a path string uses Windows separators.
fn is_windows_flavor(s: &str) -> bool {
    // Check for drive letter (C:) or UNC (\\server)
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return true;
        }
        if bytes[0] == b'\\' && bytes[1] == b'\\' {
            return true;
        }
    }
    s.contains('\\')
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

/// `pathlib._molt_path_join(base, *args)` -> str
/// Joins path segments using POSIX rules.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_join(base_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let base = match require_str(_py, base_bits, "base") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let mut result = PathBuf::from(&base);
        if let Some(ptr) = obj_from_bits(args_bits).as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                let elems = unsafe { seq_vec_ref(ptr) };
                for &elem_bits in elems {
                    if let Some(s) = string_obj_to_owned(obj_from_bits(elem_bits)) {
                        let p = Path::new(&s);
                        if p.is_absolute() {
                            result = p.to_path_buf();
                        } else {
                            result.push(p);
                        }
                    }
                }
            }
        }
        str_bits(_py, &result.to_string_lossy())
    })
}

/// `pathlib._molt_path_str(path)` -> str  (normalized)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_str(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

/// `path.drive` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_drive(path_bits: u64, posix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let (drive, _, _) = splitroot(&s, posix);
        str_bits(_py, &drive)
    })
}

/// `path.root` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_root(path_bits: u64, posix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let (_, root, _) = splitroot(&s, posix);
        str_bits(_py, &root)
    })
}

/// `path.anchor` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_anchor(path_bits: u64, posix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let (drive, root, _) = splitroot(&s, posix);
        str_bits(_py, &format!("{drive}{root}"))
    })
}

/// `path.name` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_name(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        str_bits(_py, &name)
    })
}

/// `path.suffix` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_suffix(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        let suffix = p
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        str_bits(_py, &suffix)
    })
}

/// `path.suffixes` property -> list[str]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_suffixes(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        let name = match p.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => return list_of_strings(_py, &[]),
        };
        let mut suffixes = Vec::new();
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() > 1 {
            for part in &parts[1..] {
                suffixes.push(format!(".{part}"));
            }
        }
        list_of_strings(_py, &suffixes)
    })
}

/// `path.stem` property
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_stem(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        str_bits(_py, &stem)
    })
}

/// `path.parent` property -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_parent(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        let parent = p
            .parent()
            .map(|pp| pp.to_string_lossy().into_owned())
            .unwrap_or_else(|| s.clone());
        if parent.is_empty() {
            str_bits(_py, ".")
        } else {
            str_bits(_py, &parent)
        }
    })
}

/// `path.parents` -> list[str]  (all ancestor paths)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_parents(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        let mut parents = Vec::new();
        let mut cur = p.parent();
        while let Some(pp) = cur {
            let ps = pp.to_string_lossy().into_owned();
            if ps.is_empty() {
                parents.push(".".to_string());
            } else {
                parents.push(ps);
            }
            cur = pp.parent();
        }
        list_of_strings(_py, &parents)
    })
}

/// `path.is_absolute()` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_is_absolute(path_bits: u64, posix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        if posix {
            bool_bits(s.starts_with('/'))
        } else {
            let (drive, root, _) = splitroot(&s, false);
            bool_bits(!root.is_empty() && !drive.is_empty())
        }
    })
}

/// `path.is_relative_to(other)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_is_relative_to(path_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let other = match require_str(_py, other_bits, "other") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        let o = Path::new(&other);
        bool_bits(p.starts_with(o))
    })
}

/// `path.relative_to(other)` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_relative_to(path_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let other = match require_str(_py, other_bits, "other") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        let o = Path::new(&other);
        match p.strip_prefix(o) {
            Ok(rel) => str_bits(_py, &rel.to_string_lossy()),
            Err(_) => raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("'{s}' is not relative to '{other}'"),
            ),
        }
    })
}

/// `path.with_name(name)` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_with_name(path_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let name = match require_str(_py, name_bits, "name") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        if p.file_name().is_none() {
            return raise_exception::<u64>(_py, "ValueError", &format!("{s} has an empty name"));
        }
        let result = p.with_file_name(&name);
        str_bits(_py, &result.to_string_lossy())
    })
}

/// `path.with_stem(stem)` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_with_stem(path_bits: u64, stem_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let stem = match require_str(_py, stem_bits, "stem") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        if p.file_name().is_none() {
            return raise_exception::<u64>(_py, "ValueError", &format!("{s} has an empty name"));
        }
        let ext = p
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let new_name = format!("{stem}{ext}");
        let result = p.with_file_name(&new_name);
        str_bits(_py, &result.to_string_lossy())
    })
}

/// `path.with_suffix(suffix)` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_with_suffix(path_bits: u64, suffix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let suffix = match require_str(_py, suffix_bits, "suffix") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = Path::new(&s);
        if p.file_name().is_none() {
            return raise_exception::<u64>(_py, "ValueError", &format!("{s} has an empty name"));
        }
        if !suffix.is_empty() && !suffix.starts_with('.') {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("Invalid suffix '{suffix}'"),
            );
        }
        let result = if suffix.is_empty() {
            // Remove extension
            let stem = p.file_stem().unwrap_or_default();
            p.with_file_name(stem)
        } else {
            p.with_extension(&suffix[1..])
        };
        str_bits(_py, &result.to_string_lossy())
    })
}

/// `path.match(pattern)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_match(path_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let pattern = match require_str(_py, pattern_bits, "pattern") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        // Use glob::Pattern for matching
        #[cfg(feature = "stdlib_fs_extra")]
        let matched = match glob::Pattern::new(&pattern) {
            Ok(pat) => {
                let p = Path::new(&s);
                // If pattern has no separator, match only the name
                if !pattern.contains('/') && !pattern.contains('\\') {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    pat.matches(&name)
                } else {
                    pat.matches(&s)
                }
            }
            Err(_) => false,
        };
        #[cfg(not(feature = "stdlib_fs_extra"))]
        let matched = {
            let _ = (&s, &pattern);
            false
        };
        bool_bits(matched)
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        match std::env::current_dir() {
            Ok(p) => str_bits(_py, &p.to_string_lossy()),
            Err(err) => raise_os_error::<u64>(_py, err, "cwd"),
        }
    })
}

/// `Path.home()` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_home() -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.resolve", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry!(_py, {
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

/// `path.exists()` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_exists(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.exists", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        bool_bits(Path::new(&s).exists())
    })
}

/// `path.is_file()` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_is_file(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.is_file", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        bool_bits(Path::new(&s).is_file())
    })
}

/// `path.is_dir()` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_is_dir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.is_dir", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        bool_bits(Path::new(&s).is_dir())
    })
}

/// `path.is_symlink()` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_is_symlink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.is_symlink", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        bool_bits(Path::new(&s).is_symlink())
    })
}

/// `path.is_mount()` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_is_mount(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.is_mount", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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

/// `path.stat()` -> tuple  (mode, ino, dev, nlink, uid, gid, size, atime, mtime, ctime)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_stat(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.stat", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let meta = match fs::metadata(&s) {
            Ok(m) => m,
            Err(err) => return raise_os_error::<u64>(_py, err, "stat"),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let elems = [
                MoltObject::from_int(meta.mode() as i64).bits(),
                MoltObject::from_int(meta.ino() as i64).bits(),
                MoltObject::from_int(meta.dev() as i64).bits(),
                MoltObject::from_int(meta.nlink() as i64).bits(),
                MoltObject::from_int(meta.uid() as i64).bits(),
                MoltObject::from_int(meta.gid() as i64).bits(),
                MoltObject::from_int(meta.size() as i64).bits(),
                MoltObject::from_int(meta.atime()).bits(),
                MoltObject::from_int(meta.mtime()).bits(),
                MoltObject::from_int(meta.ctime()).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
        #[cfg(not(unix))]
        {
            let len = meta.len() as i64;
            let elems = [
                MoltObject::from_int(0).bits(),   // mode
                MoltObject::from_int(0).bits(),   // ino
                MoltObject::from_int(0).bits(),   // dev
                MoltObject::from_int(1).bits(),   // nlink
                MoltObject::from_int(0).bits(),   // uid
                MoltObject::from_int(0).bits(),   // gid
                MoltObject::from_int(len).bits(), // size
                MoltObject::from_int(0).bits(),   // atime
                MoltObject::from_int(0).bits(),   // mtime
                MoltObject::from_int(0).bits(),   // ctime
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// `path.lstat()` -> tuple (like stat but doesn't follow symlinks)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_lstat(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.lstat", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let meta = match fs::symlink_metadata(&s) {
            Ok(m) => m,
            Err(err) => return raise_os_error::<u64>(_py, err, "lstat"),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let elems = [
                MoltObject::from_int(meta.mode() as i64).bits(),
                MoltObject::from_int(meta.ino() as i64).bits(),
                MoltObject::from_int(meta.dev() as i64).bits(),
                MoltObject::from_int(meta.nlink() as i64).bits(),
                MoltObject::from_int(meta.uid() as i64).bits(),
                MoltObject::from_int(meta.gid() as i64).bits(),
                MoltObject::from_int(meta.size() as i64).bits(),
                MoltObject::from_int(meta.atime()).bits(),
                MoltObject::from_int(meta.mtime()).bits(),
                MoltObject::from_int(meta.ctime()).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
        #[cfg(not(unix))]
        {
            let len = meta.len() as i64;
            let elems = [
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(len).bits(),
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(0).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// `path.iterdir()` -> list[str]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_iterdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.iterdir", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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

/// `path.glob(pattern)` -> list[str]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_glob(path_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.glob", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
            pattern.clone()
        } else {
            format!("{s}/{pattern}")
        };
        let mut results: Vec<String> = Vec::new();
        #[cfg(feature = "stdlib_fs_extra")]
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
        #[cfg(not(feature = "stdlib_fs_extra"))]
        {
            let _ = &full_pattern;
        }
        list_of_strings(_py, &results)
    })
}

/// `path.rglob(pattern)` -> list[str]  (recursive glob)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_rglob(path_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.rglob", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
        let mut results: Vec<String> = Vec::new();
        #[cfg(feature = "stdlib_fs_extra")]
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
        #[cfg(not(feature = "stdlib_fs_extra"))]
        {
            let _ = &full_pattern;
        }
        list_of_strings(_py, &results)
    })
}

/// `path.mkdir(mode=0o777, parents=False, exist_ok=False)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_mkdir(path_bits: u64, parents_bits: u64, exist_ok_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.mkdir", "fs.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let parents = is_truthy(_py, obj_from_bits(parents_bits));
        let exist_ok = is_truthy(_py, obj_from_bits(exist_ok_bits));
        let p = Path::new(&s);
        let result = if parents {
            fs::create_dir_all(p)
        } else {
            fs::create_dir(p)
        };
        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(err) if exist_ok && err.kind() == std::io::ErrorKind::AlreadyExists => {
                MoltObject::none().bits()
            }
            Err(err) => raise_os_error::<u64>(_py, err, "mkdir"),
        }
    })
}

/// `path.rmdir()` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_rmdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.rmdir", "fs.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        match fs::remove_dir(&s) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "rmdir"),
        }
    })
}

/// `path.unlink(missing_ok=False)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_unlink(path_bits: u64, missing_ok_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.unlink", "fs.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let missing_ok = is_truthy(_py, obj_from_bits(missing_ok_bits));
        match fs::remove_file(&s) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) if missing_ok && err.kind() == std::io::ErrorKind::NotFound => {
                MoltObject::none().bits()
            }
            Err(err) => raise_os_error::<u64>(_py, err, "unlink"),
        }
    })
}

/// `path.rename(target)` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_rename(path_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.rename", "fs.write", AuditArgs::None, allowed);
        if !allowed {
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
        match fs::rename(&s, &target) {
            Ok(()) => str_bits(_py, &target),
            Err(err) => raise_os_error::<u64>(_py, err, "rename"),
        }
    })
}

/// `path.replace(target)` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_replace(path_bits: u64, target_bits: u64) -> u64 {
    // replace is identical to rename on Unix; on Windows it's more permissive
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.replace", "fs.write", AuditArgs::None, allowed);
        if !allowed {
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
        match fs::rename(&s, &target) {
            Ok(()) => str_bits(_py, &target),
            Err(err) => raise_os_error::<u64>(_py, err, "replace"),
        }
    })
}

/// `path.touch(exist_ok=True)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_touch(path_bits: u64, exist_ok_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.touch", "fs.write", AuditArgs::None, allowed);
        if !allowed {
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

/// `path.symlink_to(target)` -> None
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_symlink_to(path_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.symlink_to", "fs.write", AuditArgs::None, allowed);
        if !allowed {
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
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(&target, &s);
        #[cfg(windows)]
        let result = {
            if Path::new(&target).is_dir() {
                std::os::windows::fs::symlink_dir(&target, &s)
            } else {
                std::os::windows::fs::symlink_file(&target, &s)
            }
        };
        #[cfg(not(any(unix, windows)))]
        let result: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks not supported",
        ));
        match result {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "symlink_to"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_symlink_to(_path_bits: u64, _target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "symlink_to")
    })
}

/// `path.hardlink_to(target)` -> None
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_hardlink_to(path_bits: u64, target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.hardlink_to", "fs.write", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "hardlink_to")
    })
}

/// `path.readlink()` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_readlink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.readlink", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.read capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        match fs::read_link(&s) {
            Ok(target) => str_bits(_py, &target.to_string_lossy()),
            Err(err) => raise_os_error::<u64>(_py, err, "readlink"),
        }
    })
}

/// `path.read_text(encoding=None)` -> str
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_read_text(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.read_text", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.read_bytes", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.write_text", "fs.write", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.write_bytes", "fs.write", AuditArgs::None, allowed);
        if !allowed {
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

/// `path.chmod(mode)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_chmod(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.write");
        audit_capability_decision("pathlib.chmod", "fs.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing fs.write capability");
        }
        let s = match require_str(_py, path_bits, "path") {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let mode = match to_i64(obj_from_bits(mode_bits)) {
            Some(m) => m,
            None => return raise_exception::<u64>(_py, "TypeError", "mode must be int"),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(mode as u32);
            match fs::set_permissions(&s, perms) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => raise_os_error::<u64>(_py, err, "chmod"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (s, mode);
            MoltObject::none().bits()
        }
    })
}

/// `path.owner()` -> str (Unix only)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_owner(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.owner", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.group", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("pathlib.samefile", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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

/// Return the OS path separator
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_sep() -> u64 {
    crate::with_gil_entry!(_py, { str_bits(_py, std::path::MAIN_SEPARATOR_STR) })
}
