// === FILE: runtime/molt-runtime/src/builtins/os_ext.rs ===
//
// Additional os intrinsics: directory ops, file metadata, process info, and
// os.path helpers that are not yet covered in io.rs or platform.rs.
//
// Capability mapping:
//   fs.read  – directory listing, stat, readlink, getcwd, access, getsize, mtime, …
//   fs.write – chdir, mkdir, rmdir, removedirs, chmod, link, symlink, truncate
//   env.read – getlogin
//   (process info intrinsics need no capability gate – they are always available
//    in a running process and contain no sensitive filesystem state)
//
// WASM notes: most operations that require real POSIX semantics are guarded with
// #[cfg(not(target_arch = "wasm32"))].  On WASM we return ENOSYS equivalents so
// the Python wrapper can raise NotImplementedError.

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

use crate::*;
use std::path::{Component, Path, PathBuf};

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Map a std::io::Error to the appropriate Python exception bits.
#[inline]
fn os_err_bits(_py: &PyToken<'_>, err: std::io::Error, ctx: &str) -> u64 {
    raise_os_error::<u64>(_py, err, ctx)
}

/// Borrow a `str` from a runtime string object, raising TypeError on failure.
#[inline]
#[allow(dead_code)]
fn require_str(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(s) => Ok(s),
        None => Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be str"),
        )),
    }
}

/// Resolve a path argument (str | bytes | os.PathLike) to a PathBuf.
#[inline]
fn require_path(_py: &PyToken<'_>, bits: u64, _label: &str) -> Result<PathBuf, u64> {
    match path_from_bits(_py, bits) {
        Ok(p) => Ok(p),
        Err(msg) => Err(raise_exception::<u64>(_py, "TypeError", &msg)),
    }
}

/// Allocate a runtime string from a Rust &str slice, returning bits.
/// Returns None bits on allocation failure rather than propagating OOM (consistent
/// with existing patterns in io.rs).
#[inline]
fn str_bits(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// 1. Directory operations
// ---------------------------------------------------------------------------

/// `os.listdir(path)` → list[str]
/// Already implemented as `molt_path_listdir`; this variant exposes it under
/// the canonical `molt_os_listdir` name so os.py can use either.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_listdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let rd = match std::fs::read_dir(&path) {
            Ok(rd) => rd,
            Err(err) => return os_err_bits(_py, err, "listdir"),
        };
        let mut entries: Vec<u64> = Vec::new();
        for entry_result in rd {
            match entry_result {
                Ok(entry) => {
                    let name = entry.file_name();
                    let text = name.to_string_lossy();
                    let ptr = alloc_string(_py, text.as_bytes());
                    if ptr.is_null() {
                        for e in &entries {
                            dec_ref_bits(_py, *e);
                        }
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    entries.push(MoltObject::from_ptr(ptr).bits());
                }
                Err(err) => {
                    for e in &entries {
                        dec_ref_bits(_py, *e);
                    }
                    return os_err_bits(_py, err, "listdir");
                }
            }
        }
        let list_ptr = alloc_list(_py, &entries);
        for e in &entries {
            dec_ref_bits(_py, *e);
        }
        if list_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

/// `os.scandir(path)` → list[tuple[str, str, bool, bool, bool]]
/// Each tuple: (name, path, is_dir, is_file, is_symlink)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_scandir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let dir_path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let rd = match std::fs::read_dir(&dir_path) {
            Ok(rd) => rd,
            Err(err) => return os_err_bits(_py, err, "scandir"),
        };
        let mut tuples: Vec<u64> = Vec::new();
        for entry_result in rd {
            match entry_result {
                Ok(entry) => {
                    let name_os = entry.file_name();
                    let name_str = name_os.to_string_lossy();
                    let full_path = entry.path();
                    let full_str = full_path.to_string_lossy();

                    let meta_sym = std::fs::symlink_metadata(entry.path());
                    let is_symlink = meta_sym
                        .as_ref()
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false);
                    let meta = std::fs::metadata(entry.path());
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let is_file = meta.as_ref().map(|m| m.is_file()).unwrap_or(false);

                    let name_ptr = alloc_string(_py, name_str.as_bytes());
                    let path_ptr = alloc_string(_py, full_str.as_bytes());
                    if name_ptr.is_null() || path_ptr.is_null() {
                        if !name_ptr.is_null() {
                            dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
                        }
                        if !path_ptr.is_null() {
                            dec_ref_bits(_py, MoltObject::from_ptr(path_ptr).bits());
                        }
                        for t in &tuples {
                            dec_ref_bits(_py, *t);
                        }
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    let elems = [
                        MoltObject::from_ptr(name_ptr).bits(),
                        MoltObject::from_ptr(path_ptr).bits(),
                        MoltObject::from_bool(is_dir).bits(),
                        MoltObject::from_bool(is_file).bits(),
                        MoltObject::from_bool(is_symlink).bits(),
                    ];
                    let tup_ptr = alloc_tuple(_py, &elems);
                    dec_ref_bits(_py, elems[0]);
                    dec_ref_bits(_py, elems[1]);
                    if tup_ptr.is_null() {
                        for t in &tuples {
                            dec_ref_bits(_py, *t);
                        }
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    tuples.push(MoltObject::from_ptr(tup_ptr).bits());
                }
                Err(err) => {
                    for t in &tuples {
                        dec_ref_bits(_py, *t);
                    }
                    return os_err_bits(_py, err, "scandir");
                }
            }
        }
        let list_ptr = alloc_list(_py, &tuples);
        for t in &tuples {
            dec_ref_bits(_py, *t);
        }
        if list_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

/// `os.walk(top, topdown=True, followlinks=False)` → list[tuple[str, list[str], list[str]]]
///
/// Each element: (dirpath, dirnames, filenames).
/// We collect the full walk eagerly (no generator semantics in the intrinsic layer).
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_walk(top_bits: u64, topdown_bits: u64, followlinks_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let top = match require_path(_py, top_bits, "top") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let topdown = is_truthy(_py, obj_from_bits(topdown_bits));
        let followlinks = is_truthy(_py, obj_from_bits(followlinks_bits));

        let mut results: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
        walk_dir_collect(&top, topdown, followlinks, &mut results);

        let mut triplets: Vec<u64> = Vec::with_capacity(results.len());
        for (dirpath, dirnames, filenames) in &results {
            let dp_ptr = alloc_string(_py, dirpath.as_bytes());
            if dp_ptr.is_null() {
                for t in &triplets {
                    dec_ref_bits(_py, *t);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let dn_bits: Vec<u64> = dirnames
                .iter()
                .filter_map(|s| {
                    let p = alloc_string(_py, s.as_bytes());
                    if p.is_null() {
                        None
                    } else {
                        Some(MoltObject::from_ptr(p).bits())
                    }
                })
                .collect();
            let fn_bits: Vec<u64> = filenames
                .iter()
                .filter_map(|s| {
                    let p = alloc_string(_py, s.as_bytes());
                    if p.is_null() {
                        None
                    } else {
                        Some(MoltObject::from_ptr(p).bits())
                    }
                })
                .collect();
            let dn_list = alloc_list(_py, &dn_bits);
            let fn_list = alloc_list(_py, &fn_bits);
            for b in &dn_bits {
                dec_ref_bits(_py, *b);
            }
            for b in &fn_bits {
                dec_ref_bits(_py, *b);
            }
            if dn_list.is_null() || fn_list.is_null() {
                if !dn_list.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(dn_list).bits());
                }
                if !fn_list.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(fn_list).bits());
                }
                dec_ref_bits(_py, MoltObject::from_ptr(dp_ptr).bits());
                for t in &triplets {
                    dec_ref_bits(_py, *t);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let elems = [
                MoltObject::from_ptr(dp_ptr).bits(),
                MoltObject::from_ptr(dn_list).bits(),
                MoltObject::from_ptr(fn_list).bits(),
            ];
            let tup_ptr = alloc_tuple(_py, &elems);
            for e in &elems {
                dec_ref_bits(_py, *e);
            }
            if tup_ptr.is_null() {
                for t in &triplets {
                    dec_ref_bits(_py, *t);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            triplets.push(MoltObject::from_ptr(tup_ptr).bits());
        }
        let list_ptr = alloc_list(_py, &triplets);
        for t in &triplets {
            dec_ref_bits(_py, *t);
        }
        if list_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

/// Recursive walk helper (pure Rust, no GIL calls).
fn walk_dir_collect(
    dir: &Path,
    topdown: bool,
    followlinks: bool,
    out: &mut Vec<(String, Vec<String>, Vec<String>)>,
) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    let mut dirnames: Vec<String> = Vec::new();
    let mut filenames: Vec<String> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry_result in rd {
        let Ok(entry) = entry_result else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let meta = if followlinks {
            std::fs::metadata(&path)
        } else {
            std::fs::symlink_metadata(&path)
        };
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            dirnames.push(name);
            subdirs.push(path);
        } else {
            filenames.push(name.clone());
        }
    }
    let dirpath = dir.to_string_lossy().into_owned();
    if topdown {
        out.push((dirpath, dirnames.clone(), filenames));
        for sub in &subdirs {
            walk_dir_collect(sub, topdown, followlinks, out);
        }
    } else {
        for sub in &subdirs {
            walk_dir_collect(sub, topdown, followlinks, out);
        }
        out.push((dirpath, dirnames, filenames));
    }
}

/// `os.getcwd()` → str
/// Note: molt_getcwd already exists in platform.rs under that name.
/// This provides the canonical os-namespaced variant.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getcwd() -> u64 {
    crate::with_gil_entry!(_py, {
        match std::env::current_dir() {
            Ok(path) => {
                let text = path.to_string_lossy();
                str_bits(_py, &text)
            }
            Err(err) => os_err_bits(_py, err, "getcwd"),
        }
    })
}

/// `os.chdir(path)` → None
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_chdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::env::set_current_dir(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => os_err_bits(_py, err, "chdir"),
        }
    })
}

/// `os.mkdir(path, mode=0o777)` → None  (single directory only)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_mkdir(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let mode = to_i64(obj_from_bits(mode_bits)).unwrap_or(0o777);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            match std::fs::DirBuilder::new().mode(mode as u32).create(&path) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => os_err_bits(_py, err, "mkdir"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
            match std::fs::create_dir(&path) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => os_err_bits(_py, err, "mkdir"),
            }
        }
    })
}

/// `os.rmdir(path)` → None  (remove empty directory)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_rmdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::fs::remove_dir(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => os_err_bits(_py, err, "rmdir"),
        }
    })
}

/// `os.removedirs(path)` → None  (remove leaf then successive empty parents)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_removedirs(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        if let Err(err) = std::fs::remove_dir(&path) {
            return os_err_bits(_py, err, "removedirs");
        }
        let mut cur = path.as_path();
        while let Some(parent) = cur.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            if std::fs::remove_dir(parent).is_err() {
                break;
            }
            cur = parent;
        }
        MoltObject::none().bits()
    })
}

// ---------------------------------------------------------------------------
// 2. File operations
// ---------------------------------------------------------------------------

/// `os.access(path, mode)` → bool
/// mode: F_OK=0, X_OK=1, W_OK=2, R_OK=4  (POSIX standard values)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_access(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let mode = to_i64(obj_from_bits(mode_bits)).unwrap_or(0) as u32;
        // F_OK: existence
        let meta = std::fs::metadata(&path);
        if meta.is_err() {
            return MoltObject::from_bool(false).bits();
        }
        let meta = meta.unwrap();
        // On WASM or Windows we approximate using std metadata.
        #[cfg(unix)]
        {
            // X_OK=1, W_OK=2, R_OK=4
            let perms = meta.permissions();
            use std::os::unix::fs::PermissionsExt;
            let m = perms.mode();
            // Check against effective user — we use a simplified owner check.
            // For production correctness this calls libc::access.
            let path_c = match std::ffi::CString::new(path.to_string_lossy().as_bytes()) {
                Ok(c) => c,
                Err(_) => return MoltObject::from_bool(false).bits(),
            };
            let rc = unsafe { libc::access(path_c.as_ptr(), mode as libc::c_int) };
            let _ = m;
            MoltObject::from_bool(rc == 0).bits()
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            // Windows: approximate - F_OK already succeeded via metadata.
            if mode == 0 {
                return MoltObject::from_bool(true).bits();
            }
            // W_OK: check read-only flag
            if mode & 2 != 0 && meta.permissions().readonly() {
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(true).bits()
        }
    })
}

/// `os.chmod(path, mode)` → None
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_chmod(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let mode = to_i64(obj_from_bits(mode_bits)).unwrap_or(0o644) as u32;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            match std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => os_err_bits(_py, err, "chmod"),
            }
        }
        #[cfg(not(unix))]
        {
            // On Windows: toggle read-only via a coarse approximation.
            let readonly = (mode & 0o200) == 0;
            match std::fs::metadata(&path) {
                Ok(meta) => {
                    let mut perms = meta.permissions();
                    perms.set_readonly(readonly);
                    match std::fs::set_permissions(&path, perms) {
                        Ok(()) => MoltObject::none().bits(),
                        Err(err) => os_err_bits(_py, err, "chmod"),
                    }
                }
                Err(err) => os_err_bits(_py, err, "chmod"),
            }
        }
    })
}

/// `os.link(src, dst)` → None  (hard link)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_link(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let src = match require_path(_py, src_bits, "src") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let dst = match require_path(_py, dst_bits, "dst") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::fs::hard_link(&src, &dst) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => os_err_bits(_py, err, "link"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_link(_src_bits: u64, _dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "link")
    })
}

/// `os.symlink(src, dst)` → None
/// Thin delegation to the existing molt_path_symlink behaviour.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_symlink(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let src = match require_path(_py, src_bits, "src") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let dst = match require_path(_py, dst_bits, "dst") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            match symlink(&src, &dst) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => os_err_bits(_py, err, "symlink"),
            }
        }
        #[cfg(windows)]
        {
            // On Windows the target type matters for the API call.
            let is_dir = src.is_dir();
            let result = if is_dir {
                std::os::windows::fs::symlink_dir(&src, &dst)
            } else {
                std::os::windows::fs::symlink_file(&src, &dst)
            };
            match result {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => os_err_bits(_py, err, "symlink"),
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "symlink")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_symlink(_src_bits: u64, _dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "symlink")
    })
}

/// `os.readlink(path)` → str
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_readlink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::fs::read_link(&path) {
            Ok(target) => str_bits(_py, &target.to_string_lossy()),
            Err(err) => os_err_bits(_py, err, "readlink"),
        }
    })
}

/// `os.truncate(path, length)` → None
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_truncate(path_bits: u64, length_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let length = match to_i64(obj_from_bits(length_bits)) {
            Some(n) if n >= 0 => n as u64,
            _ => {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "length must be a non-negative integer",
                );
            }
        };
        match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(f) => match f.set_len(length) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => os_err_bits(_py, err, "truncate"),
            },
            Err(err) => os_err_bits(_py, err, "truncate"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_truncate(_path_bits: u64, _length_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "truncate")
    })
}

// ---------------------------------------------------------------------------
// 3. Process information
// ---------------------------------------------------------------------------

/// `os.getpid()` → int  (already exists as molt_getpid in platform.rs; alias here)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getpid() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            MoltObject::from_int(unsafe { libc::getpid() } as i64).bits()
        }
        #[cfg(not(unix))]
        {
            MoltObject::from_int(std::process::id() as i64).bits()
        }
    })
}

/// `os.getppid()` → int
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getppid() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            MoltObject::from_int(unsafe { libc::getppid() } as i64).bits()
        }
        #[cfg(not(unix))]
        {
            // Windows has no direct ppid; return 0 as CPython does on unsupported platforms.
            MoltObject::from_int(0).bits()
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getppid() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "OSError",
            "[Errno 38] Function not implemented: 'getppid'",
        )
    })
}

/// `os.getuid()` → int  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getuid() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            MoltObject::from_int(unsafe { libc::getuid() } as i64).bits()
        }
        #[cfg(not(unix))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getuid")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getuid() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getuid")
    })
}

/// `os.getgid()` → int  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getgid() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            MoltObject::from_int(unsafe { libc::getgid() } as i64).bits()
        }
        #[cfg(not(unix))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getgid")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getgid() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getgid")
    })
}

/// `os.geteuid()` → int  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_geteuid() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            MoltObject::from_int(unsafe { libc::geteuid() } as i64).bits()
        }
        #[cfg(not(unix))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "geteuid")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_geteuid() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "geteuid")
    })
}

/// `os.getegid()` → int  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getegid() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            MoltObject::from_int(unsafe { libc::getegid() } as i64).bits()
        }
        #[cfg(not(unix))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getegid")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getegid() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getegid")
    })
}

/// `os.getlogin()` → str
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getlogin() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            // getlogin() returns a pointer to a static buffer; not thread-safe
            // but consistent with CPython's os.getlogin() behaviour.
            let ptr = unsafe { libc::getlogin() };
            if !ptr.is_null() {
                let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };
                let s = cstr.to_string_lossy();
                if !s.is_empty() {
                    return str_bits(_py, &s);
                }
            }
            // Fall back to environment variable.
            for var in &["LOGNAME", "USER"] {
                if let Ok(val) = std::env::var(var)
                    && !val.is_empty()
                {
                    return str_bits(_py, &val);
                }
            }
            raise_os_error_errno::<u64>(_py, libc::ENOENT as i64, "getlogin")
        }
        #[cfg(windows)]
        {
            if let Ok(val) = std::env::var("USERNAME") {
                if !val.is_empty() {
                    return str_bits(_py, &val);
                }
            }
            raise_exception::<u64>(_py, "OSError", "getlogin failed")
        }
        #[cfg(not(any(unix, windows)))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getlogin")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getlogin() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getlogin")
    })
}

/// `os.cpu_count()` → int | None
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_cpu_count() -> u64 {
    crate::with_gil_entry!(_py, {
        let n = num_cpus::get();
        MoltObject::from_int(n as i64).bits()
    })
}

/// `os.get_terminal_size(fd=1)` → tuple[int, int]  (columns, lines)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_get_terminal_size(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let fd = to_i64(obj_from_bits(fd_bits)).unwrap_or(1) as libc::c_int;
        #[cfg(unix)]
        {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
            if rc < 0 || ws.ws_col == 0 || ws.ws_row == 0 {
                return raise_exception::<u64>(_py, "OSError", "could not get terminal size");
            }
            let elems = [
                MoltObject::from_int(ws.ws_col as i64).bits(),
                MoltObject::from_int(ws.ws_row as i64).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
        #[cfg(not(unix))]
        {
            let _ = fd;
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_terminal_size")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_get_terminal_size(_fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_terminal_size")
    })
}

/// `os.getloadavg()` → tuple[float, float, float]  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getloadavg() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            let mut avgs: [f64; 3] = [0.0; 3];
            let rc = unsafe { libc::getloadavg(avgs.as_mut_ptr(), 3) };
            if rc < 0 {
                return raise_exception::<u64>(_py, "OSError", "getloadavg failed");
            }
            let elems = [
                MoltObject::from_float(avgs[0]).bits(),
                MoltObject::from_float(avgs[1]).bits(),
                MoltObject::from_float(avgs[2]).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
        #[cfg(not(unix))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getloadavg")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getloadavg() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getloadavg")
    })
}

/// `os.uname()` → tuple[str, str, str, str, str]
/// (sysname, nodename, release, version, machine)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_uname() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(unix)]
        {
            let mut name: libc::utsname = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::uname(&mut name) };
            if rc < 0 {
                return raise_exception::<u64>(_py, "OSError", "uname failed");
            }
            fn cstr_field(bytes: &[libc::c_char]) -> String {
                let s = unsafe { std::ffi::CStr::from_ptr(bytes.as_ptr()) };
                s.to_string_lossy().into_owned()
            }
            let fields = [
                cstr_field(&name.sysname),
                cstr_field(&name.nodename),
                cstr_field(&name.release),
                cstr_field(&name.version),
                cstr_field(&name.machine),
            ];
            let mut elems: [u64; 5] = [MoltObject::none().bits(); 5];
            for (i, f) in fields.iter().enumerate() {
                let ptr = alloc_string(_py, f.as_bytes());
                if ptr.is_null() {
                    for prev in elems.iter().take(i) {
                        if !obj_from_bits(*prev).is_none() {
                            dec_ref_bits(_py, *prev);
                        }
                    }
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                elems[i] = MoltObject::from_ptr(ptr).bits();
            }
            let tup = alloc_tuple(_py, &elems);
            for e in &elems {
                if !obj_from_bits(*e).is_none() {
                    dec_ref_bits(_py, *e);
                }
            }
            if tup.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(tup).bits()
        }
        #[cfg(not(unix))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "uname")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_uname() -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "uname")
    })
}

/// `os.umask(mask)` → int (old mask)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_umask(mask_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mask = to_i64(obj_from_bits(mask_bits)).unwrap_or(0o022) as u32;
        #[cfg(unix)]
        {
            let old = unsafe { libc::umask(mask as libc::mode_t) };
            MoltObject::from_int(old as i64).bits()
        }
        #[cfg(not(unix))]
        {
            let _ = mask;
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "umask")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_umask(_mask_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "umask")
    })
}

// ---------------------------------------------------------------------------
// 4. os.path operations not yet in io.rs
// ---------------------------------------------------------------------------

/// `os.path.commonpath(paths)` → str
/// All paths must share a common root; raises ValueError if they do not.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_commonpath(paths_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let paths_obj = obj_from_bits(paths_bits);
        if paths_obj.is_none() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "commonpath() arg is an empty sequence",
            );
        }
        let paths: Vec<PathBuf> = {
            let Some(ptr) = paths_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "paths must be iterable");
            };
            let type_id = unsafe { object_type_id(ptr) };
            if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "paths must be a list or tuple");
            }
            let elems = unsafe { seq_vec_ref(ptr) };
            if elems.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "commonpath() arg is an empty sequence",
                );
            }
            let mut out = Vec::with_capacity(elems.len());
            for &bits in elems.iter() {
                match path_from_bits(_py, bits) {
                    Ok(p) => out.push(p),
                    Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
                }
            }
            out
        };
        match common_path_impl(&paths) {
            Some(common) => str_bits(_py, &common.to_string_lossy()),
            None => raise_exception::<_>(_py, "ValueError", "paths have different drives or roots"),
        }
    })
}

fn common_path_impl(paths: &[PathBuf]) -> Option<PathBuf> {
    if paths.is_empty() {
        return None;
    }
    // Collect component lists for each path.
    let component_lists: Vec<Vec<Component<'_>>> =
        paths.iter().map(|p| p.components().collect()).collect();
    // Find the minimum length prefix that all paths share.
    let min_len = component_lists.iter().map(|c| c.len()).min().unwrap_or(0);
    let mut common: Vec<Component<'_>> = Vec::new();
    'outer: for i in 0..min_len {
        let first = &component_lists[0][i];
        for comps in component_lists.iter().skip(1) {
            if comps[i] != *first {
                break 'outer;
            }
        }
        common.push(*first);
    }
    if common.is_empty() {
        return None;
    }
    let mut result = PathBuf::new();
    for c in common {
        result.push(c);
    }
    Some(result)
}

/// `os.path.commonprefix(paths)` → str
/// Character-level common prefix of the string representations.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_commonprefix(paths_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let paths_obj = obj_from_bits(paths_bits);
        if paths_obj.is_none() {
            let ptr = alloc_string(_py, b"");
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        }
        let strs: Vec<String> = {
            let Some(ptr) = paths_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "paths must be iterable");
            };
            let type_id = unsafe { object_type_id(ptr) };
            if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "paths must be a list or tuple");
            }
            let elems = unsafe { seq_vec_ref(ptr) };
            if elems.is_empty() {
                let ep = alloc_string(_py, b"");
                return if ep.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ep).bits()
                };
            }
            let mut out = Vec::with_capacity(elems.len());
            for &bits in elems.iter() {
                match string_obj_to_owned(obj_from_bits(bits)) {
                    Some(s) => out.push(s),
                    None => return raise_exception::<_>(_py, "TypeError", "paths must be strings"),
                }
            }
            out
        };
        // Common character prefix.
        let first = &strs[0];
        let prefix_len = strs.iter().skip(1).fold(first.len(), |acc, s| {
            acc.min(
                first
                    .chars()
                    .zip(s.chars())
                    .take_while(|(a, b)| a == b)
                    .count(),
            )
        });
        let prefix: String = first.chars().take(prefix_len).collect();
        str_bits(_py, &prefix)
    })
}

/// `os.path.getsize(path)` → int
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_getsize(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::fs::metadata(&path) {
            Ok(meta) => MoltObject::from_int(meta.len() as i64).bits(),
            Err(err) => os_err_bits(_py, err, "getsize"),
        }
    })
}

/// `os.path.getmtime(path)` → float  (modification time, seconds since epoch)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_getmtime(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(t) => {
                let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                MoltObject::from_float(dur.as_secs_f64()).bits()
            }
            Err(err) => os_err_bits(_py, err, "getmtime"),
        }
    })
}

/// `os.path.getatime(path)` → float  (access time)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_getatime(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::fs::metadata(&path).and_then(|m| m.accessed()) {
            Ok(t) => {
                let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                MoltObject::from_float(dur.as_secs_f64()).bits()
            }
            Err(err) => os_err_bits(_py, err, "getatime"),
        }
    })
}

/// `os.path.getctime(path)` → float  (creation/metadata-change time)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_getctime(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        // std::fs::Metadata::created() is the closest portable equivalent.
        // On Linux it returns ENOTSUP; we fall back to mtime in that case.
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(err) => return os_err_bits(_py, err, "getctime"),
        };
        let t = meta.created().or_else(|_| meta.modified());
        match t {
            Ok(time) => {
                let dur = time
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                MoltObject::from_float(dur.as_secs_f64()).bits()
            }
            Err(err) => os_err_bits(_py, err, "getctime"),
        }
    })
}

/// `os.path.samefile(path1, path2)` → bool
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_samefile(path1_bits: u64, path2_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let p1 = match require_path(_py, path1_bits, "path1") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let p2 = match require_path(_py, path2_bits, "path2") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let m1 = match std::fs::metadata(&p1) {
                Ok(m) => m,
                Err(err) => return os_err_bits(_py, err, "samefile"),
            };
            let m2 = match std::fs::metadata(&p2) {
                Ok(m) => m,
                Err(err) => return os_err_bits(_py, err, "samefile"),
            };
            MoltObject::from_bool(m1.dev() == m2.dev() && m1.ino() == m2.ino()).bits()
        }
        #[cfg(not(unix))]
        {
            // On Windows: resolve to absolute canonicalized paths and compare.
            let c1 = std::fs::canonicalize(&p1);
            let c2 = std::fs::canonicalize(&p2);
            match (c1, c2) {
                (Ok(c1), Ok(c2)) => MoltObject::from_bool(c1 == c2).bits(),
                (Err(err), _) | (_, Err(err)) => os_err_bits(_py, err, "samefile"),
            }
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_samefile(_path1_bits: u64, _path2_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "samefile")
    })
}

// ---------------------------------------------------------------------------
// 5. os constants
// ---------------------------------------------------------------------------

/// `os.sep` → str  ("/" on POSIX, "\\" on Windows)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_sep() -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = std::path::MAIN_SEPARATOR_STR;
        str_bits(_py, sep)
    })
}

/// `os.linesep` → str  ("\n" on POSIX, "\r\n" on Windows)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_linesep() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(windows)]
        {
            str_bits(_py, "\r\n")
        }
        #[cfg(not(windows))]
        {
            str_bits(_py, "\n")
        }
    })
}

/// `os.devnull` → str  ("/dev/null" on POSIX, "nul" on Windows)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_devnull() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(windows)]
        {
            str_bits(_py, "nul")
        }
        #[cfg(not(windows))]
        {
            str_bits(_py, "/dev/null")
        }
    })
}

/// `os.curdir` → str  (".")
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_curdir() -> u64 {
    crate::with_gil_entry!(_py, { str_bits(_py, ".") })
}

/// `os.pardir` → str  ("..")
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_pardir() -> u64 {
    crate::with_gil_entry!(_py, { str_bits(_py, "..") })
}

/// `os.extsep` → str  (".")
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_extsep() -> u64 {
    crate::with_gil_entry!(_py, { str_bits(_py, ".") })
}

/// `os.altsep` → str | None  (None on POSIX, "/" on Windows)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_altsep() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(windows)]
        {
            str_bits(_py, "/")
        }
        #[cfg(not(windows))]
        {
            MoltObject::none().bits()
        }
    })
}

/// `os.pathsep` → str  (":" on POSIX, ";" on Windows)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_pathsep() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(windows)]
        {
            str_bits(_py, ";")
        }
        #[cfg(not(windows))]
        {
            str_bits(_py, ":")
        }
    })
}

// ---------------------------------------------------------------------------
// File descriptor operations (Phase 2)
// ---------------------------------------------------------------------------

/// `os.dup2(fd, fd2)` → int — duplicate fd onto fd2
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_dup2(fd_bits: u64, fd2_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fd must be an integer");
        };
        let Some(fd2) = to_i64(obj_from_bits(fd2_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fd2 must be an integer");
        };
        if fd < 0 || fd2 < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "dup2");
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "dup2")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let result = unsafe { libc::dup2(fd as libc::c_int, fd2 as libc::c_int) };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "dup2");
                }
                return raise_os_error::<u64>(_py, err, "dup2");
            }
            MoltObject::from_int(result as i64).bits()
        }
    })
}

/// `os.lseek(fd, pos, how)` → int — seek within file descriptor
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_lseek(fd_bits: u64, pos_bits: u64, how_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fd must be an integer");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be an integer");
        };
        let Some(how) = to_i64(obj_from_bits(how_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "how must be an integer");
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "lseek");
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "lseek")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let result =
                unsafe { libc::lseek(fd as libc::c_int, pos as libc::off_t, how as libc::c_int) };
            if result == -1 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "lseek");
                }
                return raise_os_error::<u64>(_py, err, "lseek");
            }
            MoltObject::from_int(result as i64).bits()
        }
    })
}

/// `os.ftruncate(fd, length)` → None — truncate fd to length
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_ftruncate(fd_bits: u64, length_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fd must be an integer");
        };
        let Some(length) = to_i64(obj_from_bits(length_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "length must be an integer");
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "ftruncate");
        }
        if length < 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "ftruncate: length must be non-negative",
            );
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "ftruncate")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let result = unsafe { libc::ftruncate(fd as libc::c_int, length as libc::off_t) };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "ftruncate");
                }
                return raise_os_error::<u64>(_py, err, "ftruncate");
            }
            MoltObject::none().bits()
        }
    })
}

/// `os.isatty(fd)` → bool — return True if fd is a tty
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_isatty(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fd must be an integer");
        };
        if fd < 0 {
            return MoltObject::from_bool(false).bits();
        }
        #[cfg(target_arch = "wasm32")]
        {
            MoltObject::from_bool(false).bits()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let result = unsafe { libc::isatty(fd as libc::c_int) };
            MoltObject::from_bool(result == 1).bits()
        }
    })
}

/// `os.fdopen(fd, mode, closefd)` — stub: raises NotImplementedError
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_fdopen(_fd_bits: u64, _mode_bits: u64, _closefd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "os.fdopen() is not yet implemented in Molt",
        )
    })
}

// ---------------------------------------------------------------------------
// Process operations (Phase 2)
// ---------------------------------------------------------------------------

/// `os.kill(pid, sig)` → None — send signal to process
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_kill(pid_bits: u64, sig_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "process") {
            return raise_exception::<_>(_py, "PermissionError", "missing process capability");
        }
        let Some(pid) = to_i64(obj_from_bits(pid_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pid must be an integer");
        };
        let Some(sig) = to_i64(obj_from_bits(sig_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "sig must be an integer");
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (pid, sig);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "kill")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let result = unsafe { libc::kill(pid as libc::pid_t, sig as libc::c_int) };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "kill");
                }
                return raise_os_error::<u64>(_py, err, "kill");
            }
            MoltObject::none().bits()
        }
    })
}

/// `os.waitpid(pid, options)` → (pid, status) — wait for process
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_waitpid(pid_bits: u64, options_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "process") {
            return raise_exception::<_>(_py, "PermissionError", "missing process capability");
        }
        let Some(pid) = to_i64(obj_from_bits(pid_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pid must be an integer");
        };
        let Some(options) = to_i64(obj_from_bits(options_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "options must be an integer");
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (pid, options);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "waitpid")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut status: libc::c_int = 0;
            let result =
                unsafe { libc::waitpid(pid as libc::pid_t, &mut status, options as libc::c_int) };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "waitpid");
                }
                return raise_os_error::<u64>(_py, err, "waitpid");
            }
            let elems = [
                MoltObject::from_int(result as i64).bits(),
                MoltObject::from_int(status as i64).bits(),
            ];
            let tup_ptr = alloc_tuple(_py, &elems);
            if tup_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(tup_ptr).bits()
        }
    })
}

/// `os.getpgrp()` → int — get process group
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getpgrp() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getpgrp")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let pgrp = unsafe { libc::getpgrp() };
            MoltObject::from_int(pgrp as i64).bits()
        }
    })
}

/// `os.setpgrp()` → None — set process group
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_setpgrp() -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "process") {
            return raise_exception::<_>(_py, "PermissionError", "missing process capability");
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "setpgrp")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // macOS does not have setpgrp(); use setpgid(0, 0) which is equivalent
            let result = unsafe { libc::setpgid(0, 0) };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "setpgrp");
                }
                return raise_os_error::<u64>(_py, err, "setpgrp");
            }
            MoltObject::none().bits()
        }
    })
}

/// `os.setsid()` → int — create new session
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_setsid() -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "process") {
            return raise_exception::<_>(_py, "PermissionError", "missing process capability");
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "setsid")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let result = unsafe { libc::setsid() };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "setsid");
                }
                return raise_os_error::<u64>(_py, err, "setsid");
            }
            MoltObject::from_int(result as i64).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// System configuration (Phase 2)
// ---------------------------------------------------------------------------

/// `os.sysconf(name)` → int — get system configuration value
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_sysconf(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = to_i64(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "sysconf name must be an integer");
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = name;
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "sysconf")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let result = unsafe { libc::sysconf(name as libc::c_int) };
            if result == -1 {
                // sysconf returns -1 for both errors and "indeterminate" values.
                // Check last OS error — if non-zero, it's a real error.
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error()
                    && errno != 0
                {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "sysconf");
                }
                // errno == 0 means "no limit" / indeterminate — return -1
            }
            MoltObject::from_int(result as i64).bits()
        }
    })
}

/// `os.sysconf_names` — returns flat list [name_str, value_int, ...] of common POSIX sysconf names
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_sysconf_names() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(target_arch = "wasm32")]
        {
            // Return an empty list on WASM — Python side builds dict from it
            let list_ptr = alloc_list(_py, &[]);
            if list_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(list_ptr).bits()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let names: &[(&str, libc::c_int)] = &[
                ("SC_PAGE_SIZE", libc::_SC_PAGE_SIZE),
                ("SC_NPROCESSORS_CONF", libc::_SC_NPROCESSORS_CONF),
                ("SC_NPROCESSORS_ONLN", libc::_SC_NPROCESSORS_ONLN),
                ("SC_CLK_TCK", libc::_SC_CLK_TCK),
                ("SC_OPEN_MAX", libc::_SC_OPEN_MAX),
                ("SC_ARG_MAX", libc::_SC_ARG_MAX),
                ("SC_CHILD_MAX", libc::_SC_CHILD_MAX),
                ("SC_HOST_NAME_MAX", libc::_SC_HOST_NAME_MAX),
                ("SC_LOGIN_NAME_MAX", libc::_SC_LOGIN_NAME_MAX),
                ("SC_PHYS_PAGES", libc::_SC_PHYS_PAGES),
            ];
            let mut entries: Vec<u64> = Vec::with_capacity(names.len() * 2);
            for (name_str, val) in names {
                let s_ptr = alloc_string(_py, name_str.as_bytes());
                if s_ptr.is_null() {
                    for e in &entries {
                        dec_ref_bits(_py, *e);
                    }
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                entries.push(MoltObject::from_ptr(s_ptr).bits());
                entries.push(MoltObject::from_int(*val as i64).bits());
            }
            let list_ptr = alloc_list(_py, &entries);
            // dec_ref the string entries (ints are inline, no dec_ref needed)
            for (i, e) in entries.iter().enumerate() {
                if i % 2 == 0 {
                    // string entries at even indices
                    dec_ref_bits(_py, *e);
                }
            }
            if list_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// Filesystem operations (Phase 2)
// ---------------------------------------------------------------------------

/// `os.path.realpath(path)` → str — resolve path following symlinks
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_realpath(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::fs::canonicalize(&path) {
            Ok(resolved) => str_bits(_py, &resolved.to_string_lossy()),
            Err(err) => os_err_bits(_py, err, "realpath"),
        }
    })
}

/// `os.utime(path, atime, mtime)` → None — set access/modification times
///
/// `atime_bits` and `mtime_bits` are floats, or both None for current time.
/// The Python wrapper decomposes the `times` tuple before calling this.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_utime(path_bits: u64, atime_bits: u64, mtime_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, atime_bits, mtime_bits);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "utime")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::ffi::CString;

            let c_path = match CString::new(path.to_string_lossy().as_bytes()) {
                Ok(c) => c,
                Err(_) => {
                    return raise_exception::<_>(_py, "ValueError", "path contains null byte");
                }
            };

            let atime_obj = obj_from_bits(atime_bits);
            if atime_obj.is_none() {
                // None means set to current time — pass null to utimes
                let result = unsafe { libc::utimes(c_path.as_ptr(), std::ptr::null()) };
                if result < 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "utime");
                    }
                    return raise_os_error::<u64>(_py, err, "utime");
                }
            } else {
                let atime_f = match to_f64(atime_obj) {
                    Some(v) => v,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "utime: atime must be a number",
                        );
                    }
                };
                let mtime_f = match to_f64(obj_from_bits(mtime_bits)) {
                    Some(v) => v,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "utime: mtime must be a number",
                        );
                    }
                };
                let tv = [
                    libc::timeval {
                        tv_sec: atime_f as libc::time_t,
                        tv_usec: ((atime_f.fract() * 1_000_000.0) as libc::suseconds_t),
                    },
                    libc::timeval {
                        tv_sec: mtime_f as libc::time_t,
                        tv_usec: ((mtime_f.fract() * 1_000_000.0) as libc::suseconds_t),
                    },
                ];
                let result = unsafe { libc::utimes(c_path.as_ptr(), tv.as_ptr()) };
                if result < 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "utime");
                    }
                    return raise_os_error::<u64>(_py, err, "utime");
                }
            }
            MoltObject::none().bits()
        }
    })
}

/// `os.sendfile(out_fd, in_fd, offset, count)` → int — zero-copy file send
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_sendfile(
    out_fd_bits: u64,
    in_fd_bits: u64,
    offset_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(out_fd) = to_i64(obj_from_bits(out_fd_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "out_fd must be an integer");
        };
        let Some(in_fd) = to_i64(obj_from_bits(in_fd_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "in_fd must be an integer");
        };
        let Some(count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "count must be an integer");
        };
        let offset_obj = obj_from_bits(offset_bits);
        let offset_val = if offset_obj.is_none() {
            None
        } else {
            match to_i64(offset_obj) {
                Some(v) => Some(v),
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "offset must be an integer or None",
                    );
                }
            }
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (out_fd, in_fd, offset_val, count);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "sendfile")
        }
        #[cfg(target_os = "linux")]
        {
            let mut off = offset_val.map(|v| v as libc::off_t);
            let off_ptr = match off.as_mut() {
                Some(o) => o as *mut libc::off_t,
                None => std::ptr::null_mut(),
            };
            let result = unsafe {
                libc::sendfile(
                    out_fd as libc::c_int,
                    in_fd as libc::c_int,
                    off_ptr,
                    count as usize,
                )
            };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "sendfile");
                }
                return raise_os_error::<u64>(_py, err, "sendfile");
            }
            MoltObject::from_int(result as i64).bits()
        }
        #[cfg(target_os = "macos")]
        {
            // macOS sendfile: sendfile(in_fd, out_fd, offset, &mut len, hdtr, flags)
            // Note reversed fd order compared to Linux!
            let off = offset_val.unwrap_or(0) as libc::off_t;
            let mut len = count as libc::off_t;
            let result = unsafe {
                libc::sendfile(
                    in_fd as libc::c_int,
                    out_fd as libc::c_int,
                    off,
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "sendfile");
                }
                return raise_os_error::<u64>(_py, err, "sendfile");
            }
            // On macOS, len is set to the number of bytes sent
            MoltObject::from_int(len as i64).bits()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_arch = "wasm32")))]
        {
            let _ = (out_fd, in_fd, offset_val, count);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "sendfile")
        }
    })
}

// ---------------------------------------------------------------------------
// waitpid status macros — intrinsic-backed
// ---------------------------------------------------------------------------

/// `os.WIFEXITED(status)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wifexited(status_bits: u64) -> u64 {
    let status = to_i64(obj_from_bits(status_bits)).unwrap_or(0) as i32;
    let result = (status & 0x7F) == 0;
    MoltObject::from_bool(result).bits()
}

/// `os.WEXITSTATUS(status)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wexitstatus(status_bits: u64) -> u64 {
    let status = to_i64(obj_from_bits(status_bits)).unwrap_or(0) as i32;
    let result = (status >> 8) & 0xFF;
    MoltObject::from_int(result as i64).bits()
}

/// `os.WIFSIGNALED(status)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wifsignaled(status_bits: u64) -> u64 {
    let status = to_i64(obj_from_bits(status_bits)).unwrap_or(0) as i32;
    let result = ((status & 0x7F) + 1) >> 1 > 0;
    MoltObject::from_bool(result).bits()
}

/// `os.WTERMSIG(status)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wtermsig(status_bits: u64) -> u64 {
    let status = to_i64(obj_from_bits(status_bits)).unwrap_or(0) as i32;
    let result = status & 0x7F;
    MoltObject::from_int(result as i64).bits()
}

/// `os.WIFSTOPPED(status)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wifstopped(status_bits: u64) -> u64 {
    let status = to_i64(obj_from_bits(status_bits)).unwrap_or(0) as i32;
    let result = (status & 0xFF) == 0x7F;
    MoltObject::from_bool(result).bits()
}

/// `os.WSTOPSIG(status)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wstopsig(status_bits: u64) -> u64 {
    let status = to_i64(obj_from_bits(status_bits)).unwrap_or(0) as i32;
    let result = (status >> 8) & 0xFF;
    MoltObject::from_int(result as i64).bits()
}

/// `os.fspath(path)` -> str | bytes
/// Resolves PathLike objects by calling __fspath__.
/// If path is already str or bytes, returns it directly.
/// Otherwise attempts __fspath__ protocol via path_from_bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_fspath(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(path_bits);
        // Fast path: already str
        if string_obj_to_owned(obj).is_some() {
            inc_ref_bits(_py, path_bits);
            return path_bits;
        }
        // Fast path: already bytes (check via type_id)
        if let Some(ptr) = obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == crate::object::type_ids::TYPE_ID_BYTES {
                inc_ref_bits(_py, path_bits);
                return path_bits;
            }
        }
        // Try __fspath__ protocol via path_from_bits
        match path_from_bits(_py, path_bits) {
            Ok(pathbuf) => {
                let s = pathbuf.to_string_lossy();
                str_bits(_py, &s)
            }
            Err(_msg) => {
                let tn = type_name(_py, obj);
                let msg = format!("expected str, bytes or os.PathLike object, not {}", tn);
                raise_exception::<u64>(_py, "TypeError", &msg)
            }
        }
    })
}

// ---------------------------------------------------------------------------
// 9. Tier-0 gaps for click / trio / httpx support
// ---------------------------------------------------------------------------

/// `os.environ` → dict[str, str]  (snapshot of the process environment)
///
/// Returns a new dict each call; the Python wrapper caches it in `os.environ`
/// and keeps it synchronised via `os.putenv` / `os.unsetenv`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_environ() -> u64 {
    crate::with_gil_entry!(_py, {
        let vars: Vec<(String, String)> = std::env::vars().collect();
        let mut pairs: Vec<u64> = Vec::with_capacity(vars.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(vars.len() * 2);
        for (key, value) in &vars {
            let k_ptr = alloc_string(_py, key.as_bytes());
            let v_ptr = alloc_string(_py, value.as_bytes());
            if k_ptr.is_null() || v_ptr.is_null() {
                if !k_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(k_ptr).bits());
                }
                if !v_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(v_ptr).bits());
                }
                for bits in &owned {
                    dec_ref_bits(_py, *bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let k_bits = MoltObject::from_ptr(k_ptr).bits();
            let v_bits = MoltObject::from_ptr(v_ptr).bits();
            pairs.push(k_bits);
            pairs.push(v_bits);
            owned.push(k_bits);
            owned.push(v_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in &owned {
            dec_ref_bits(_py, *bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

/// `os.makedirs(name, mode=0o777, exist_ok=False)` → None
///
/// Recursive directory creation (like `mkdir -p`).
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_makedirs(
    path_bits: u64,
    mode_bits: u64,
    exist_ok_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path(_py, path_bits, "name") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let exist_ok = is_truthy(_py, obj_from_bits(exist_ok_bits));
        let _mode = to_i64(obj_from_bits(mode_bits)).unwrap_or(0o777);

        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true).mode(_mode as u32);
            match builder.create(&path) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) if exist_ok && err.kind() == std::io::ErrorKind::AlreadyExists => {
                    // exist_ok=True: succeed if the target is a directory
                    if path.is_dir() {
                        MoltObject::none().bits()
                    } else {
                        os_err_bits(_py, err, "makedirs")
                    }
                }
                Err(err) => os_err_bits(_py, err, "makedirs"),
            }
        }
        #[cfg(not(unix))]
        {
            match std::fs::create_dir_all(&path) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) if exist_ok && err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if path.is_dir() {
                        MoltObject::none().bits()
                    } else {
                        os_err_bits(_py, err, "makedirs")
                    }
                }
                Err(err) => os_err_bits(_py, err, "makedirs"),
            }
        }
    })
}

/// `os.path.join(a, *p)` → str
///
/// Joins one or more path components. Accepts exactly two args at the intrinsic
/// level; the Python wrapper folds over `*p` by repeated calls.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_join(base_bits: u64, part_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let base_str = match string_obj_to_owned(obj_from_bits(base_bits)) {
            Some(s) => s,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "expected str, bytes or os.PathLike object",
                );
            }
        };
        let part_str = match string_obj_to_owned(obj_from_bits(part_bits)) {
            Some(s) => s,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "expected str, bytes or os.PathLike object",
                );
            }
        };
        let result = Path::new(&base_str).join(&part_str);
        str_bits(_py, &result.to_string_lossy())
    })
}

/// `os.path.exists(path)` → bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_exists(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(_) => return MoltObject::from_bool(false).bits(),
        };
        MoltObject::from_bool(path.exists()).bits()
    })
}

/// `os.path.isfile(path)` → bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_isfile(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(_) => return MoltObject::from_bool(false).bits(),
        };
        MoltObject::from_bool(path.is_file()).bits()
    })
}

/// `os.path.isdir(path)` → bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_isdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(_) => return MoltObject::from_bool(false).bits(),
        };
        MoltObject::from_bool(path.is_dir()).bits()
    })
}
