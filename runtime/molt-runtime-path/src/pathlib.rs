// === FILE: runtime/molt-runtime/src/builtins/pathlib.rs ===
//
// pathlib intrinsics: PurePath / Path operations delegated to Rust std::path.
//
// Native projections share one root parser. The compiled Python facade owns
// version-selected callback protocols; exact paths stay in these native paths.

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

fn list_of_paths(py: &CoreGilToken, items: &[PathBuf]) -> u64 {
    let mut values = Vec::with_capacity(items.len());
    for path in items {
        let value = path_result(py, path, false);
        if exception_pending(py) {
            for value in values {
                dec_ref_bits(py, value);
            }
            return MoltObject::none().bits();
        }
        values.push(value);
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

/// All root decisions use the shared parser. Python callers choose the native
/// Windows provider only where their target version imports that provider.
fn root_coordinate(
    windows_utf16: bool,
    bytes: bool,
) -> molt_runtime_platform::path_text::PathCoordinate {
    if windows_utf16 {
        molt_runtime_platform::path_text::PathCoordinate::WindowsUtf16
    } else if bytes {
        molt_runtime_platform::path_text::PathCoordinate::Bytes
    } else {
        molt_runtime_platform::path_text::PathCoordinate::PythonCodePoints
    }
}

/// Storage is owned across allocation/codec callbacks; offsets always refer to it.
struct ParsedRoot {
    raw: Vec<u8>,
    bytes: bool,
    drive: usize,
    root: usize,
}

impl ParsedRoot {
    /// PurePath normalizes separators and supplies a root for complete UNC
    /// shares without a trailing separator. Raw ntpath splitroot does neither.
    fn normalize_pathlib(&mut self, posix: bool) {
        if posix {
            return;
        }
        for byte in &mut self.raw {
            if *byte == b'/' {
                *byte = b'\\';
            }
        }
        let drive = &self.raw[..self.drive];
        if self.drive != self.root || !drive.starts_with(b"\\") || drive.ends_with(b"\\") {
            return;
        }
        let mut count = 0;
        let mut ordinary_server = false;
        for (index, part) in drive.split(|byte| *byte == b'\\').enumerate() {
            count += 1;
            if index == 2 {
                ordinary_server = !part.is_empty() && part != b"?" && part != b".";
            }
        }
        if (count == 4 && ordinary_server) || count == 6 {
            self.raw.insert(self.drive, b'\\');
            self.root += 1;
        }
    }
}

fn parse_root(
    py: &CoreGilToken,
    bits: u64,
    posix: bool,
    windows_utf16: bool,
) -> Result<ParsedRoot, u64> {
    let (raw, bytes) = {
        #[cfg(windows)]
        if windows_utf16 && !posix {
            let (path, bytes) = path_from_bits_with_flavor(py, bits)?;
            (path.into_os_string().into_encoded_bytes(), bytes)
        } else {
            if let Some(raw) = string_obj_bytes(obj_from_bits(bits)) {
                (raw, false)
            } else if let Some(pointer) = obj_from_bits(bits).as_ptr()
                && unsafe { object_type_id(pointer) } == TYPE_ID_BYTES
            {
                (
                    unsafe { bytes_like_slice(pointer) }
                        .expect("bytes storage")
                        .to_vec(),
                    true,
                )
            } else {
                return Err(raise_exception::<u64>(
                    py,
                    "TypeError",
                    "path must be str or bytes",
                ));
            }
        }
        #[cfg(not(windows))]
        {
            if let Some(raw) = string_obj_bytes(obj_from_bits(bits)) {
                (raw, false)
            } else if let Some(pointer) = obj_from_bits(bits).as_ptr()
                && unsafe { object_type_id(pointer) } == TYPE_ID_BYTES
            {
                (
                    unsafe { bytes_like_slice(pointer) }
                        .expect("bytes storage")
                        .to_vec(),
                    true,
                )
            } else {
                return Err(raise_exception::<u64>(
                    py,
                    "TypeError",
                    "path must be str or bytes",
                ));
            }
        }
    };
    let sep = if posix { b'/' } else { b'\\' };
    let (drive, root) = molt_runtime_platform::path_text::path_root_offsets(
        &raw,
        sep,
        root_coordinate(windows_utf16, bytes),
    );
    Ok(ParsedRoot {
        raw,
        bytes,
        drive,
        root,
    })
}

// ---------------------------------------------------------------------------
// Public intrinsics -- Pure path operations
// ---------------------------------------------------------------------------

/// Returns normalized pathlib parts using the same root provider as splitroot.
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_parts(
    path_bits: u64,
    posix_bits: u64,
    windows_utf16_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let windows_utf16 = is_truthy(_py, obj_from_bits(windows_utf16_bits));
        let mut parsed = match parse_root(_py, path_bits, posix, windows_utf16) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if parsed.bytes {
            return raise_exception::<u64>(_py, "TypeError", "path must be str");
        }
        parsed.normalize_pathlib(posix);
        let sep = if posix { b'/' } else { b'\\' };
        let raw = &parsed.raw;
        let root = parsed.root;
        let mut slices = Vec::new();
        if root != 0 {
            slices.push(&raw[..root]);
        }
        slices.extend(
            raw[root..]
                .split(|byte| *byte == sep)
                .filter(|part| !part.is_empty() && *part != b"."),
        );
        let mut owners = Vec::with_capacity(slices.len());
        for part in slices {
            let ptr = alloc_string(_py, part);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            owners.push(unsafe {
                molt_runtime_core::ProtectedRuntimeValue::from_owned_bits(
                    _py,
                    MoltObject::from_ptr(ptr).bits(),
                )
            });
        }
        let elems: Vec<_> = owners.iter().map(|owner| owner.bits()).collect();
        let ptr = alloc_tuple(_py, &elems);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// Split original separator spelling; pathlib normalizes its own input first.
#[unsafe(no_mangle)]
pub extern "C" fn molt_path_splitroot(
    path_bits: u64,
    posix_bits: u64,
    windows_utf16_bits: u64,
    pathlib_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let windows_utf16 = is_truthy(_py, obj_from_bits(windows_utf16_bits));
        let mut parsed = match parse_root(_py, path_bits, posix, windows_utf16) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if is_truthy(_py, obj_from_bits(pathlib_bits)) {
            parsed.normalize_pathlib(posix);
        }
        let raw = &parsed.raw;
        let (drive, root) = (parsed.drive, parsed.root);
        let mut owners = Vec::with_capacity(3);
        for part in [&raw[..drive], &raw[drive..root], &raw[root..]] {
            let bits = if parsed.bytes && !(cfg!(windows) && windows_utf16 && !posix) {
                let pointer = alloc_bytes(_py, part);
                if pointer.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(pointer).bits()
            } else if parsed.bytes {
                // A native Windows codec projection uses unchanged OsStr
                // storage cut only at ASCII parser boundaries.
                let value = unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(part) };
                path_result(_py, Path::new(value), true)
            } else {
                let ptr = alloc_string(_py, part);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            };
            let owner = unsafe { ProtectedRuntimeValue::from_owned_bits(_py, bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            owners.push(owner);
        }
        let elems: Vec<_> = owners.iter().map(|owner| owner.bits()).collect();
        let ptr = alloc_tuple(_py, &elems);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
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
            Ok(p) => path_result(_py, &p, false),
            Err(err) => raise_os_error::<u64>(_py, err, "cwd"),
        }
    })
}

/// `path.resolve()` -> str  (absolute, normalized path)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_resolve(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "pathlib.resolve", audit_path_arg(path_bits)) {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let p = if s.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            s.clone()
        };
        match fs::canonicalize(&p) {
            Ok(abs) => path_result(_py, &abs, false),
            Err(_) => {
                // If the path doesn't exist, just make it absolute
                match std::env::current_dir() {
                    Ok(cwd) => path_result(_py, &cwd.join(&p), false),
                    Err(err) => raise_os_error::<u64>(_py, err, "resolve"),
                }
            }
        }
    })
}

/// `path.is_mount()` -> bool
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_is_mount(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "pathlib.is_mount", audit_path_arg(path_bits)) {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
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
        if let Err(error) = require_operation(_py, "pathlib.iterdir", audit_path_arg(path_bits)) {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
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
                Ok(e) => names.push(e.path()),
                Err(err) => return raise_os_error::<u64>(_py, err, "iterdir"),
            }
        }
        list_of_paths(_py, &names)
    })
}

/// `path.touch(exist_ok=True)` -> None
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_touch(path_bits: u64, exist_ok_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "pathlib.touch", audit_path_arg(path_bits)) {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
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
                    &format!("File exists: '{}'", s.display()),
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
        if let Err(error) = require_operation(_py, "pathlib.hardlink_to", audit_path_arg(path_bits))
        {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let target = match path_from_bits(_py, target_bits) {
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

/// `path.read_bytes()` -> bytes
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_read_bytes(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "pathlib.read_bytes", audit_path_arg(path_bits))
        {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
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

/// `path.write_bytes(data)` -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_write_bytes(path_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "pathlib.write_bytes", audit_path_arg(path_bits))
        {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        // The Python facade has already acquired memoryview(data), as
        // pathlib does. Opening/truncating precedes the file writer's SIMPLE
        // request, including rejection of a noncontiguous memoryview.
        let mut file = match fs::File::create(&s) {
            Ok(file) => file,
            Err(err) => return raise_os_error::<u64>(_py, err, "write_bytes"),
        };
        let input = match unsafe {
            molt_cpython_abi::native_custody::acquire_buffer(
                data_bits,
                molt_cpython_abi::native_custody::PyBUF_SIMPLE,
            )
        } {
            Ok(input) => input,
            Err(_) => return MoltObject::none().bits(),
        };
        let data = unsafe { molt_cpython_abi::native_custody::buffer_bytes(&input) };
        match std::io::Write::write_all(&mut file, data) {
            Ok(()) => MoltObject::from_int(data.len() as i64).bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "write_bytes"),
        }
    })
}

/// `path.owner()` -> str (Unix only)
#[unsafe(no_mangle)]
pub extern "C" fn molt_pathlib_owner(path_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if let Err(error) = require_operation(_py, "pathlib.owner", audit_path_arg(path_bits)) {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
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
            {
                use std::os::unix::ffi::OsStrExt;
                path_result(
                    _py,
                    Path::new(std::ffi::OsStr::from_bytes(name.to_bytes())),
                    false,
                )
            }
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
        if let Err(error) = require_operation(_py, "pathlib.group", audit_path_arg(path_bits)) {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
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
            {
                use std::os::unix::ffi::OsStrExt;
                path_result(
                    _py,
                    Path::new(std::ffi::OsStr::from_bytes(name.to_bytes())),
                    false,
                )
            }
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
        if let Err(error) = require_operation(_py, "pathlib.samefile", audit_path_arg(path_bits)) {
            return error;
        }
        let s = match path_from_bits(_py, path_bits) {
            Ok(s) => s,
            Err(bits) => return bits,
        };
        let other = match path_from_bits(_py, other_bits) {
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

#[cfg(test)]
mod root_projection_tests {
    use super::ParsedRoot;

    #[test]
    fn raw_and_pathlib_unc_projections_have_distinct_required_roots() {
        for (raw, drive, root, expected) in [
            (
                b"//server/share".as_slice(),
                14,
                14,
                b"\\\\server\\share\\".as_slice(),
            ),
            (
                b"//?/UNC/server/share".as_slice(),
                20,
                20,
                b"\\\\?\\UNC\\server\\share\\".as_slice(),
            ),
            (
                b"//./device".as_slice(),
                10,
                10,
                b"\\\\.\\device".as_slice(),
            ),
            (
                b"//server/share/leaf".as_slice(),
                14,
                15,
                b"\\\\server\\share\\leaf".as_slice(),
            ),
        ] {
            let mut parsed = ParsedRoot {
                raw: raw.to_vec(),
                bytes: false,
                drive,
                root,
            };
            assert_eq!(parsed.raw, raw); // raw projection preserves spelling
            parsed.normalize_pathlib(false);
            assert_eq!(parsed.raw, expected);
            assert_eq!(parsed.drive, drive);
            assert_eq!(
                parsed.root,
                if expected.get(drive) == Some(&b'\\') {
                    drive + 1
                } else {
                    root
                }
            );
        }
        let mut posix = ParsedRoot {
            raw: b"//server/share".to_vec(),
            bytes: false,
            drive: 0,
            root: 2,
        };
        posix.normalize_pathlib(true);
        assert_eq!(posix.raw, b"//server/share");
        assert_eq!((posix.drive, posix.root), (0, 2));
    }
}
