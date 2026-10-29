// Additional os intrinsics: directory ops, file metadata, process info, and
// os.path helpers not covered by the main runtime I/O and platform modules.
//
// Host operation names resolve through the generated host-capability registry.
// The runtime host enforces target availability and reports every missing grant;
// this satellite supplies operation identity and audit arguments, never grants.
//
// WASM notes: most operations that require real POSIX semantics are guarded with
// #[cfg(not(target_arch = "wasm32"))].  On WASM we return ENOSYS equivalents so
// the Python wrapper can raise NotImplementedError.

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(not(target_arch = "wasm32"))]
const EBADF_VALUE: i32 = libc::EBADF;
#[cfg(target_arch = "wasm32")]
const EBADF_VALUE: i32 = 9; // EBADF = 9 on WASI

use crate::bridge::*;
use molt_obj_model::MoltObject;
use molt_runtime_core::host_capabilities_generated::OperationId;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Map a std::io::Error to the appropriate Python exception bits.
#[inline]
fn os_err_bits(_py: &CoreGilToken, err: std::io::Error, ctx: &str) -> u64 {
    raise_os_error::<u64>(_py, err, ctx)
}

/// Borrow a `str` from a runtime string object, raising TypeError on failure.
#[inline]
#[allow(dead_code)]
fn require_str(_py: &CoreGilToken, bits: u64, label: &str) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(s) => Ok(s),
        None => Err(raise_exception::<u64>(
            _py,
            BuiltinExc::TypeError,
            &format!("{label} must be str"),
        )),
    }
}

/// Resolve a path argument (str | bytes | os.PathLike) to a PathBuf.
#[inline]
fn require_path(_py: &CoreGilToken, bits: u64, _label: &str) -> Result<PathBuf, u64> {
    path_from_bits(_py, bits)
}

/// Allocate a runtime string from a Rust &str slice, returning bits.
/// Returns None bits on allocation failure rather than propagating OOM (consistent
/// with existing patterns in io.rs).
#[inline]
fn str_bits(_py: &CoreGilToken, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// CPython's `int` argument converter (`_PyLong_AsInt` after `__index__`),
/// shared by every `mode`, `mask`, `fd` and `status` argument here. A failed
/// conversion leaves its TypeError/OverflowError (or the `__index__` error)
/// pending and the caller returns; it never substitutes a default value.
#[inline]
fn c_int_arg(_py: &CoreGilToken, bits: u64) -> Result<i32, u64> {
    index_c_int_from_obj(_py, bits).ok_or_else(|| MoltObject::none().bits())
}

/// CPython's `Py_off_t` argument converter, for `length`/`pos` arguments.
#[inline]
fn off_t_arg(_py: &CoreGilToken, bits: u64) -> Result<i64, u64> {
    index_off_t_from_obj(_py, bits).ok_or_else(|| MoltObject::none().bits())
}

/// `mkdir(2)` with CPython's `mode` argument; targets without POSIX modes
/// accept and ignore it, as CPython does on Windows.
fn mkdir_with_mode(path: &Path, mode: i32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(mode as u32).create(path)
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::create_dir(path)
    }
}

// ---------------------------------------------------------------------------
// 1. Directory operations
// ---------------------------------------------------------------------------

/// `os.listdir(path)` → list[str] or list[bytes], matching the path flavor
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_listdir(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.listdir", AuditArg::None) {
            return error;
        }
        let (path, bytes_flavor) = match path_from_bits_with_flavor(_py, path_bits) {
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
                    let bits = path_result(_py, Path::new(&name), bytes_flavor);
                    if obj_from_bits(bits).is_none() {
                        for e in &entries {
                            dec_ref_bits(_py, *e);
                        }
                        return MoltObject::none().bits();
                    }
                    entries.push(bits);
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
            return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

/// `os.scandir(path)` → entries whose name/path match the input str/bytes flavor.
/// Each tuple: (name, path, is_dir, is_file, is_symlink)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_scandir(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.scandir", AuditArg::None) {
            return error;
        }
        let (dir_path, bytes_flavor) = match path_from_bits_with_flavor(_py, path_bits) {
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
                    let full_path = entry.path();

                    let meta_sym = std::fs::symlink_metadata(entry.path());
                    let is_symlink = meta_sym
                        .as_ref()
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false);
                    let meta = std::fs::metadata(entry.path());
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let is_file = meta.as_ref().map(|m| m.is_file()).unwrap_or(false);

                    let name_bits = path_result(_py, Path::new(&name_os), bytes_flavor);
                    let path_bits = if obj_from_bits(name_bits).is_none() {
                        MoltObject::none().bits()
                    } else {
                        path_result(_py, &full_path, bytes_flavor)
                    };
                    if obj_from_bits(name_bits).is_none() || obj_from_bits(path_bits).is_none() {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, path_bits);
                        for t in &tuples {
                            dec_ref_bits(_py, *t);
                        }
                        return MoltObject::none().bits();
                    }
                    let elems = [
                        name_bits,
                        path_bits,
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
                        return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
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
            return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

/// `os.walk(top, topdown=True, followlinks=False)` → directory/name triplets
/// whose path components match the input str/bytes flavor.
///
/// Each element: (dirpath, dirnames, filenames).
/// We collect the full walk eagerly (no generator semantics in the intrinsic layer).
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_walk(top_bits: u64, topdown_bits: u64, followlinks_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.walk", AuditArg::None) {
            return error;
        }
        let (top, bytes_flavor) = match path_from_bits_with_flavor(_py, top_bits) {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let topdown = is_truthy(_py, obj_from_bits(topdown_bits));
        let followlinks = is_truthy(_py, obj_from_bits(followlinks_bits));

        let mut results: Vec<(PathBuf, Vec<PathBuf>, Vec<PathBuf>)> = Vec::new();
        walk_dir_collect(&top, topdown, followlinks, &mut results);

        let mut triplets: Vec<u64> = Vec::with_capacity(results.len());
        for (dirpath, dirnames, filenames) in &results {
            let mut elems = [MoltObject::none().bits(); 3];
            elems[0] = path_result(_py, dirpath, bytes_flavor);
            if !obj_from_bits(elems[0]).is_none() {
                elems[1] = path_list_result(_py, dirnames, bytes_flavor);
            }
            if !obj_from_bits(elems[1]).is_none() {
                elems[2] = path_list_result(_py, filenames, bytes_flavor);
            }
            if elems.iter().any(|bits| obj_from_bits(*bits).is_none()) {
                for bits in elems {
                    dec_ref_bits(_py, bits);
                }
                for bits in triplets {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let tup_ptr = alloc_tuple(_py, &elems);
            for e in &elems {
                dec_ref_bits(_py, *e);
            }
            if tup_ptr.is_null() {
                for t in &triplets {
                    dec_ref_bits(_py, *t);
                }
                return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
            }
            triplets.push(MoltObject::from_ptr(tup_ptr).bits());
        }
        let list_ptr = alloc_list(_py, &triplets);
        for t in &triplets {
            dec_ref_bits(_py, *t);
        }
        if list_ptr.is_null() {
            return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

fn path_list_result(py: &CoreGilToken, paths: &[PathBuf], bytes: bool) -> u64 {
    let mut values = Vec::with_capacity(paths.len());
    for path in paths {
        let bits = path_result(py, path, bytes);
        if obj_from_bits(bits).is_none() {
            for value in values {
                dec_ref_bits(py, value);
            }
            return bits;
        }
        values.push(bits);
    }
    let pointer = alloc_list(py, &values);
    for value in values {
        dec_ref_bits(py, value);
    }
    if pointer.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(pointer).bits()
    }
}

/// Recursive walk helper (pure Rust, no GIL calls).
fn walk_dir_collect(
    dir: &Path,
    topdown: bool,
    followlinks: bool,
    out: &mut Vec<(PathBuf, Vec<PathBuf>, Vec<PathBuf>)>,
) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    let mut dirnames: Vec<PathBuf> = Vec::new();
    let mut filenames: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry_result in rd {
        let Ok(entry) = entry_result else { continue };
        let name = PathBuf::from(entry.file_name());
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
    let dirpath = dir.to_path_buf();
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
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getcwd() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getcwd", AuditArg::None) {
            return error;
        }
        match std::env::current_dir() {
            Ok(path) => path_result(_py, &path, false),
            Err(err) => os_err_bits(_py, err, "getcwd"),
        }
    })
}

/// `os.chdir(path)` → None
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_chdir(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.chdir", AuditArg::None) {
            return error;
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
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.mkdir", AuditArg::None) {
            return error;
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let mode = match c_int_arg(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        match mkdir_with_mode(&path, mode) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => os_err_bits(_py, err, "mkdir"),
        }
    })
}

/// `os.rmdir(path)` → None  (remove empty directory)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_rmdir(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.rmdir", AuditArg::None) {
            return error;
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
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.removedirs", AuditArg::None) {
            return error;
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

/// `os.access(path, mode)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_access(path_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(py, {
        if let Err(error) = require_operation(py, "os.access", AuditArg::None) {
            return error;
        }
        let path = match require_path(py, path_bits, "path") {
            Ok(path) => path,
            Err(error) => return error,
        };
        if path.as_os_str().as_encoded_bytes().contains(&0) {
            return raise_exception::<u64>(py, BuiltinExc::ValueError, "embedded null byte");
        }
        let mode = match index_c_int_from_obj(py, mode_bits) {
            Some(mode) => mode,
            None => return MoltObject::none().bits(),
        };
        MoltObject::from_bool(molt_runtime_platform::path_text::path_access(&path, mode)).bits()
    })
}

/// `os.chmod(path, mode)` → None
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_chmod(path_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.chmod", AuditArg::None) {
            return error;
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let mode = match c_int_arg(_py, mode_bits) {
            Ok(mode) => mode as u32,
            Err(bits) => return bits,
        };
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
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.link", AuditArg::None) {
            return error;
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "link")
    })
}

/// `os.symlink(src, dst)` → None
/// Thin delegation to the existing molt_path_symlink behaviour.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_symlink(src_bits: u64, dst_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.symlink", AuditArg::None) {
            return error;
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "symlink")
    })
}

/// `os.truncate(path, length)` → None
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_truncate(path_bits: u64, length_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.truncate", AuditArg::None) {
            return error;
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let length = match off_t_arg(_py, length_bits) {
            Ok(length) => length,
            Err(bits) => return bits,
        };
        // truncate(2) rejects a negative length with EINVAL before path lookup.
        let Ok(length) = u64::try_from(length) else {
            return raise_os_error_errno::<u64>(_py, libc::EINVAL as i64, "truncate");
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "truncate")
    })
}

// ---------------------------------------------------------------------------
// 3. Process information
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn molt_getpid_host() -> i64;
}

/// `os.getpid()` → int, using the same host transport on every WASM profile.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getpid() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getpid", AuditArg::None) {
            return error;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let pid = unsafe { molt_getpid_host() };
            MoltObject::from_int(pid.max(0)).bits()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            MoltObject::from_int(std::process::id() as i64).bits()
        }
    })
}

/// `os.getppid()` → int
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getppid() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getppid", AuditArg::None) {
            return error;
        }
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_exception::<u64>(
            _py,
            BuiltinExc::OSError,
            "[Errno 38] Function not implemented: 'getppid'",
        )
    })
}

/// `os.getuid()` → int  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getuid() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getuid", AuditArg::None) {
            return error;
        }
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getuid")
    })
}

/// `os.getgid()` → int  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getgid() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getgid", AuditArg::None) {
            return error;
        }
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getgid")
    })
}

/// `os.geteuid()` → int  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_geteuid() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.geteuid", AuditArg::None) {
            return error;
        }
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "geteuid")
    })
}

/// `os.getegid()` → int  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getegid() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getegid", AuditArg::None) {
            return error;
        }
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getegid")
    })
}

/// `os.getlogin()` → str
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getlogin() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getlogin", AuditArg::None) {
            return error;
        }
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
            if let Ok(val) = std::env::var("USERNAME")
                && !val.is_empty()
            {
                return str_bits(_py, &val);
            }
            raise_exception::<u64>(_py, BuiltinExc::OSError, "getlogin failed")
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getlogin")
    })
}

/// `os.cpu_count()` → int | None
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_cpu_count() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.cpu_count", AuditArg::None) {
            return error;
        }
        let n = num_cpus::get();
        MoltObject::from_int(n as i64).bits()
    })
}

/// `os.get_terminal_size(fd=1)` → tuple[int, int]  (columns, lines)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_get_terminal_size(fd_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let fd: libc::c_int = match c_int_arg(_py, fd_bits) {
            Ok(fd) => fd,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
            if rc < 0 || ws.ws_col == 0 || ws.ws_row == 0 {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::OSError,
                    "could not get terminal size",
                );
            }
            let elems = [
                MoltObject::from_int(ws.ws_col as i64).bits(),
                MoltObject::from_int(ws.ws_row as i64).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_terminal_size")
    })
}

/// `os.getloadavg()` → tuple[float, float, float]  (Unix only)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getloadavg() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getloadavg", AuditArg::None) {
            return error;
        }
        #[cfg(unix)]
        {
            let mut avgs: [f64; 3] = [0.0; 3];
            let rc = unsafe { libc::getloadavg(avgs.as_mut_ptr(), 3) };
            if rc < 0 {
                return raise_exception::<u64>(_py, BuiltinExc::OSError, "getloadavg failed");
            }
            let elems = [
                MoltObject::from_float(avgs[0]).bits(),
                MoltObject::from_float(avgs[1]).bits(),
                MoltObject::from_float(avgs[2]).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getloadavg")
    })
}

/// `os.uname()` → tuple[str, str, str, str, str]
/// (sysname, nodename, release, version, machine)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_uname() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.uname", AuditArg::None) {
            return error;
        }
        #[cfg(unix)]
        {
            let mut name: libc::utsname = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::uname(&mut name) };
            if rc < 0 {
                return raise_exception::<u64>(_py, BuiltinExc::OSError, "uname failed");
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
                    return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
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
                return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "uname")
    })
}

/// `os.umask(mask)` → int (old mask)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_umask(mask_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.umask", AuditArg::None) {
            return error;
        }
        let mask = match c_int_arg(_py, mask_bits) {
            Ok(mask) => mask as u32,
            Err(bits) => return bits,
        };
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "umask")
    })
}

// ---------------------------------------------------------------------------
// 4. os.path operations not yet in io.rs
// ---------------------------------------------------------------------------

/// `os.path.commonpath(paths)` → str or bytes, matching the path flavor
/// All paths must share a common root; raises ValueError if they do not.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_commonpath(paths_bits: u64, windows_utf16_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let paths_obj = obj_from_bits(paths_bits);
        if paths_obj.is_none() {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "commonpath() arg is an empty sequence",
            );
        }
        let windows_utf16 = cfg!(windows) && is_truthy(_py, obj_from_bits(windows_utf16_bits));
        let mut bytes_flavor = None;
        let paths: Vec<(Vec<u8>, (usize, usize))> = {
            let Some(ptr) = paths_obj.as_ptr() else {
                return raise_exception::<_>(_py, BuiltinExc::TypeError, "paths must be iterable");
            };
            let type_id = unsafe { object_type_id(ptr) };
            if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "paths must be a list or tuple",
                );
            }
            let elems = unsafe { seq_snapshot(ptr) };
            if elems.is_empty() {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::ValueError,
                    "commonpath() arg is an empty sequence",
                );
            }
            let mut values = Vec::with_capacity(elems.len());
            for &bits in elems.iter() {
                let value = fspath_bits(bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                values.push(unsafe { ProtectedRuntimeValue::from_owned_bits(_py, value) });
            }
            let mut out = Vec::with_capacity(values.len());
            for owner in &values {
                let value = owner.bits();
                let is_bytes = unsafe {
                    object_type_id(obj_from_bits(value).as_ptr().expect("fspath str/bytes"))
                } == TYPE_ID_BYTES;
                // A mixed component fails at replace, before its splitroot codec.
                // Earlier same-flavor codec failures still take precedence.
                if bytes_flavor.is_some_and(|previous| previous != is_bytes) {
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::TypeError,
                        "Can't mix strings and bytes in path components",
                    );
                }
                bytes_flavor = Some(is_bytes);
                let raw = if windows_utf16 {
                    let converted = path_from_bits_with_flavor(_py, value);
                    match converted {
                        Ok((path, bytes)) => (path.into_os_string().into_encoded_bytes(), bytes),
                        Err(error) => return error,
                    }
                } else if let Some(text) = string_obj_bytes(obj_from_bits(value)) {
                    (text, false)
                } else {
                    let pointer = obj_from_bits(value).as_ptr().expect("fspath str/bytes");
                    (
                        unsafe { bytes_like_slice(pointer) }
                            .expect("fspath bytes")
                            .to_vec(),
                        true,
                    )
                };
                let coordinate = if windows_utf16 {
                    molt_runtime_platform::path_text::PathCoordinate::WindowsUtf16
                } else if raw.1 {
                    molt_runtime_platform::path_text::PathCoordinate::Bytes
                } else {
                    molt_runtime_platform::path_text::PathCoordinate::PythonCodePoints
                };
                let offsets = molt_runtime_platform::path_text::path_root_offsets(
                    &raw.0,
                    std::path::MAIN_SEPARATOR as u8,
                    coordinate,
                );
                if windows_utf16 && raw.1 {
                    // Preserve the actual codec-projected byte pieces, not merely
                    // offsets into potentially noncanonical input spellings.
                    let mut projected = Vec::new();
                    let mut projected_offsets = [0; 2];
                    for (index, part) in [
                        &raw.0[..offsets.0],
                        &raw.0[offsets.0..offsets.1],
                        &raw.0[offsets.1..],
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let value = unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(part) };
                        let bits = path_result(_py, Path::new(value), true);
                        let owner = unsafe { ProtectedRuntimeValue::from_owned_bits(_py, bits) };
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        let pointer = obj_from_bits(owner.bits())
                            .as_ptr()
                            .expect("path bytes result");
                        projected.extend_from_slice(
                            unsafe { bytes_like_slice(pointer) }.expect("path bytes result"),
                        );
                        if index < 2 {
                            projected_offsets[index] = projected.len();
                        }
                    }
                    out.push((projected, (projected_offsets[0], projected_offsets[1])));
                } else {
                    out.push((raw.0, offsets));
                }
            }
            out
        };
        match common_path_impl(&paths) {
            Some(common) => {
                let pointer = if bytes_flavor == Some(true) {
                    alloc_bytes(_py, &common)
                } else {
                    alloc_string(_py, &common)
                };
                if pointer.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(pointer).bits()
                }
            }
            None => raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "paths have different drives or roots",
            ),
        }
    })
}

fn common_path_impl(paths: &[(Vec<u8>, (usize, usize))]) -> Option<Vec<u8>> {
    let sep = std::path::MAIN_SEPARATOR as u8;
    let normalized: Vec<Vec<u8>> = paths
        .iter()
        .map(|(path, _)| {
            path.iter()
                .map(|&byte| {
                    if sep == b'\\' && byte == b'/' {
                        sep
                    } else {
                        byte
                    }
                })
                .collect()
        })
        .collect();
    let mut components = Vec::with_capacity(paths.len());
    let mut anchor: Option<(&[u8], bool)> = None;
    for (path, (_, (drive, root))) in normalized.iter().zip(paths) {
        let (drive, root) = (*drive, *root);
        let current = (&path[..drive], root != drive);
        if let Some(previous) = anchor {
            if previous.0 != current.0 || previous.1 != current.1 {
                return None;
            }
        } else {
            anchor = Some(current);
        }
        components.push(
            path[root..]
                .split(|&byte| byte == sep)
                .filter(|part| !part.is_empty() && *part != b".")
                .collect::<Vec<_>>(),
        );
    }
    let (drive, rooted) = anchor?;
    let first = components.first()?;
    let count = (0..first.len())
        .take_while(|&index| {
            components
                .iter()
                .all(|parts| parts.get(index) == Some(&first[index]))
        })
        .count();
    let mut out = drive.to_vec();
    if rooted {
        out.push(sep);
    }
    for (index, part) in first[..count].iter().enumerate() {
        if index != 0 {
            out.push(sep);
        }
        out.extend_from_slice(part);
    }
    Some(out)
}

/// `os.path.commonprefix(paths)` → str
/// Character-level common prefix of the string representations.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_commonprefix(paths_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
                return raise_exception::<_>(_py, BuiltinExc::TypeError, "paths must be iterable");
            };
            let type_id = unsafe { object_type_id(ptr) };
            if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "paths must be a list or tuple",
                );
            }
            let elems = unsafe { seq_snapshot(ptr) };
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
                    None => {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
                            "paths must be strings",
                        );
                    }
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
    molt_runtime_core::with_core_gil!(_py, {
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        match path_metadata(_py, &path, true, OperationId::OsPathGetsize, false) {
            Ok(Some(metadata)) => match i64::try_from(metadata.fields[6]) {
                Ok(size) => molt_runtime_core::rt_int(size),
                Err(_) => raise_exception::<u64>(
                    _py,
                    BuiltinExc::OverflowError,
                    "file size exceeds runtime integer range",
                ),
            },
            Ok(None) => MoltObject::none().bits(),
            Err(error) => error,
        }
    })
}

/// `os.path.getmtime(path)` → float  (modification time, seconds since epoch)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_getmtime(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        match path_metadata(_py, &path, true, OperationId::OsPathGetmtime, false) {
            Ok(Some(metadata)) => MoltObject::from_float(metadata.times[1].as_float()).bits(),
            Ok(None) => MoltObject::none().bits(),
            Err(error) => error,
        }
    })
}

/// `os.path.getatime(path)` → float  (access time)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_getatime(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        match path_metadata(_py, &path, true, OperationId::OsPathGetatime, false) {
            Ok(Some(metadata)) => MoltObject::from_float(metadata.times[0].as_float()).bits(),
            Ok(None) => MoltObject::none().bits(),
            Err(error) => error,
        }
    })
}

/// `os.path.getctime(path)` → float  (creation/metadata-change time)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_getctime(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        match path_metadata(_py, &path, true, OperationId::OsPathGetctime, false) {
            Ok(Some(metadata)) => MoltObject::from_float(metadata.times[2].as_float()).bits(),
            Ok(None) => MoltObject::none().bits(),
            Err(error) => error,
        }
    })
}

/// `os.path.samefile(path1, path2)` uses the selected filesystem identity.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_samefile(path1_bits: u64, path2_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let left = match path_from_bits(_py, path1_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let right = match path_from_bits(_py, path2_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        paths_same_file(_py, &left, &right, OperationId::OsPathSamefile)
    })
}

// ---------------------------------------------------------------------------
// 5. os constants
// ---------------------------------------------------------------------------

/// `os.sep` → str  ("/" on POSIX, "\\" on Windows)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_sep() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let sep = std::path::MAIN_SEPARATOR_STR;
        str_bits(_py, sep)
    })
}

/// `os.linesep` → str  ("\n" on POSIX, "\r\n" on Windows)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_linesep() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, str_bits(_py, "."))
}

/// `os.pardir` → str  ("..")
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_pardir() -> u64 {
    molt_runtime_core::with_core_gil!(_py, str_bits(_py, ".."))
}

/// `os.extsep` → str  (".")
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_extsep() -> u64 {
    molt_runtime_core::with_core_gil!(_py, str_bits(_py, "."))
}

/// `os.altsep` → str | None  (None on POSIX, "/" on Windows)
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_altsep() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.dup2", AuditArg::None) {
            return error;
        }
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "fd must be an integer");
        };
        let Some(fd2) = to_i64(obj_from_bits(fd2_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "fd2 must be an integer");
        };
        if fd < 0 || fd2 < 0 {
            return raise_os_error_errno::<u64>(_py, EBADF_VALUE as i64, "dup2");
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
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.lseek", AuditArg::None) {
            return error;
        }
        let Some(fd) = index_c_int_from_obj(_py, fd_bits) else {
            return MoltObject::none().bits();
        };
        let Some(pos) = index_off_t_from_obj(_py, pos_bits) else {
            return MoltObject::none().bits();
        };
        #[cfg(unix)]
        let pos = {
            let host_pos = pos as libc::off_t;
            if i128::from(host_pos) != i128::from(pos) {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::OverflowError,
                    "Python int too large to convert to C off_t",
                );
            }
            host_pos
        };
        let Some(how) = index_c_int_from_obj(_py, how_bits) else {
            return MoltObject::none().bits();
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, EBADF_VALUE as i64, "lseek");
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (pos, how);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "lseek")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            let result = unsafe { libc::lseek(fd, pos, how) };
            #[cfg(windows)]
            let result = unsafe { libc::lseek64(fd, pos, how) };
            #[cfg(not(any(unix, windows)))]
            let result = -1;
            if result == -1 {
                #[cfg(windows)]
                let err = std::io::Error::from_raw_os_error(unsafe {
                    *molt_runtime_platform::windows_abi::_errno()
                });
                #[cfg(not(windows))]
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "lseek");
                }
                return raise_os_error::<u64>(_py, err, "lseek");
            }
            rt_int(result as i64)
        }
    })
}

/// `os.ftruncate(fd, length)` → None — truncate fd to length
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_ftruncate(fd_bits: u64, length_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.ftruncate", AuditArg::None) {
            return error;
        }
        let fd = match c_int_arg(_py, fd_bits) {
            Ok(fd) => fd,
            Err(bits) => return bits,
        };
        let length = match off_t_arg(_py, length_bits) {
            Ok(length) => length,
            Err(bits) => return bits,
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, EBADF_VALUE as i64, "ftruncate");
        }
        // ftruncate(2) owns the negative-length EINVAL.
        #[cfg(target_arch = "wasm32")]
        {
            let _ = length;
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "ftruncate")
        }
        #[cfg(unix)]
        {
            let result = unsafe { libc::ftruncate(fd, length as libc::off_t) };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "ftruncate");
                }
                return raise_os_error::<u64>(_py, err, "ftruncate");
            }
            MoltObject::none().bits()
        }
        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            let _ = (fd, length);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "ftruncate")
        }
    })
}

/// `os.isatty(fd)` → bool — return True if fd is a tty
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_isatty(fd_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "fd must be an integer");
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
    molt_runtime_core::with_core_gil!(_py, {
        raise_exception::<u64>(
            _py,
            BuiltinExc::NotImplementedError,
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
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.kill", AuditArg::None) {
            return error;
        }
        let Some(pid) = to_i64(obj_from_bits(pid_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "pid must be an integer");
        };
        let Some(sig) = to_i64(obj_from_bits(sig_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "sig must be an integer");
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (pid, sig);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "kill")
        }
        #[cfg(unix)]
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
        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            let _ = (pid, sig);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "kill")
        }
    })
}

/// `os.waitpid(pid, options)` → (pid, status) — wait for process
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_waitpid(pid_bits: u64, options_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.waitpid", AuditArg::None) {
            return error;
        }
        let Some(pid) = to_i64(obj_from_bits(pid_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "pid must be an integer");
        };
        let Some(options) = to_i64(obj_from_bits(options_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "options must be an integer");
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (pid, options);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "waitpid")
        }
        #[cfg(unix)]
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
                return raise_exception::<_>(_py, BuiltinExc::MemoryError, "out of memory");
            }
            MoltObject::from_ptr(tup_ptr).bits()
        }
        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            let _ = (pid, options);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "waitpid")
        }
    })
}

/// `os.getpgrp()` → int — get process group
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_getpgrp() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.getpgrp", AuditArg::None) {
            return error;
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getpgrp")
        }
        #[cfg(unix)]
        {
            let pgrp = unsafe { libc::getpgrp() };
            MoltObject::from_int(pgrp as i64).bits()
        }
        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "getpgrp")
        }
    })
}

/// `os.setpgrp()` → None — set process group
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_setpgrp() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.setpgrp", AuditArg::None) {
            return error;
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "setpgrp")
        }
        #[cfg(unix)]
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
        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "setpgrp")
        }
    })
}

/// `os.setsid()` → int — create new session
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_setsid() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.setsid", AuditArg::None) {
            return error;
        }
        #[cfg(target_arch = "wasm32")]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "setsid")
        }
        #[cfg(unix)]
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
        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "setsid")
        }
    })
}

// ---------------------------------------------------------------------------
// System configuration (Phase 2)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
const WASM_SYSCONF_SC_IOV_MAX: i64 = 1;
#[cfg(target_arch = "wasm32")]
const WASM_SYSCONF_SC_PAGE_SIZE: i64 = 2;
#[cfg(target_arch = "wasm32")]
const WASM_SYSCONF_SC_PAGESIZE: i64 = 3;
#[cfg(target_arch = "wasm32")]
const WASM_SYSCONF_SC_NPROCESSORS_CONF: i64 = 4;
#[cfg(target_arch = "wasm32")]
const WASM_SYSCONF_SC_NPROCESSORS_ONLN: i64 = 5;
#[cfg(target_arch = "wasm32")]
const WASM_SYSCONF_ENTRIES: &[(&str, i64, i64)] = &[
    ("SC_IOV_MAX", WASM_SYSCONF_SC_IOV_MAX, 1024),
    ("SC_PAGE_SIZE", WASM_SYSCONF_SC_PAGE_SIZE, 65536),
    ("SC_PAGESIZE", WASM_SYSCONF_SC_PAGESIZE, 65536),
    ("SC_NPROCESSORS_CONF", WASM_SYSCONF_SC_NPROCESSORS_CONF, 1),
    ("SC_NPROCESSORS_ONLN", WASM_SYSCONF_SC_NPROCESSORS_ONLN, 1),
];

/// `os.sysconf(name)` → int — get system configuration value
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_sysconf(name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(name) = to_i64(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "sysconf name must be an integer",
            );
        };
        #[cfg(target_arch = "wasm32")]
        {
            if let Some((_, _, value)) =
                WASM_SYSCONF_ENTRIES.iter().find(|(_, key, _)| *key == name)
            {
                return MoltObject::from_int(*value).bits();
            }
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "sysconf")
        }
        #[cfg(unix)]
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
        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            let _ = name;
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "sysconf")
        }
    })
}

#[cfg(any(unix, target_arch = "wasm32"))]
fn alloc_sysconf_names(
    py: &CoreGilToken,
    names: impl ExactSizeIterator<Item = (&'static str, i64)>,
) -> u64 {
    let mut entries = Vec::with_capacity(names.len() * 2);
    for (name, value) in names {
        let string = alloc_string(py, name.as_bytes());
        if string.is_null() {
            for bits in entries {
                dec_ref_bits(py, bits);
            }
            return raise_exception::<_>(py, BuiltinExc::MemoryError, "out of memory");
        }
        entries.push(MoltObject::from_ptr(string).bits());
        entries.push(MoltObject::from_int(value).bits());
    }
    let list = alloc_list(py, &entries);
    for bits in entries {
        dec_ref_bits(py, bits);
    }
    if list.is_null() {
        return raise_exception::<_>(py, BuiltinExc::MemoryError, "out of memory");
    }
    MoltObject::from_ptr(list).bits()
}

/// Target-owned sysconf names, or None when this target has no sysconf API.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_sysconf_names() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        #[cfg(target_arch = "wasm32")]
        {
            alloc_sysconf_names(
                _py,
                WASM_SYSCONF_ENTRIES
                    .iter()
                    .map(|&(name, key, _)| (name, key)),
            )
        }
        #[cfg(unix)]
        {
            let names: &[(&str, libc::c_int)] = &[
                ("SC_PAGE_SIZE", libc::_SC_PAGE_SIZE),
                ("SC_PAGESIZE", libc::_SC_PAGESIZE),
                ("SC_NPROCESSORS_CONF", libc::_SC_NPROCESSORS_CONF),
                ("SC_NPROCESSORS_ONLN", libc::_SC_NPROCESSORS_ONLN),
                ("SC_CLK_TCK", libc::_SC_CLK_TCK),
                ("SC_OPEN_MAX", libc::_SC_OPEN_MAX),
                ("SC_ARG_MAX", libc::_SC_ARG_MAX),
                ("SC_CHILD_MAX", libc::_SC_CHILD_MAX),
                ("SC_HOST_NAME_MAX", libc::_SC_HOST_NAME_MAX),
                ("SC_LOGIN_NAME_MAX", libc::_SC_LOGIN_NAME_MAX),
                ("SC_PHYS_PAGES", libc::_SC_PHYS_PAGES),
                ("SC_IOV_MAX", libc::_SC_IOV_MAX),
                ("SC_GETPW_R_SIZE_MAX", libc::_SC_GETPW_R_SIZE_MAX),
                ("SC_GETGR_R_SIZE_MAX", libc::_SC_GETGR_R_SIZE_MAX),
                ("SC_THREAD_STACK_MIN", libc::_SC_THREAD_STACK_MIN),
                ("SC_THREAD_THREADS_MAX", libc::_SC_THREAD_THREADS_MAX),
                ("SC_NGROUPS_MAX", libc::_SC_NGROUPS_MAX),
                ("SC_RE_DUP_MAX", libc::_SC_RE_DUP_MAX),
                ("SC_LINE_MAX", libc::_SC_LINE_MAX),
                ("SC_STREAM_MAX", libc::_SC_STREAM_MAX),
                ("SC_TZNAME_MAX", libc::_SC_TZNAME_MAX),
                ("SC_BC_BASE_MAX", libc::_SC_BC_BASE_MAX),
                ("SC_BC_DIM_MAX", libc::_SC_BC_DIM_MAX),
                ("SC_BC_SCALE_MAX", libc::_SC_BC_SCALE_MAX),
                ("SC_BC_STRING_MAX", libc::_SC_BC_STRING_MAX),
                ("SC_EXPR_NEST_MAX", libc::_SC_EXPR_NEST_MAX),
            ];
            alloc_sysconf_names(_py, names.iter().map(|&(name, key)| (name, i64::from(key))))
        }
        #[cfg(all(not(unix), not(target_arch = "wasm32")))]
        {
            MoltObject::none().bits()
        }
    })
}

// ---------------------------------------------------------------------------
// Filesystem operations (Phase 2)
// ---------------------------------------------------------------------------

/// `os.path.realpath(path)` → str or bytes, matching the path flavor — resolve path following symlinks
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_path_realpath(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.path.realpath", AuditArg::None) {
            return error;
        }
        let (path, bytes_flavor) = match path_from_bits_with_flavor(_py, path_bits) {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match std::fs::canonicalize(&path) {
            Ok(resolved) => path_result(_py, &resolved, bytes_flavor),
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
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.utime", AuditArg::None) {
            return error;
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
        #[cfg(unix)]
        {
            use std::ffi::CString;

            let c_path = match CString::new(path.as_os_str().as_encoded_bytes()) {
                Ok(c) => c,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::ValueError,
                        "path contains null byte",
                    );
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
                            BuiltinExc::TypeError,
                            "utime: atime must be a number",
                        );
                    }
                };
                let mtime_f = match to_f64(obj_from_bits(mtime_bits)) {
                    Some(v) => v,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
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
        #[cfg(windows)]
        {
            let atime_obj = obj_from_bits(atime_bits);
            let (atime, mtime) = if atime_obj.is_none() {
                let now = match windows_filetime_now(_py) {
                    Ok(v) => v,
                    Err(bits) => return bits,
                };
                (now, now)
            } else {
                let atime_f = match to_f64(atime_obj) {
                    Some(v) => v,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
                            "utime: atime must be a number",
                        );
                    }
                };
                let mtime_f = match to_f64(obj_from_bits(mtime_bits)) {
                    Some(v) => v,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::TypeError,
                            "utime: mtime must be a number",
                        );
                    }
                };
                let atime = match windows_filetime_from_seconds(_py, atime_f, "atime") {
                    Ok(v) => v,
                    Err(bits) => return bits,
                };
                let mtime = match windows_filetime_from_seconds(_py, mtime_f, "mtime") {
                    Ok(v) => v,
                    Err(bits) => return bits,
                };
                (atime, mtime)
            };
            if let Err(err) = set_windows_file_times(&path, atime, mtime) {
                return raise_os_error::<u64>(_py, err, "utime");
            }
            MoltObject::none().bits()
        }
        #[cfg(all(not(unix), not(windows), not(target_arch = "wasm32")))]
        {
            let _ = (path, atime_bits, mtime_bits);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "utime")
        }
    })
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct WindowsFileTime {
    dw_low_date_time: u32,
    dw_high_date_time: u32,
}

#[cfg(windows)]
#[allow(non_snake_case)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetFileTime(
        hfile: *mut std::ffi::c_void,
        lpcreationtime: *const WindowsFileTime,
        lplastaccesstime: *const WindowsFileTime,
        lplastwritetime: *const WindowsFileTime,
    ) -> i32;
}

#[cfg(windows)]
fn windows_filetime_from_seconds(
    _py: &CoreGilToken,
    seconds: f64,
    label: &str,
) -> Result<WindowsFileTime, u64> {
    if !seconds.is_finite() {
        return Err(raise_exception::<u64>(
            _py,
            BuiltinExc::OverflowError,
            &format!("utime: {label} out of range"),
        ));
    }
    let sec_floor = seconds.floor();
    if sec_floor < i64::MIN as f64 || sec_floor > i64::MAX as f64 {
        return Err(raise_exception::<u64>(
            _py,
            BuiltinExc::OverflowError,
            &format!("utime: {label} out of range"),
        ));
    }
    let mut secs = sec_floor as i64;
    let mut nanos = ((seconds - sec_floor) * 1_000_000_000.0).round() as i64;
    if nanos >= 1_000_000_000 {
        secs = secs.checked_add(1).ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                BuiltinExc::OverflowError,
                &format!("utime: {label} out of range"),
            )
        })?;
        nanos -= 1_000_000_000;
    }
    if nanos < 0 {
        secs = secs.checked_sub(1).ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                BuiltinExc::OverflowError,
                &format!("utime: {label} out of range"),
            )
        })?;
        nanos += 1_000_000_000;
    }
    windows_filetime_from_unix_parts(secs, nanos as u32).ok_or_else(|| {
        raise_exception::<u64>(
            _py,
            BuiltinExc::OverflowError,
            &format!("utime: {label} out of range"),
        )
    })
}

#[cfg(windows)]
fn windows_filetime_now(_py: &CoreGilToken) -> Result<WindowsFileTime, u64> {
    let now = std::time::SystemTime::now();
    let (secs, nanos) = match now.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => (
            i64::try_from(duration.as_secs()).map_err(|_| {
                raise_exception::<u64>(
                    _py,
                    BuiltinExc::OverflowError,
                    "utime: current time out of range",
                )
            })?,
            duration.subsec_nanos(),
        ),
        Err(err) => {
            let duration = err.duration();
            let secs = i64::try_from(duration.as_secs()).map_err(|_| {
                raise_exception::<u64>(
                    _py,
                    BuiltinExc::OverflowError,
                    "utime: current time out of range",
                )
            })?;
            let nanos = duration.subsec_nanos();
            if nanos == 0 {
                (
                    secs.checked_neg().ok_or_else(|| {
                        raise_exception::<u64>(
                            _py,
                            BuiltinExc::OverflowError,
                            "utime: current time out of range",
                        )
                    })?,
                    0,
                )
            } else {
                (
                    secs.checked_neg()
                        .and_then(|v| v.checked_sub(1))
                        .ok_or_else(|| {
                            raise_exception::<u64>(
                                _py,
                                BuiltinExc::OverflowError,
                                "utime: current time out of range",
                            )
                        })?,
                    1_000_000_000 - nanos,
                )
            }
        }
    };
    windows_filetime_from_unix_parts(secs, nanos).ok_or_else(|| {
        raise_exception::<u64>(
            _py,
            BuiltinExc::OverflowError,
            "utime: current time out of range",
        )
    })
}

#[cfg(windows)]
fn windows_filetime_from_unix_parts(secs: i64, nanos: u32) -> Option<WindowsFileTime> {
    const WINDOWS_EPOCH_OFFSET_SECS: i128 = 11_644_473_600;
    const WINDOWS_TICKS_PER_SEC: i128 = 10_000_000;
    let intervals = (secs as i128)
        .checked_add(WINDOWS_EPOCH_OFFSET_SECS)?
        .checked_mul(WINDOWS_TICKS_PER_SEC)?
        .checked_add((nanos as i128) / 100)?;
    if !(0..=u64::MAX as i128).contains(&intervals) {
        return None;
    }
    let intervals = intervals as u64;
    Some(WindowsFileTime {
        dw_low_date_time: intervals as u32,
        dw_high_date_time: (intervals >> 32) as u32,
    })
}

#[cfg(windows)]
fn windows_filetime_from_sec_nsec_bits(
    _py: &CoreGilToken,
    sec_bits: u64,
    nsec_bits: u64,
    label: &str,
) -> Result<WindowsFileTime, u64> {
    let secs = to_i64(obj_from_bits(sec_bits)).ok_or_else(|| {
        raise_exception::<u64>(_py, BuiltinExc::TypeError, "utime: times must be integers")
    })?;
    let nanos = to_i64(obj_from_bits(nsec_bits)).ok_or_else(|| {
        raise_exception::<u64>(_py, BuiltinExc::TypeError, "utime: times must be integers")
    })?;
    if nanos == -1 {
        return windows_filetime_now(_py);
    }
    if !(0..1_000_000_000).contains(&nanos) {
        return Err(raise_exception::<u64>(
            _py,
            BuiltinExc::ValueError,
            &format!("utime: {label} nanosecond out of range"),
        ));
    }
    windows_filetime_from_unix_parts(secs, nanos as u32).ok_or_else(|| {
        raise_exception::<u64>(
            _py,
            BuiltinExc::OverflowError,
            &format!("utime: {label} out of range"),
        )
    })
}

#[cfg(windows)]
fn set_windows_file_times(
    path: &Path,
    atime: WindowsFileTime,
    mtime: WindowsFileTime,
) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let result = unsafe {
        SetFileTime(
            file.as_raw_handle().cast(),
            std::ptr::null(),
            &atime,
            &mtime,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// dir_fd-relative variants (`*at` POSIX family). `dir_fd=None` -> AT_FDCWD.
//
// `molt_os_lstat_at` / `molt_os_stat_at` / `molt_os_rename_at` /
// `molt_os_replace_at` / `molt_os_link_at` live in molt-runtime's
// `builtins/io_path.rs` (always compiled, alongside the stat-tuple builders);
// only the readlink/symlink/utime family — whose base variants live in this
// crate — is defined here.
//
// WASI preview-1 cannot mint general dir fds dynamically, so the wasm32 stubs
// raise OSError(ENOSYS); Windows is covered by the `#[cfg(not(unix))]` arm.
// ---------------------------------------------------------------------------

/// Convert a path to a NUL-terminated `CString` for the `*at` syscall family,
/// raising `ValueError` (CPython parity) on an interior NUL.
#[cfg(all(not(target_arch = "wasm32"), unix))]
#[inline]
fn at_path_to_cstring(_py: &CoreGilToken, path: &Path) -> Result<std::ffi::CString, u64> {
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
/// `None` -> platform `libc::AT_FDCWD`; non-integer -> `TypeError`.
#[cfg(all(not(target_arch = "wasm32"), unix))]
#[inline]
fn at_resolve_dir_fd(_py: &CoreGilToken, dir_fd_bits: u64) -> Result<libc::c_int, u64> {
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

/// `os.readlink(path, dir_fd=...)` → str or bytes, matching the path flavor.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_readlink_at(path_bits: u64, dir_fd_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let (path, bytes_flavor) = match path_from_bits_with_flavor(_py, path_bits) {
            Ok(path) => path,
            Err(error) => return error,
        };
        let directory = obj_from_bits(dir_fd_bits);
        let dir_fd = if directory.is_none() {
            None
        } else {
            match to_i64(directory).and_then(|value| libc::c_int::try_from(value).ok()) {
                Some(value) => Some(value),
                None => {
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::TypeError,
                        "an integer is required",
                    );
                }
            }
        };
        #[cfg(unix)]
        let default_directory = dir_fd.is_none() || dir_fd == Some(libc::AT_FDCWD);
        #[cfg(not(unix))]
        let default_directory = dir_fd.is_none();
        // POSIX absolute paths ignore a valid descriptor. Default-CWD paths
        // share the canonical mount selection and aggregate capability gate.
        if path.is_absolute() || default_directory {
            return path_read_link(_py, &path, bytes_flavor);
        }
        if let Err(error) = require_operation(_py, "os.readlink_at", AuditArg::None) {
            return error;
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let c_path = match at_path_to_cstring(_py, &path) {
                Ok(path) => path,
                Err(error) => return error,
            };
            let mut cap: usize = 256;
            loop {
                let mut buf: Vec<u8> = vec![0u8; cap];
                let rc = unsafe {
                    libc::readlinkat(
                        dir_fd.unwrap(),
                        c_path.as_ptr(),
                        buf.as_mut_ptr().cast(),
                        cap,
                    )
                };
                if rc < 0 {
                    return os_err_bits(_py, std::io::Error::last_os_error(), "readlink");
                }
                let n = rc as usize;
                if n < cap {
                    buf.truncate(n);
                    use std::os::unix::ffi::OsStringExt;
                    return path_result(
                        _py,
                        &PathBuf::from(std::ffi::OsString::from_vec(buf)),
                        bytes_flavor,
                    );
                }
                cap *= 2;
            }
        }
        #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
        {
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "readlink")
        }
    })
}

/// `os.symlink(src, dst, dir_fd=...)` → None — directory-relative symlink.
///
/// NOTE: `symlinkat(target, newdirfd, linkpath)` takes the new-directory fd as
/// its SECOND argument; `dir_fd` here qualifies the LINK (`dst`), matching
/// CPython.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_symlink_at(src_bits: u64, dst_bits: u64, dir_fd_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.symlink_at", AuditArg::None) {
            return error;
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
            let dir_fd = match at_resolve_dir_fd(_py, dir_fd_bits) {
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
            let rc = unsafe { libc::symlinkat(c_src.as_ptr(), dir_fd, c_dst.as_ptr()) };
            if rc < 0 {
                return os_err_bits(_py, std::io::Error::last_os_error(), "symlink");
            }
            MoltObject::none().bits()
        }
        #[cfg(not(unix))]
        {
            let _ = (src, dst, dir_fd_bits);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "symlink")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_symlink_at(_s: u64, _d: u64, _dir: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "symlink")
    })
}

/// `os.utime(path, dir_fd=..., follow_symlinks=...)` → None.
///
/// Times arrive pre-split into `(sec, nsec)` pairs by the Python wrapper (from
/// `times=`, `ns=`, or "current time"). Splitting on the Python side keeps every
/// argument within the inline-int range — a single epoch-scale nanosecond count
/// (~1.7e18) would exceed it. A `nsec` sentinel of `-1` selects POSIX
/// `UTIME_NOW` for that timestamp (the "no times given" path sends `-1` for
/// both).
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_utime_at(
    path_bits: u64,
    dir_fd_bits: u64,
    atime_sec_bits: u64,
    atime_nsec_bits: u64,
    mtime_sec_bits: u64,
    mtime_nsec_bits: u64,
    follow_symlinks_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.utime_at", AuditArg::None) {
            return error;
        }
        let path = match require_path(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            let dir_fd = match at_resolve_dir_fd(_py, dir_fd_bits) {
                Ok(fd) => fd,
                Err(bits) => return bits,
            };
            let ts = match at_timespec_pair(
                _py,
                atime_sec_bits,
                atime_nsec_bits,
                mtime_sec_bits,
                mtime_nsec_bits,
            ) {
                Ok(ts) => ts,
                Err(bits) => return bits,
            };
            let follow = is_truthy(_py, obj_from_bits(follow_symlinks_bits));
            let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
            let c_path = match at_path_to_cstring(_py, &path) {
                Ok(c) => c,
                Err(bits) => return bits,
            };
            let rc = unsafe { libc::utimensat(dir_fd, c_path.as_ptr(), ts.as_ptr(), flags) };
            if rc < 0 {
                return os_err_bits(_py, std::io::Error::last_os_error(), "utime");
            }
            MoltObject::none().bits()
        }
        #[cfg(windows)]
        {
            if !obj_from_bits(dir_fd_bits).is_none()
                || !is_truthy(_py, obj_from_bits(follow_symlinks_bits))
            {
                return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "utime");
            }
            let atime = match windows_filetime_from_sec_nsec_bits(
                _py,
                atime_sec_bits,
                atime_nsec_bits,
                "atime",
            ) {
                Ok(v) => v,
                Err(bits) => return bits,
            };
            let mtime = match windows_filetime_from_sec_nsec_bits(
                _py,
                mtime_sec_bits,
                mtime_nsec_bits,
                "mtime",
            ) {
                Ok(v) => v,
                Err(bits) => return bits,
            };
            if let Err(err) = set_windows_file_times(&path, atime, mtime) {
                return os_err_bits(_py, err, "utime");
            }
            MoltObject::none().bits()
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let _ = (
                path,
                dir_fd_bits,
                atime_sec_bits,
                atime_nsec_bits,
                mtime_sec_bits,
                mtime_nsec_bits,
                follow_symlinks_bits,
            );
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "utime")
        }
    })
}

/// Build the `[atime, mtime]` `timespec` pair for `utimensat` from four
/// NaN-boxed `(sec, nsec)` ints. A `nsec` of `-1` maps to `UTIME_NOW`. Any
/// non-integer raises `TypeError` (error bits carry the recorded exception).
#[cfg(all(not(target_arch = "wasm32"), unix))]
#[inline]
fn at_timespec_pair(
    _py: &CoreGilToken,
    atime_sec_bits: u64,
    atime_nsec_bits: u64,
    mtime_sec_bits: u64,
    mtime_nsec_bits: u64,
) -> Result<[libc::timespec; 2], u64> {
    let one = |bits: u64| -> Result<i64, u64> {
        to_i64(obj_from_bits(bits)).ok_or_else(|| {
            raise_exception::<u64>(_py, BuiltinExc::TypeError, "utime: times must be integers")
        })
    };
    let mk = |sec: i64, nsec: i64| -> libc::timespec {
        if nsec == -1 {
            libc::timespec {
                tv_sec: sec as libc::time_t,
                tv_nsec: libc::UTIME_NOW,
            }
        } else {
            libc::timespec {
                tv_sec: sec as libc::time_t,
                tv_nsec: nsec as _,
            }
        }
    };
    let atime = mk(one(atime_sec_bits)?, one(atime_nsec_bits)?);
    let mtime = mk(one(mtime_sec_bits)?, one(mtime_nsec_bits)?);
    Ok([atime, mtime])
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_utime_at(
    _p: u64,
    _d: u64,
    _as: u64,
    _an: u64,
    _ms: u64,
    _mn: u64,
    _f: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "utime")
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
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.sendfile", AuditArg::None) {
            return error;
        }
        let Some(out_fd) = to_i64(obj_from_bits(out_fd_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "out_fd must be an integer");
        };
        let Some(in_fd) = to_i64(obj_from_bits(in_fd_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "in_fd must be an integer");
        };
        let Some(count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "count must be an integer");
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
                        BuiltinExc::TypeError,
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

/// The target's `<sys/wait.h>` status layout. POSIX targets use the platform
/// macros (Linux and Darwin differ for WIFSTOPPED); other targets decode the
/// Linux layout.
mod wait_status {
    #[cfg(unix)]
    pub(super) fn exited(status: i32) -> bool {
        libc::WIFEXITED(status)
    }
    #[cfg(unix)]
    pub(super) fn exit_status(status: i32) -> i32 {
        libc::WEXITSTATUS(status)
    }
    #[cfg(unix)]
    pub(super) fn signaled(status: i32) -> bool {
        libc::WIFSIGNALED(status)
    }
    #[cfg(unix)]
    pub(super) fn term_sig(status: i32) -> i32 {
        libc::WTERMSIG(status)
    }
    #[cfg(unix)]
    pub(super) fn stopped(status: i32) -> bool {
        libc::WIFSTOPPED(status)
    }
    #[cfg(unix)]
    pub(super) fn stop_sig(status: i32) -> i32 {
        libc::WSTOPSIG(status)
    }

    #[cfg(not(unix))]
    pub(super) fn exited(status: i32) -> bool {
        status & 0x7f == 0
    }
    #[cfg(not(unix))]
    pub(super) fn exit_status(status: i32) -> i32 {
        (status >> 8) & 0xff
    }
    #[cfg(not(unix))]
    pub(super) fn signaled(status: i32) -> bool {
        // glibc: ((signed char) ((status & 0x7f) + 1) >> 1) > 0
        ((((status & 0x7f) + 1) as i8) >> 1) > 0
    }
    #[cfg(not(unix))]
    pub(super) fn term_sig(status: i32) -> i32 {
        status & 0x7f
    }
    #[cfg(not(unix))]
    pub(super) fn stopped(status: i32) -> bool {
        status & 0xff == 0x7f
    }
    #[cfg(not(unix))]
    pub(super) fn stop_sig(status: i32) -> i32 {
        (status >> 8) & 0xff
    }
}

/// Shared `status: int` conversion for the wait-status macros.
fn wait_status_macro(status_bits: u64, project: fn(i32) -> MoltObject) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        match c_int_arg(_py, status_bits) {
            Ok(status) => project(status).bits(),
            Err(bits) => bits,
        }
    })
}

/// `os.WIFEXITED(status)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wifexited(status_bits: u64) -> u64 {
    wait_status_macro(status_bits, |status| {
        MoltObject::from_bool(wait_status::exited(status))
    })
}

/// `os.WEXITSTATUS(status)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wexitstatus(status_bits: u64) -> u64 {
    wait_status_macro(status_bits, |status| {
        MoltObject::from_int(i64::from(wait_status::exit_status(status)))
    })
}

/// `os.WIFSIGNALED(status)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wifsignaled(status_bits: u64) -> u64 {
    wait_status_macro(status_bits, |status| {
        MoltObject::from_bool(wait_status::signaled(status))
    })
}

/// `os.WTERMSIG(status)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wtermsig(status_bits: u64) -> u64 {
    wait_status_macro(status_bits, |status| {
        MoltObject::from_int(i64::from(wait_status::term_sig(status)))
    })
}

/// `os.WIFSTOPPED(status)` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wifstopped(status_bits: u64) -> u64 {
    wait_status_macro(status_bits, |status| {
        MoltObject::from_bool(wait_status::stopped(status))
    })
}

/// `os.WSTOPSIG(status)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_wstopsig(status_bits: u64) -> u64 {
    wait_status_macro(status_bits, |status| {
        MoltObject::from_int(i64::from(wait_status::stop_sig(status)))
    })
}

/// os.fspath shares the canonical runtime protocol and preserves result type.
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_fspath(path_bits: u64) -> u64 {
    crate::bridge::fspath_bits(path_bits)
}

// ---------------------------------------------------------------------------
// 9. Tier-0 gaps for click / trio / httpx support
// ---------------------------------------------------------------------------

/// `os.path.split` over a path's encoded bytes: posixpath when `sep` is `/`,
/// ntpath (either separator, after the drive and root) when it is `\\`.
/// Both halves end on ASCII separator boundaries of the input.
fn split_path_bytes(path: &[u8], sep: u8) -> (&[u8], &[u8]) {
    if sep == b'/' {
        // posixpath.split keeps a head made only of separators verbatim.
        let i = path.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
        let (head, tail) = path.split_at(i);
        if head.iter().all(|&b| b == b'/') {
            return (head, tail);
        }
        let end = head.iter().rposition(|&b| b != b'/').map_or(0, |i| i + 1);
        return (&head[..end], tail);
    }
    use molt_runtime_platform::path_text::{PathCoordinate, path_root_offsets};
    let is_sep = |b: u8| b == b'\\' || b == b'/';
    let (_, root_end) = path_root_offsets(path, b'\\', PathCoordinate::PythonCodePoints);
    let tail_start = path[root_end..]
        .iter()
        .rposition(|&b| is_sep(b))
        .map_or(root_end, |i| root_end + i + 1);
    let mut head_end = tail_start;
    while head_end > root_end && is_sep(path[head_end - 1]) {
        head_end -= 1;
    }
    (&path[..head_end], &path[tail_start..])
}

fn split_path(path: &Path) -> (PathBuf, std::ffi::OsString) {
    let sep = if cfg!(windows) { b'\\' } else { b'/' };
    let (head, tail) = split_path_bytes(path.as_os_str().as_encoded_bytes(), sep);
    // SAFETY: both halves are slices of this OsStr's own encoded bytes split at
    // ASCII separators, which `from_encoded_bytes_unchecked` permits.
    unsafe {
        (
            PathBuf::from(std::ffi::OsString::from_encoded_bytes_unchecked(
                head.to_vec(),
            )),
            std::ffi::OsString::from_encoded_bytes_unchecked(tail.to_vec()),
        )
    }
}

enum MakedirsFailure {
    Os(std::io::Error),
    /// Leaf `mode` conversion raised; the exception is pending.
    Raised(u64),
}

/// CPython's `os.makedirs` algorithm (Lib/os.py). Missing ancestors are made
/// first with the default mode and the same `exist_ok`; an ancestor that a
/// racing creator made (`FileExistsError`) is accepted; a trailing `curdir`
/// component ends after its parent. Only the leaf receives `mode`, and the
/// leaf's `mkdir(name, mode)` converts it, so a bad mode raises after the
/// ancestors exist, exactly as in CPython. `leaf_mode_bits` is None for
/// ancestors.
fn makedirs_tree(
    _py: &CoreGilToken,
    name: &Path,
    leaf_mode_bits: Option<u64>,
    exist_ok: bool,
) -> Result<(), MakedirsFailure> {
    let (mut head, mut tail) = split_path(name);
    if tail.is_empty() {
        (head, tail) = split_path(&head);
    }
    if !head.as_os_str().is_empty() && !tail.is_empty() && !head.exists() {
        match makedirs_tree(_py, &head, None, exist_ok) {
            Ok(()) => {}
            Err(MakedirsFailure::Os(err)) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(failure) => return Err(failure),
        }
        if tail == "." {
            return Ok(());
        }
    }
    let mode = match leaf_mode_bits {
        Some(bits) => c_int_arg(_py, bits).map_err(MakedirsFailure::Raised)?,
        None => 0o777,
    };
    match mkdir_with_mode(name, mode) {
        Ok(()) => Ok(()),
        // CPython cannot rely on EEXIST (EACCES or EROFS may take priority),
        // so any failure is accepted when exist_ok and the leaf is a directory.
        Err(_) if exist_ok && name.is_dir() => Ok(()),
        Err(err) => Err(MakedirsFailure::Os(err)),
    }
}

/// `os.makedirs(name, mode=0o777, exist_ok=False)` → None
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_makedirs(path_bits: u64, mode_bits: u64, exist_ok_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "os.makedirs", AuditArg::None) {
            return error;
        }
        let path = match require_path(_py, path_bits, "name") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let exist_ok = is_truthy(_py, obj_from_bits(exist_ok_bits));
        match makedirs_tree(_py, &path, Some(mode_bits), exist_ok) {
            Ok(()) => MoltObject::none().bits(),
            Err(MakedirsFailure::Os(err)) => os_err_bits(_py, err, "makedirs"),
            Err(MakedirsFailure::Raised(bits)) => bits,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::split_path_bytes;

    type SplitCase = (&'static [u8], &'static [u8], &'static [u8]);

    fn assert_split(sep: u8, cases: &[SplitCase]) {
        for &(path, head, tail) in cases {
            assert_eq!(
                split_path_bytes(path, sep),
                (head, tail),
                "split of {:?}",
                String::from_utf8_lossy(path)
            );
        }
    }

    /// Expected halves are CPython `posixpath.split` results.
    #[test]
    fn split_path_bytes_matches_posixpath() {
        assert_split(
            b'/',
            &[
                (b"", b"", b""),
                (b"a", b"", b"a"),
                (b"/", b"/", b""),
                (b"//", b"//", b""),
                (b"///", b"///", b""),
                (b"a/", b"a", b""),
                (b"a//b", b"a", b"b"),
                (b"/a/", b"/a", b""),
                (b"//a", b"//", b"a"),
                (b"///a", b"///", b"a"),
                (b"a/b/.", b"a/b", b"."),
                (b"a/b/..", b"a/b", b".."),
                (b"/a/b/c", b"/a/b", b"c"),
                (b"x/\xff/y", b"x/\xff", b"y"),
            ],
        );
    }

    /// Expected halves are CPython `ntpath.split` results.
    #[test]
    fn split_path_bytes_matches_ntpath() {
        assert_split(
            b'\\',
            &[
                (b"", b"", b""),
                (b"a", b"", b"a"),
                (b"C:", b"C:", b""),
                (b"C:\\", b"C:\\", b""),
                (b"C:/", b"C:/", b""),
                (b"C:a", b"C:", b"a"),
                (b"C:\\a", b"C:\\", b"a"),
                (b"C:\\\\a", b"C:\\", b"a"),
                (b"C:/a/b", b"C:/a", b"b"),
                (b"a/b\\c", b"a/b", b"c"),
                (b"\\\\server\\share", b"\\\\server\\share", b""),
                (b"\\\\server\\share\\", b"\\\\server\\share\\", b""),
                (b"\\\\server\\share\\a\\b", b"\\\\server\\share\\a", b"b"),
                (b"\\a", b"\\", b"a"),
                (b"a\\\\", b"a", b""),
                (b"\\\\?\\UNC\\srv\\shr\\x", b"\\\\?\\UNC\\srv\\shr\\", b"x"),
                (b"a\\b\\.", b"a\\b", b"."),
            ],
        );
    }
}
