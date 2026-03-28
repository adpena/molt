#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/shutil.rs ===
//
// shutil intrinsics: high-level file and directory operations.
//
// Existing coverage: molt_shutil_which, molt_shutil_copyfile (in functions.rs).
// This file adds: copy, copy2, copytree, rmtree, move, disk_usage,
//                 get_terminal_size, make_archive, unpack_archive, chown.
//
// Capability gates:
//   copy / copy2 / copytree / move: fs.read + fs.write
//   rmtree:                         fs.write
//   disk_usage / get_terminal_size: fs.read (no writes)
//   make_archive / unpack_archive:  fs.read + fs.write + process (tar formats)
//   chown:                          fs.write  (Unix only)

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::*;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Internal helpers (local scope)
// ---------------------------------------------------------------------------

#[inline]
fn shutil_err(_py: &PyToken<'_>, err: std::io::Error, ctx: &str) -> u64 {
    raise_os_error::<u64>(_py, err, ctx)
}

#[inline]
fn require_path_local(_py: &PyToken<'_>, bits: u64, _label: &str) -> Result<PathBuf, u64> {
    match path_from_bits(_py, bits) {
        Ok(p) => Ok(p),
        Err(msg) => Err(raise_exception::<u64>(_py, "TypeError", &msg)),
    }
}

#[inline]
fn str_bits_local(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// Recursive copy helpers (pure Rust; no GIL calls inside)
// ---------------------------------------------------------------------------

fn copy_file_data(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::copy(src, dst).map(|_| ())
}

fn copy_metadata(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = fs::metadata(src)?;
    let perms = meta.permissions();
    fs::set_permissions(dst, perms)?;
    // Timestamps: best-effort via std::fs; errors are silently ignored per
    // CPython's shutil.copy2 semantics.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let atime = libc::timespec {
            tv_sec: meta.atime(),
            tv_nsec: meta.atime_nsec(),
        };
        let mtime = libc::timespec {
            tv_sec: meta.mtime(),
            tv_nsec: meta.mtime_nsec(),
        };
        use std::os::unix::ffi::OsStrExt;
        let c_dst = std::ffi::CString::new(dst.as_os_str().as_bytes()).unwrap_or_default();
        unsafe {
            libc::utimensat(libc::AT_FDCWD, c_dst.as_ptr(), [atime, mtime].as_ptr(), 0);
        }
    }
    Ok(())
}

fn copytree_recursive(src: &Path, dst: &Path, dirs_exist_ok: bool) -> std::io::Result<()> {
    if dst.exists() {
        if !dirs_exist_ok {
            return Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                format!("destination '{}' already exists", dst.display()),
            ));
        }
    } else {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copytree_recursive(&src_path, &dst_path, dirs_exist_ok)?;
        } else {
            copy_file_data(&src_path, &dst_path)?;
            // copy2 style: copy permissions and timestamps.
            let _ = copy_metadata(&src_path, &dst_path);
        }
    }
    Ok(())
}

fn rmtree_recursive(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return fs::remove_file(path);
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            rmtree_recursive(&entry.path())?;
        }
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

/// `shutil.copy(src, dst)` → str
/// Copies file data and permissions but not metadata timestamps.
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_copy(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") || !has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let src = match require_path_local(_py, src_bits, "src") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let mut dst = match require_path_local(_py, dst_bits, "dst") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        // If dst is an existing directory, place the file inside it.
        if dst.is_dir()
            && let Some(name) = src.file_name()
        {
            dst = dst.join(name);
        }
        if let Err(err) = copy_file_data(&src, &dst) {
            return shutil_err(_py, err, "copy");
        }
        // Copy permissions (ignore errors per CPython behaviour).
        if let Ok(meta) = fs::metadata(&src) {
            let _ = fs::set_permissions(&dst, meta.permissions());
        }
        str_bits_local(_py, &dst.to_string_lossy())
    })
}

/// `shutil.copy2(src, dst)` → str
/// Copies file data, permissions, and timestamps.
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_copy2(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") || !has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let src = match require_path_local(_py, src_bits, "src") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let mut dst = match require_path_local(_py, dst_bits, "dst") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        if dst.is_dir()
            && let Some(name) = src.file_name()
        {
            dst = dst.join(name);
        }
        if let Err(err) = copy_file_data(&src, &dst) {
            return shutil_err(_py, err, "copy2");
        }
        let _ = copy_metadata(&src, &dst);
        str_bits_local(_py, &dst.to_string_lossy())
    })
}

/// `shutil.copytree(src, dst, dirs_exist_ok=False)` → str
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_copytree(
    src_bits: u64,
    dst_bits: u64,
    dirs_exist_ok_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") || !has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let src = match require_path_local(_py, src_bits, "src") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let dst = match require_path_local(_py, dst_bits, "dst") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let dirs_exist_ok = is_truthy(_py, obj_from_bits(dirs_exist_ok_bits));
        match copytree_recursive(&src, &dst, dirs_exist_ok) {
            Ok(()) => str_bits_local(_py, &dst.to_string_lossy()),
            Err(err) => shutil_err(_py, err, "copytree"),
        }
    })
}

/// `shutil.rmtree(path)` → None
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_rmtree(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path_local(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        match rmtree_recursive(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => shutil_err(_py, err, "rmtree"),
        }
    })
}

/// `shutil.move(src, dst)` → str
/// Attempts rename first; falls back to copy+delete for cross-device moves.
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_move(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") || !has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let src = match require_path_local(_py, src_bits, "src") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        let mut dst = match require_path_local(_py, dst_bits, "dst") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        // If dst is an existing directory, move src into it.
        if dst.is_dir()
            && let Some(name) = src.file_name()
        {
            dst = dst.join(name);
        }
        // Try atomic rename first.
        if fs::rename(&src, &dst).is_ok() {
            return str_bits_local(_py, &dst.to_string_lossy());
        }
        // Cross-device move: copy then delete.
        if src.is_dir() {
            if let Err(err) = copytree_recursive(&src, &dst, false) {
                return shutil_err(_py, err, "move");
            }
            if let Err(err) = rmtree_recursive(&src) {
                return shutil_err(_py, err, "move");
            }
        } else {
            if let Err(err) = copy_file_data(&src, &dst) {
                return shutil_err(_py, err, "move");
            }
            let _ = copy_metadata(&src, &dst);
            if let Err(err) = fs::remove_file(&src) {
                return shutil_err(_py, err, "move");
            }
        }
        str_bits_local(_py, &dst.to_string_lossy())
    })
}

/// `shutil.disk_usage(path)` → tuple[int, int, int]  (total, used, free)
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_disk_usage(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path_local(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let c_path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
                Ok(c) => c,
                Err(_) => {
                    return raise_exception::<_>(_py, "ValueError", "invalid path");
                }
            };
            let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                return shutil_err(_py, err, "disk_usage");
            }
            let bsize = stat.f_frsize;
            let total = stat.f_blocks as u64 * bsize;
            let free = stat.f_bavail as u64 * bsize;
            let used = total.saturating_sub(stat.f_bfree as u64 * bsize);
            let elems = [
                MoltObject::from_int(total as i64).bits(),
                MoltObject::from_int(used as i64).bits(),
                MoltObject::from_int(free as i64).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::MAX_PATH;
            use windows_sys::Win32::Storage::FileSystem::{
                GetDiskFreeSpaceExW, GetVolumePathNameW,
            };

            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut vol_path = vec![0u16; MAX_PATH as usize + 1];
            let ok = unsafe {
                GetVolumePathNameW(wide.as_ptr(), vol_path.as_mut_ptr(), vol_path.len() as u32)
            };
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                return shutil_err(_py, err, "disk_usage");
            }
            let mut free_bytes_caller: u64 = 0;
            let mut total_bytes: u64 = 0;
            let mut total_free: u64 = 0;
            let ok2 = unsafe {
                GetDiskFreeSpaceExW(
                    vol_path.as_ptr(),
                    &mut free_bytes_caller,
                    &mut total_bytes,
                    &mut total_free,
                )
            };
            if ok2 == 0 {
                let err = std::io::Error::last_os_error();
                return shutil_err(_py, err, "disk_usage");
            }
            let used = total_bytes.saturating_sub(total_free);
            let elems = [
                MoltObject::from_int(total_bytes as i64).bits(),
                MoltObject::from_int(used as i64).bits(),
                MoltObject::from_int(free_bytes_caller as i64).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "disk_usage")
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_disk_usage(_path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "disk_usage")
    })
}

/// `shutil.get_terminal_size(fallback=(80, 24))` → tuple[int, int]  (columns, lines)
///
/// `fallback_bits` is a tuple[int, int] or None (default 80×24).
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_get_terminal_size(fallback_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (fb_cols, fb_lines) = if obj_from_bits(fallback_bits).is_none() {
            (80i64, 24i64)
        } else if let Some(ptr) = obj_from_bits(fallback_bits).as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                let elems = unsafe { seq_vec_ref(ptr) };
                if elems.len() >= 2 {
                    let c = to_i64(obj_from_bits(elems[0])).unwrap_or(80);
                    let l = to_i64(obj_from_bits(elems[1])).unwrap_or(24);
                    (c, l)
                } else {
                    (80, 24)
                }
            } else {
                (80, 24)
            }
        } else {
            (80, 24)
        };

        // Try COLUMNS / LINES env vars first (CPython semantics).
        let cols = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(-1);
        let lines = std::env::var("LINES")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(-1);
        if cols > 0 && lines > 0 {
            let elems = [
                MoltObject::from_int(cols).bits(),
                MoltObject::from_int(lines).bits(),
            ];
            let ptr = alloc_tuple(_py, &elems);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(ptr).bits();
        }

        // Try ioctl on stdout (fd=1) then stdin (fd=0).
        #[cfg(unix)]
        {
            for fd in [1i32, 0i32] {
                let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
                if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0
                    && ws.ws_col > 0
                    && ws.ws_row > 0
                {
                    let final_cols = if cols > 0 { cols } else { ws.ws_col as i64 };
                    let final_lines = if lines > 0 { lines } else { ws.ws_row as i64 };
                    let elems = [
                        MoltObject::from_int(final_cols).bits(),
                        MoltObject::from_int(final_lines).bits(),
                    ];
                    let ptr = alloc_tuple(_py, &elems);
                    if ptr.is_null() {
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
            }
        }

        // Fall back to the provided fallback tuple.
        let final_cols = if cols > 0 { cols } else { fb_cols };
        let final_lines = if lines > 0 { lines } else { fb_lines };
        let elems = [
            MoltObject::from_int(final_cols).bits(),
            MoltObject::from_int(final_lines).bits(),
        ];
        let ptr = alloc_tuple(_py, &elems);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_get_terminal_size(fallback_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // On WASM return the fallback directly.
        let (fb_cols, fb_lines) = if obj_from_bits(fallback_bits).is_none() {
            (80i64, 24i64)
        } else if let Some(ptr) = obj_from_bits(fallback_bits).as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                let elems = unsafe { seq_vec_ref(ptr) };
                if elems.len() >= 2 {
                    let c = to_i64(obj_from_bits(elems[0])).unwrap_or(80);
                    let l = to_i64(obj_from_bits(elems[1])).unwrap_or(24);
                    (c, l)
                } else {
                    (80, 24)
                }
            } else {
                (80, 24)
            }
        } else {
            (80, 24)
        };
        let elems = [
            MoltObject::from_int(fb_cols).bits(),
            MoltObject::from_int(fb_lines).bits(),
        ];
        let ptr = alloc_tuple(_py, &elems);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `shutil.make_archive(base_name, format, root_dir=None)` → str
///
/// Supported formats: "zip", "tar", "gztar", "bztar", "xztar".
/// Returns the path of the created archive.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_make_archive(
    base_name_bits: u64,
    format_bits: u64,
    root_dir_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") || !has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let base_name = match string_obj_to_owned(obj_from_bits(base_name_bits)) {
            Some(s) => s,
            None => return raise_exception::<_>(_py, "TypeError", "base_name must be str"),
        };
        let format = match string_obj_to_owned(obj_from_bits(format_bits)) {
            Some(s) => s,
            None => return raise_exception::<_>(_py, "TypeError", "format must be str"),
        };
        let root_dir = if obj_from_bits(root_dir_bits).is_none() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            match path_from_bits(_py, root_dir_bits) {
                Ok(p) => p,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            }
        };

        let ext = match format.as_str() {
            "zip" => ".zip",
            "tar" => ".tar",
            "gztar" => ".tar.gz",
            "bztar" => ".tar.bz2",
            "xztar" => ".tar.xz",
            other => {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    &format!("unknown archive format '{other}'"),
                );
            }
        };
        let archive_path = format!("{base_name}{ext}");

        match format.as_str() {
            #[cfg(feature = "stdlib_archive")]
            "zip" => {
                use std::io::Write;
                let file = match fs::File::create(&archive_path) {
                    Ok(f) => f,
                    Err(err) => return shutil_err(_py, err, "make_archive"),
                };
                let mut zip = zip::ZipWriter::new(file);
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);
                if let Err(err) = zip_add_directory(&mut zip, &root_dir, &root_dir, options) {
                    return shutil_err(_py, err, "make_archive");
                }
                if let Err(err) = zip.finish() {
                    let io_err = std::io::Error::other(err.to_string());
                    return shutil_err(_py, io_err, "make_archive");
                }
            }
            "tar" | "gztar" | "bztar" | "xztar" => {
                // tar requires spawning a subprocess — gate on "process" capability.
                let allowed = has_capability(_py, "process");
                audit_capability_decision(
                    "shutil.make_archive",
                    "process",
                    AuditArgs::None,
                    allowed,
                );
                if !allowed {
                    return raise_exception::<u64>(
                        _py,
                        "PermissionError",
                        "missing process capability for archive operations",
                    );
                }
                // Use std::process to invoke tar(1) which is universally available.
                let compression_flag = match format.as_str() {
                    "gztar" => Some("-z"),
                    "bztar" => Some("-j"),
                    "xztar" => Some("-J"),
                    _ => None,
                };
                let mut cmd = std::process::Command::new("tar");
                cmd.arg("-c");
                if let Some(flag) = compression_flag {
                    cmd.arg(flag);
                }
                cmd.arg("-f").arg(&archive_path);
                cmd.arg("-C").arg(&root_dir);
                cmd.arg(".");
                match cmd.status() {
                    Ok(status) if status.success() => {}
                    Ok(_) => {
                        return raise_exception::<_>(_py, "OSError", "tar command failed");
                    }
                    Err(err) => return shutil_err(_py, err, "make_archive"),
                }
            }
            _ => unreachable!(),
        }

        str_bits_local(_py, &archive_path)
    })
}

#[cfg(all(not(target_arch = "wasm32"), feature = "stdlib_archive"))]
fn zip_add_directory<W: std::io::Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    base: &Path,
    dir: &Path,
    options: zip::write::SimpleFileOptions,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(base).unwrap_or(&path);
        let name = relative.to_string_lossy();
        if path.is_dir() {
            zip.add_directory(format!("{}/", name), options)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            zip_add_directory(zip, base, &path, options)?;
        } else {
            zip.start_file(name.as_ref(), options)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let data = fs::read(&path)?;
            use std::io::Write;
            zip.write_all(&data)?;
        }
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_make_archive(
    _base_name_bits: u64,
    _format_bits: u64,
    _root_dir_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "make_archive")
    })
}

/// `shutil.unpack_archive(filename, extract_dir=None)` → None
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_unpack_archive(filename_bits: u64, extract_dir_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") || !has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let filename = match path_from_bits(_py, filename_bits) {
            Ok(p) => p,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let extract_dir = if obj_from_bits(extract_dir_bits).is_none() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            match path_from_bits(_py, extract_dir_bits) {
                Ok(p) => p,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            }
        };

        let name_lower = filename.to_string_lossy().to_lowercase();
        #[cfg(feature = "stdlib_archive")]
        if name_lower.ends_with(".zip") {
            match unpack_zip(&filename, &extract_dir) {
                Ok(()) => return MoltObject::none().bits(),
                Err(err) => return shutil_err(_py, err, "unpack_archive"),
            }
        }
        // Use tar(1) for all tar variants.
        if name_lower.ends_with(".tar")
            || name_lower.ends_with(".tar.gz")
            || name_lower.ends_with(".tgz")
            || name_lower.ends_with(".tar.bz2")
            || name_lower.ends_with(".tbz2")
            || name_lower.ends_with(".tar.xz")
            || name_lower.ends_with(".txz")
        {
            // tar requires spawning a subprocess — gate on "process" capability.
            let allowed = has_capability(_py, "process");
            audit_capability_decision("shutil.unpack_archive", "process", AuditArgs::None, allowed);
            if !allowed {
                return raise_exception::<u64>(
                    _py,
                    "PermissionError",
                    "missing process capability for archive operations",
                );
            }
            let mut cmd = std::process::Command::new("tar");
            cmd.arg("-xf").arg(&filename);
            cmd.arg("-C").arg(&extract_dir);
            match cmd.status() {
                Ok(status) if status.success() => return MoltObject::none().bits(),
                Ok(_) => {
                    return raise_exception::<_>(_py, "OSError", "tar extraction failed");
                }
                Err(err) => return shutil_err(_py, err, "unpack_archive"),
            }
        }
        raise_exception::<_>(_py, "ValueError", "unknown archive format")
    })
}

#[cfg(all(not(target_arch = "wasm32"), feature = "stdlib_archive"))]
fn unpack_zip(archive: &Path, dest: &Path) -> std::io::Result<()> {
    let file = fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e.to_string()))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e.to_string()))?;
        let out_path = dest.join(
            entry
                .enclosed_name()
                .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidData, "invalid path"))?,
        );
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out_file = fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
        }
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_unpack_archive(_filename_bits: u64, _extract_dir_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "unpack_archive")
    })
}

/// `shutil.chown(path, user=None, group=None)` → None  (Unix only)
///
/// `user_bits` / `group_bits` can be str (name) or int (uid/gid) or None.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_chown(path_bits: u64, user_bits: u64, group_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match require_path_local(_py, path_bits, "path") {
            Ok(p) => p,
            Err(bits) => return bits,
        };
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            let uid: libc::uid_t = if obj_from_bits(user_bits).is_none() {
                u32::MAX // signals "don't change"
            } else if let Some(n) = to_i64(obj_from_bits(user_bits)) {
                n as libc::uid_t
            } else if let Some(name) = string_obj_to_owned(obj_from_bits(user_bits)) {
                match lookup_uid(&name) {
                    Some(uid) => uid,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "LookupError",
                            &format!("no such user: '{name}'"),
                        );
                    }
                }
            } else {
                return raise_exception::<_>(_py, "TypeError", "user must be str, int, or None");
            };

            let gid: libc::gid_t = if obj_from_bits(group_bits).is_none() {
                u32::MAX
            } else if let Some(n) = to_i64(obj_from_bits(group_bits)) {
                n as libc::gid_t
            } else if let Some(name) = string_obj_to_owned(obj_from_bits(group_bits)) {
                match lookup_gid(&name) {
                    Some(gid) => gid,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "LookupError",
                            &format!("no such group: '{name}'"),
                        );
                    }
                }
            } else {
                return raise_exception::<_>(_py, "TypeError", "group must be str, int, or None");
            };

            let c_path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
                Ok(c) => c,
                Err(_) => return raise_exception::<_>(_py, "ValueError", "invalid path"),
            };
            let rc = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                return shutil_err(_py, err, "chown");
            }
            MoltObject::none().bits()
        }
        #[cfg(not(unix))]
        {
            let _ = (path, user_bits, group_bits);
            raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "chown")
        }
    })
}

#[cfg(unix)]
fn lookup_uid(name: &str) -> Option<libc::uid_t> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let pw = unsafe { libc::getpwnam(c_name.as_ptr()) };
    if pw.is_null() {
        return None;
    }
    Some(unsafe { (*pw).pw_uid })
}

#[cfg(unix)]
fn lookup_gid(name: &str) -> Option<libc::gid_t> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let gr = unsafe { libc::getgrnam(c_name.as_ptr()) };
    if gr.is_null() {
        return None;
    }
    Some(unsafe { (*gr).gr_gid })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_chown(_path_bits: u64, _user_bits: u64, _group_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "chown")
    })
}
