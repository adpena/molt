// Path, glob, and filesystem utility functions.
// Extracted from io.rs for compilation-unit size reduction and tree shaking.

use crate::*;
use molt_obj_model::MoltObject;
pub(crate) use molt_runtime_platform::path_text::{
    glob_translate_text, path_as_uri_text, path_basename_text, path_dirname_text,
    path_glob_matches, path_isabs_text, path_match_text, path_normpath_raw, path_normpath_text,
    path_splitext_text,
};
use num_bigint::Sign;
use num_traits::ToPrimitive;
#[cfg(not(any(unix, windows)))]
use std::io::ErrorKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathFlavor {
    Str,
    Bytes,
}

pub(crate) fn path_root_coordinate(
    py: &PyToken<'_>,
    bytes: bool,
) -> molt_runtime_platform::path_text::PathCoordinate {
    use molt_runtime_platform::path_text::PathCoordinate;
    if cfg!(windows) && crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
        PathCoordinate::WindowsUtf16
    } else if bytes {
        PathCoordinate::Bytes
    } else {
        PathCoordinate::PythonCodePoints
    }
}

/// A parsed path in its storage coordinate and its `(drive, root)` offsets.
pub(crate) type RootPathParse<'a> = (std::borrow::Cow<'a, [u8]>, (usize, usize));

/// Parse through the selected native provider, then return offsets in the
/// original Python storage coordinate. Windows byte paths cross the existing
/// filesystem codec only here; subsequent join/split operations remain bytes.
pub(crate) fn root_path_parse<'a>(
    py: &PyToken<'_>,
    raw: &'a [u8],
    sep: u8,
    bytes: bool,
    coordinate: molt_runtime_platform::path_text::PathCoordinate,
) -> Result<RootPathParse<'a>, u64> {
    use molt_runtime_platform::path_text::{PathCoordinate, path_root_offsets};
    if coordinate != PathCoordinate::WindowsUtf16 {
        return Ok((
            std::borrow::Cow::Borrowed(raw),
            path_root_offsets(raw, sep, coordinate),
        ));
    }
    // Valid UTF-8 text already has the canonical UTF-16 round-trip spelling.
    if !bytes && std::str::from_utf8(raw).is_ok() {
        return Ok((
            std::borrow::Cow::Borrowed(raw),
            path_root_offsets(raw, sep, coordinate),
        ));
    }
    let value = if bytes {
        os_string_from_bytes(py, raw)?
    } else {
        os_string_from_text(py, raw, None)?
    };
    let storage = value.as_encoded_bytes();
    let (drive, root) = path_root_offsets(storage, sep, coordinate);
    if !bytes {
        return Ok((
            std::borrow::Cow::Owned(value.into_encoded_bytes()),
            (drive, root),
        ));
    }
    let mut projected = Vec::new();
    let mut offsets = [0; 2];
    for (index, part) in [&storage[..drive], &storage[drive..root], &storage[root..]]
        .into_iter()
        .enumerate()
    {
        // Parser boundaries are ASCII in unchanged Windows OsString storage.
        projected.extend_from_slice(&os_string_bytes(py, unsafe {
            std::ffi::OsStr::from_encoded_bytes_unchecked(part)
        })?);
        if index < 2 {
            offsets[index] = projected.len();
        }
    }
    Ok((std::borrow::Cow::Owned(projected), (offsets[0], offsets[1])))
}

fn glob_join_path(
    py: &PyToken<'_>,
    base: &[u8],
    part: &[u8],
    sep: char,
    bytes: bool,
    coordinate: molt_runtime_platform::path_text::PathCoordinate,
) -> Result<Vec<u8>, u64> {
    let (base, base_root) = root_path_parse(py, base, sep as u8, bytes, coordinate)?;
    let (part, part_root) = root_path_parse(py, part, sep as u8, bytes, coordinate)?;
    let mut joined = molt_runtime_platform::path_text::PathJoin::new(
        base.into_owned(),
        sep as u8,
        !bytes,
        base_root.0,
    );
    joined.push(&part, part_root);
    Ok(joined.finish())
}

fn glob_split_path(
    py: &PyToken<'_>,
    raw: &[u8],
    sep: char,
    bytes: bool,
    coordinate: molt_runtime_platform::path_text::PathCoordinate,
) -> Result<(Vec<u8>, Vec<u8>), u64> {
    let (storage, (_, root)) = root_path_parse(py, raw, sep as u8, bytes, coordinate)?;
    Ok(molt_runtime_platform::path_text::glob_split_path_with_root(
        &storage, sep as u8, root,
    ))
}

/// Resolve the path protocol once and retain its original result for file.name.
/// Conversion failures retire that owned result with the selected error visible.
pub(crate) fn path_and_name_from_bits(
    py: &PyToken<'_>,
    file: u64,
) -> Result<(std::path::PathBuf, u64, PathFlavor), u64> {
    let (value, flavor) = fspath_bits_with_flavor(py, file)?;
    let owner =
        unsafe { molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), value) };
    let pointer = obj_from_bits(value)
        .as_ptr()
        .expect("fspath owns str or bytes storage");
    let raw = if flavor == PathFlavor::Str {
        let bytes =
            unsafe { std::slice::from_raw_parts(string_bytes(pointer), string_len(pointer)) };
        os_string_from_text(py, bytes, Some(value))?
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(bytes_data(pointer), bytes_len(pointer)) };
        os_string_from_bytes(py, bytes)?
    };
    let path = std::path::PathBuf::from(raw);
    Ok((path, owner.into_bits(), flavor))
}

pub(crate) fn path_from_bits_with_flavor(
    py: &PyToken<'_>,
    file: u64,
) -> Result<(std::path::PathBuf, PathFlavor), u64> {
    let (path, name, flavor) = path_and_name_from_bits(py, file)?;
    dec_ref_bits(py, name);
    Ok((path, flavor))
}

pub(crate) fn fspath_bits_with_flavor(
    _py: &PyToken<'_>,
    file_bits: u64,
) -> Result<(u64, PathFlavor), u64> {
    let obj = obj_from_bits(file_bits);
    let Some(ptr) = obj.as_ptr() else {
        let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
        let msg = format!("expected str, bytes or os.PathLike object, not {obj_type}");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };

    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_STRING {
            inc_ref_bits(_py, file_bits);
            return Ok((file_bits, PathFlavor::Str));
        }
        if type_id == TYPE_ID_BYTES {
            inc_ref_bits(_py, file_bits);
            return Ok((file_bits, PathFlavor::Bytes));
        }
        // PyOS_FSPath deliberately replaces a failed special-method lookup
        // with its operand TypeError. Python 3.13 also treats __fspath__ = None
        // as absence; 3.12 reaches ordinary non-callable validation.
        let method = crate::builtins::attr::lookup_special_method(_py, file_bits, b"__fspath__")
            .unwrap_or_default();
        if let Some(call_bits) = method {
            if obj_from_bits(call_bits).is_none()
                && crate::object::ops_sys::runtime_target_at_least(_py, 3, 13)
            {
                dec_ref_bits(_py, call_bits);
                let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
                let msg = format!("expected str, bytes or os.PathLike object, not {obj_type}");
                return Err(raise_exception::<u64>(_py, "TypeError", &msg));
            }
            let res_bits = call_callable0(_py, call_bits);
            let failed = exception_pending(_py);
            dec_ref_bits(_py, call_bits);
            if failed {
                dec_ref_bits(_py, res_bits);
                return Err(MoltObject::none().bits());
            }
            let res_obj = obj_from_bits(res_bits);
            if let Some(res_ptr) = res_obj.as_ptr() {
                let res_type_id = object_type_id(res_ptr);
                if res_type_id == TYPE_ID_STRING {
                    return Ok((res_bits, PathFlavor::Str));
                }
                if res_type_id == TYPE_ID_BYTES {
                    return Ok((res_bits, PathFlavor::Bytes));
                }
            }
            let res_type = class_name_for_error(type_of_bits(_py, res_bits));
            let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
            let msg =
                format!("expected {obj_type}.__fspath__() to return str or bytes, not {res_type}");
            let error = raise_exception::<u64>(_py, "TypeError", &msg);
            dec_ref_bits(_py, res_bits);
            return Err(error);
        }
    }

    let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
    let msg = format!("expected str, bytes or os.PathLike object, not {obj_type}");
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

pub(crate) fn path_from_bits(_py: &PyToken<'_>, file_bits: u64) -> Result<std::path::PathBuf, u64> {
    path_from_bits_with_flavor(_py, file_bits).map(|(path, _flavor)| path)
}

/// Python text uses UTF-8 surrogatepass; OS strings use the selected filesystem
/// codec on POSIX and native wide-character storage on Windows.
pub(crate) fn os_string_from_text(
    py: &PyToken<'_>,
    text: &[u8],
    owner: Option<u64>,
) -> Result<std::ffi::OsString, u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let points = molt_runtime_core::text_storage::code_points(text);
        let mut wide = Vec::with_capacity(points.size_hint().0);
        for code in points {
            if code <= 0xffff {
                wide.push(code as u16);
            } else {
                let scalar = code - 0x10000;
                wide.push(0xd800 | (scalar >> 10) as u16);
                wide.push(0xdc00 | (scalar & 0x3ff) as u16);
            }
        }
        let _ = (py, owner);
        Ok(std::ffi::OsString::from_wide(&wide))
    }
    #[cfg(not(windows))]
    {
        let policy = crate::builtins::platform::startup_encoding(py);
        let raw = crate::object::ops::encode_string_with_errors(
            text,
            &policy.filesystem_encoding,
            Some(policy.filesystem_errors),
        )
        .map_err(|error| {
            if let Some(owner) = owner {
                return error.raise::<u64>(py, owner);
            }
            let pointer = alloc_string(py, text);
            if pointer.is_null() {
                return MoltObject::none().bits();
            }
            let bits = MoltObject::from_ptr(pointer).bits();
            let result = error.raise::<u64>(py, bits);
            dec_ref_bits(py, bits);
            result
        })?;
        // All byte sequences are valid OS strings on POSIX/WASI.
        Ok(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(raw) })
    }
}

pub(crate) fn os_string_from_bytes(
    py: &PyToken<'_>,
    bytes: &[u8],
) -> Result<std::ffi::OsString, u64> {
    #[cfg(not(windows))]
    {
        let _ = py;
        Ok(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(bytes.to_vec()) })
    }
    #[cfg(windows)]
    {
        let text = filesystem_decode(py, bytes)?;
        // Wide-character construction does not use the error object argument.
        os_string_from_text(py, &text, None)
    }
}

pub(crate) fn filesystem_decode(py: &PyToken<'_>, bytes: &[u8]) -> Result<Vec<u8>, u64> {
    let policy = crate::builtins::platform::startup_encoding(py);
    crate::object::ops::decode_bytes_text(
        &policy.filesystem_encoding,
        policy.filesystem_errors,
        bytes,
    )
    .map(|(text, _)| text)
    .map_err(|error| error.raise::<u64>(py, bytes))
}

pub(crate) fn os_string_text(py: &PyToken<'_>, value: &std::ffi::OsStr) -> Result<Vec<u8>, u64> {
    #[cfg(windows)]
    {
        let _ = py;
        // The current Rust target's Windows OsStr representation is WTF-8.
        Ok(value.as_encoded_bytes().to_vec())
    }
    #[cfg(not(windows))]
    {
        filesystem_decode(py, value.as_encoded_bytes())
    }
}

pub(crate) fn os_string_bytes(py: &PyToken<'_>, value: &std::ffi::OsStr) -> Result<Vec<u8>, u64> {
    #[cfg(not(windows))]
    {
        let _ = py;
        Ok(value.as_encoded_bytes().to_vec())
    }
    #[cfg(windows)]
    {
        let text = value.as_encoded_bytes();
        let policy = crate::builtins::platform::startup_encoding(py);
        crate::object::ops::encode_string_with_errors(
            text,
            &policy.filesystem_encoding,
            Some(policy.filesystem_errors),
        )
        .map_err(|error| {
            let pointer = alloc_string(py, text);
            if pointer.is_null() {
                return MoltObject::none().bits();
            }
            let owner = MoltObject::from_ptr(pointer).bits();
            let result = error.raise::<u64>(py, owner);
            dec_ref_bits(py, owner);
            result
        })
    }
}

pub(crate) fn path_result_bits(
    py: &PyToken<'_>,
    path: &std::path::Path,
    flavor: PathFlavor,
) -> u64 {
    let raw = match if flavor == PathFlavor::Str {
        os_string_text(py, path.as_os_str())
    } else {
        os_string_bytes(py, path.as_os_str())
    } {
        Ok(raw) => raw,
        Err(error) => return error,
    };
    let pointer = if flavor == PathFlavor::Str {
        alloc_string(py, &raw)
    } else {
        alloc_bytes(py, &raw)
    };
    if pointer.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(pointer).bits()
    }
}

pub(crate) fn path_sep_char() -> char {
    std::path::MAIN_SEPARATOR
}

pub(crate) fn path_value_with_flavor_from_bits(
    py: &PyToken<'_>,
    bits: u64,
) -> Result<(Vec<u8>, PathFlavor), u64> {
    let (bits, flavor) = fspath_bits_with_flavor(py, bits)?;
    let owner =
        unsafe { molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) };
    let pointer = obj_from_bits(owner.bits())
        .as_ptr()
        .expect("fspath storage");
    let value = unsafe {
        if flavor == PathFlavor::Str {
            std::slice::from_raw_parts(string_bytes(pointer), string_len(pointer)).to_vec()
        } else {
            std::slice::from_raw_parts(bytes_data(pointer), bytes_len(pointer)).to_vec()
        }
    };
    Ok((value, flavor))
}

pub(crate) fn path_string_with_flavor_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
) -> Result<(String, PathFlavor), u64> {
    let (value, flavor) = path_value_with_flavor_from_bits(_py, bits)?;
    // Legacy text-only lexical operations do not cross the OS boundary. In
    // particular they must not acquire filesystem-codec admission here.
    let text = if flavor == PathFlavor::Bytes {
        value.iter().map(|byte| char::from(*byte)).collect()
    } else {
        String::from_utf8_lossy(&value).into_owned()
    };
    Ok((text, flavor))
}

pub(crate) fn path_string_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<String, u64> {
    path_string_with_flavor_from_bits(_py, bits).map(|(path, _flavor)| path)
}

pub(crate) fn path_str_arg_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    label: &str,
) -> Result<String, u64> {
    if let Some(text) = string_obj_to_owned(obj_from_bits(bits)) {
        return Ok(text);
    }
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("{label} must be str, not {type_name}");
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

pub(crate) fn alloc_path_list_bits(_py: &PyToken<'_>, values: &[Vec<u8>], bytes_out: bool) -> u64 {
    let mut out_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = if bytes_out {
            alloc_bytes(_py, value)
        } else {
            alloc_string(_py, value)
        };
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
}

/// Native path projection of the shared lexical normalizer. ASCII component
/// boundaries preserve the platform OsStr encoding, including opaque POSIX bytes.
pub(crate) fn path_abspath_path(
    py: &PyToken<'_>,
    path: &std::path::Path,
) -> Result<std::path::PathBuf, u64> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        if !has_capability(py, "fs.read") {
            return Err(raise_exception::<_>(
                py,
                "PermissionError",
                "missing fs.read capability",
            ));
        }
        let path = if path.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            path
        };
        std::path::absolute(path).map_err(|error| raise_os_error::<u64>(py, error, "abspath"))?
    };
    let normalized = path_normpath_raw(
        absolute.as_os_str().as_encoded_bytes(),
        path_sep_char() as u8,
    );
    // SAFETY: normalization only splits at ASCII separators, removes whole
    // components, and rejoins unchanged encoded components with ASCII. Every
    // slice originates from this platform's OsStr encoding in the same process.
    Ok(std::path::PathBuf::from(unsafe {
        std::ffi::OsString::from_encoded_bytes_unchecked(normalized)
    }))
}

pub(crate) fn path_abspath_text(py: &PyToken<'_>, path: &str, sep: char) -> Result<String, u64> {
    debug_assert_eq!(sep, path_sep_char());
    let absolute = path_abspath_path(py, std::path::Path::new(path))?;
    absolute.into_os_string().into_string().map_err(|_| {
        raise_exception::<u64>(py, "UnicodeError", "absolute path is not valid Unicode")
    })
}

#[derive(Clone, Debug)]
pub(crate) enum GlobDirFdArg {
    None,
    Int(i64),
    PathLike {
        path: Vec<u8>,
        flavor: PathFlavor,
        type_name: String,
    },
    BadType {
        type_name: String,
    },
}

fn glob_dir_fd_type_error_bits(_py: &PyToken<'_>, type_name: &str) -> u64 {
    let msg = format!("argument should be integer or None, not {type_name}");
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn glob_scandir_type_error_bits(_py: &PyToken<'_>, type_name: &str) -> u64 {
    let msg = format!(
        "scandir: path should be string, bytes, os.PathLike, integer or None, not {type_name}"
    );
    raise_exception::<_>(_py, "TypeError", &msg)
}

pub(crate) fn glob_dir_fd_arg_from_bits(
    _py: &PyToken<'_>,
    dir_fd_bits: u64,
) -> Result<GlobDirFdArg, u64> {
    if obj_from_bits(dir_fd_bits).is_none() {
        return Ok(GlobDirFdArg::None);
    }
    let type_name = class_name_for_error(type_of_bits(_py, dir_fd_bits));
    let err = format!("argument should be integer or None, not {type_name}");
    if let Some(value) = index_bigint_from_obj(_py, dir_fd_bits, &err) {
        if let Some(fd) = value.to_i64() {
            return Ok(GlobDirFdArg::Int(fd));
        }
        let msg = if value.sign() == Sign::Minus {
            "fd is less than minimum"
        } else {
            "fd is greater than maximum"
        };
        return Err(raise_exception::<_>(_py, "OverflowError", msg));
    }
    if exception_pending(_py) {
        clear_exception(_py);
    }
    match path_value_with_flavor_from_bits(_py, dir_fd_bits) {
        Ok((path, flavor)) => {
            #[cfg(windows)]
            let path = path
                .into_iter()
                .map(|byte| if byte == b'/' { b'\\' } else { byte })
                .collect();
            Ok(GlobDirFdArg::PathLike {
                path,
                flavor,
                type_name,
            })
        }
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            Ok(GlobDirFdArg::BadType { type_name })
        }
    }
}

fn glob_text_to_path(
    py: &PyToken<'_>,
    text: &[u8],
    bytes_mode: bool,
) -> Result<std::path::PathBuf, u64> {
    let value = if bytes_mode {
        os_string_from_bytes(py, text)?
    } else {
        os_string_from_text(py, text, None)?
    };
    Ok(std::path::PathBuf::from(value))
}

fn glob_dir_entry_name_text(
    py: &PyToken<'_>,
    name: &std::ffi::OsStr,
    bytes_mode: bool,
) -> Result<Vec<u8>, u64> {
    if !bytes_mode {
        return os_string_text(py, name);
    }
    #[cfg(not(windows))]
    {
        Ok(name.as_encoded_bytes().to_vec())
    }
    #[cfg(windows)]
    {
        let policy = crate::builtins::platform::startup_encoding(py);
        let text = name.as_encoded_bytes();
        crate::object::ops::encode_string_with_errors(
            text,
            &policy.filesystem_encoding,
            Some(policy.filesystem_errors),
        )
        .map_err(|error| {
            let pointer = alloc_string(py, text);
            if pointer.is_null() {
                return MoltObject::none().bits();
            }
            let bits = MoltObject::from_ptr(pointer).bits();
            let result = error.raise::<u64>(py, bits);
            dec_ref_bits(py, bits);
            result
        })
    }
}

pub(crate) fn glob_dir_fd_root_path(fd: i64) -> Option<std::path::PathBuf> {
    if fd < 0 {
        return None;
    }
    #[cfg(all(unix, target_vendor = "apple"))]
    {
        let mut bytes = vec![0u8; libc::PATH_MAX as usize];
        let rc = unsafe {
            libc::fcntl(
                fd as libc::c_int,
                libc::F_GETPATH,
                bytes.as_mut_ptr() as *mut libc::c_char,
            )
        };
        if rc != -1
            && let Some(end) = bytes.iter().position(|byte| *byte == 0)
            && end > 0
        {
            bytes.truncate(end);
            return Some(std::path::PathBuf::from(unsafe {
                std::ffi::OsString::from_encoded_bytes_unchecked(bytes)
            }));
        }
    }
    #[cfg(any(unix, target_arch = "wasm32"))]
    {
        for candidate in [format!("/proc/self/fd/{fd}"), format!("/dev/fd/{fd}")] {
            if let Ok(path) = std::fs::read_link(&candidate) {
                return Some(path);
            }
            if let Ok(path) = std::fs::canonicalize(&candidate) {
                return Some(path);
            }
        }
    }
    #[cfg(windows)]
    {
        let handle = unsafe { libc::get_osfhandle(fd as libc::c_int) };
        if handle != -1 {
            return crate::builtins::io::windows_path_from_handle(handle as *mut std::ffi::c_void)
                .map(std::path::PathBuf::from);
        }
    }
    None
}

fn glob_lexists_text(
    py: &PyToken<'_>,
    path: &[u8],
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    _sep: char,
) -> Result<bool, u64> {
    if path.is_empty() {
        return Ok(false);
    }
    let mut resolved = glob_text_to_path(py, path, bytes_mode)?;
    match dir_fd {
        GlobDirFdArg::None => (),
        GlobDirFdArg::Int(fd) if !resolved.is_absolute() => {
            let Some(root) = glob_dir_fd_root_path(*fd) else {
                return Ok(false);
            };
            resolved = root.join(resolved);
        }
        GlobDirFdArg::Int(_) => (),
        GlobDirFdArg::PathLike { type_name, .. } | GlobDirFdArg::BadType { type_name } => {
            return Err(glob_dir_fd_type_error_bits(py, type_name));
        }
    }
    Ok(std::fs::symlink_metadata(resolved).is_ok())
}

fn glob_is_dir_text(
    py: &PyToken<'_>,
    path: &[u8],
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    _sep: char,
) -> Result<bool, u64> {
    if path.is_empty() {
        return Ok(false);
    }
    let mut resolved = glob_text_to_path(py, path, bytes_mode)?;
    match dir_fd {
        GlobDirFdArg::None => (),
        GlobDirFdArg::Int(fd) if !resolved.is_absolute() => {
            let Some(root) = glob_dir_fd_root_path(*fd) else {
                return Ok(false);
            };
            resolved = root.join(resolved);
        }
        GlobDirFdArg::Int(_) => (),
        GlobDirFdArg::PathLike { type_name, .. } | GlobDirFdArg::BadType { type_name } => {
            return Err(glob_dir_fd_type_error_bits(py, type_name));
        }
    }
    Ok(std::fs::metadata(resolved)
        .map(|meta| meta.is_dir())
        .unwrap_or(false))
}

struct GlobListdirResult {
    names: Vec<Vec<u8>>,
    names_are_bytes: bool,
}

fn glob_listdir_text(
    py: &PyToken<'_>,
    dirname: &[u8],
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    bytes_mode: bool,
    _sep: char,
) -> Result<GlobListdirResult, u64> {
    let mut names_are_bytes = bytes_mode;
    let target = match dir_fd {
        GlobDirFdArg::None => glob_text_to_path(
            py,
            if dirname.is_empty() { b"." } else { dirname },
            bytes_mode,
        )?,
        GlobDirFdArg::Int(fd) => {
            if dirname.is_empty() {
                names_are_bytes = bytes_mode;
                match glob_dir_fd_root_path(*fd) {
                    Some(root) => root,
                    None if *fd == -1 => std::path::PathBuf::from("."),
                    None => {
                        return Ok(GlobListdirResult {
                            names: Vec::new(),
                            names_are_bytes,
                        });
                    }
                }
            } else {
                let path = glob_text_to_path(py, dirname, bytes_mode)?;
                if path.is_absolute() {
                    path
                } else if let Some(root) = glob_dir_fd_root_path(*fd) {
                    root.join(path)
                } else {
                    return Ok(GlobListdirResult {
                        names: Vec::new(),
                        names_are_bytes,
                    });
                }
            }
        }
        GlobDirFdArg::PathLike {
            path,
            flavor,
            type_name,
        } => {
            if !dirname.is_empty() {
                return Err(glob_dir_fd_type_error_bits(py, type_name));
            }
            names_are_bytes |= *flavor == PathFlavor::Bytes;
            glob_text_to_path(py, path, *flavor == PathFlavor::Bytes)?
        }
        GlobDirFdArg::BadType { type_name } => {
            return Err(if dirname.is_empty() {
                glob_scandir_type_error_bits(py, type_name)
            } else {
                glob_dir_fd_type_error_bits(py, type_name)
            });
        }
    };
    let mut names = Vec::new();
    if let Ok(iter) = std::fs::read_dir(target) {
        for entry in iter {
            let Ok(entry) = entry else {
                continue;
            };
            if dironly && !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            names.push(glob_dir_entry_name_text(
                py,
                &entry.file_name(),
                names_are_bytes,
            )?);
        }
    }
    Ok(GlobListdirResult {
        names,
        names_are_bytes,
    })
}

#[allow(clippy::too_many_arguments)]
fn glob1_text(
    _py: &PyToken<'_>,
    dirname: &[u8],
    pattern: &[u8],
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<Vec<u8>>, u64> {
    let listed = glob_listdir_text(_py, dirname, dir_fd, dironly, bytes_mode, sep)?;
    if listed.names_are_bytes != bytes_mode {
        let msg = if bytes_mode {
            "cannot use a bytes pattern on a string-like object"
        } else {
            "cannot use a string pattern on a bytes-like object"
        };
        return Err(raise_exception::<_>(_py, "TypeError", msg));
    }
    let mut names = listed.names;
    if !(pattern.starts_with(b".") || include_hidden) {
        names.retain(|name| !name.starts_with(b"."));
    }
    if bytes_mode {
        names
            .retain(|name| molt_runtime_platform::fnmatch::fnmatch_match_bytes_impl(name, pattern));
    } else {
        let pattern: Vec<u32> = molt_runtime_core::text_storage::code_points(pattern).collect();
        names.retain(|name| {
            let name: Vec<u32> = molt_runtime_core::text_storage::code_points(name).collect();
            molt_runtime_platform::fnmatch::fnmatch_match_units(&name, &pattern)
        });
    }
    Ok(names)
}

fn glob0_text(
    _py: &PyToken<'_>,
    dirname: &[u8],
    basename: &[u8],
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    sep: char,
    coordinate: molt_runtime_platform::path_text::PathCoordinate,
) -> Result<Vec<Vec<u8>>, u64> {
    if !basename.is_empty() {
        let full = glob_join_path(_py, dirname, basename, sep, bytes_mode, coordinate)?;
        if glob_lexists_text(_py, &full, dir_fd, bytes_mode, sep)? {
            return Ok(vec![basename.to_vec()]);
        }
        return Ok(Vec::new());
    }
    if glob_is_dir_text(_py, dirname, dir_fd, bytes_mode, sep)? {
        return Ok(vec![Vec::new()]);
    }
    Ok(Vec::new())
}

// ===========================================================================
// Lazy streaming glob iterator (`glob.iglob`)
//
// CPython's `glob.iglob` is a generator chain (`_iglob` -> `_glob1`/`_glob2`/
// `_glob0` + `_rlistdir`, driven by lazy `os.scandir`). The historical molt
// hack made `iglob == glob` (full materialization), which OOMs on large trees —
// the exact `os.walk` eager-listdir bug class.
//
// We cannot back this on Python generators: molt's generator protocol leaks on
// *nested* generator delegation (`yield from` / `for y in subgen: yield ...`),
// which is precisely the recursive `**` (`_rlistdir`) shape, so a pure-Python
// port would re-introduce the OOM. (Flat generators are fine; this leak is an
// open generator-runtime baton.) Instead we reify CPython's lazy generator
// chain as an explicit stack of resumable frames in Rust (no leak: Rust frees
// frames deterministically). Live memory is O(active-frame-depth x one
// directory's listing) — bounded exactly like CPython (whose `_listdir`
// materializes one directory at a time inside `_rlistdir`).
//
// Invariant that keeps this simple and provably correct: **every frame carries
// its own complete relative prefix**, so each fragment a frame yields is
// already a full root-relative result path. The driver never joins fragments
// across frames; it returns whatever the top frame yields.
//
// The bounded helpers (`glob1_text`, `glob0_text`, `glob_lexists_text`,
// `glob_is_dir_text`, `glob_listdir_text`, `glob_split_path_text`,
// `glob_join_text`, `molt_runtime_platform::path_text::glob_has_magic_raw`) are reused verbatim, so ordering,
// hidden rules, byte mode, dir_fd TypeError parity, and include_hidden are
// identical to the eager path by construction.
// ===========================================================================

/// Cursor over one directory's worth of precomputed names (bounded — the
/// `_listdir`/`_glob1`/`_glob0` result).
struct GlobNameCursor {
    names: Vec<Vec<u8>>,
    idx: usize,
}

impl GlobNameCursor {
    #[inline]
    fn next(&mut self) -> Option<Vec<u8>> {
        let out = self.names.get(self.idx).cloned();
        if out.is_some() {
            self.idx += 1;
        }
        out
    }
}

/// Which `glob_in_dir` a magic basename selects (mirrors CPython).
#[derive(Clone, Copy, PartialEq, Eq)]
enum GlobInDir {
    /// `_glob1` — basename has magic, non-recursive.
    Glob1,
    /// `_glob2` — basename == b"**", recursive.
    Glob2,
    /// `_glob0` — basename has no magic (literal existence check).
    Glob0,
}

/// Source of parent directories for an `Iglob` frame: either a single literal
/// dirname, or a nested streaming `iglob(dirname, dironly=True)` (magic dirname).
enum GlobDirSource {
    Literal(Option<Vec<u8>>),
    Recursive(Box<GlobIterState>),
}

/// One resumable frame. Every yielded `Vec<u8>` is a complete root-relative path.
enum GlobFrame {
    /// `_iglob(pathname)` body: for each parent `dir` (literal or streamed),
    /// glob `basename` in `join(root_dir, dir)` and yield `join(dir, name)`.
    Iglob {
        kind: GlobInDir,
        basename: Vec<u8>,
        dironly: bool,
        dirs: GlobDirSource,
        /// The parent dirname currently being globbed (its names are produced
        /// either via `cur_names`, for glob0/glob1, or via a pushed Glob2
        /// child frame whose prefix is this parent).
        cur_dir: Option<Vec<u8>>,
        cur_names: Option<GlobNameCursor>,
    },
    /// `_glob2(prefix @ dir, dironly)`: yield `prefix` once (if `dir` is a
    /// dir), then stream `_rlistdir` rooted at `dir` with results prefixed by
    /// `prefix`. `dironly` is the owning `_iglob` frame's value (False for the
    /// user-facing walk → files+dirs; True for inner dir-expansion → dirs only).
    Glob2 {
        prefix: Vec<u8>,
        dir: Vec<u8>,
        dironly: bool,
        emitted_empty: bool,
        started: bool,
    },
    /// `_rlistdir(dir, dironly)` with results prefixed by `prefix`: for each
    /// non-hidden `name`, yield `join(prefix, name)` and recurse into `dir/name`.
    Rlistdir {
        prefix: Vec<u8>,
        dir: Vec<u8>,
        dironly: bool,
        names: GlobNameCursor,
    },
    /// A bounded, fully-precomputed result set (non-magic pathname, or a
    /// literal-dirname basename glob). Streams its cursor.
    Names(GlobNameCursor),
}

/// Immutable per-walk configuration shared by all frames.
struct GlobIterConfig {
    coordinate: molt_runtime_platform::path_text::PathCoordinate,
    root_dir: Option<Vec<u8>>,
    dir_fd: GlobDirFdArg,
    recursive: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
    /// Skip exactly one leading empty-string result (the `iglob` `next(it)`
    /// skip for `**`/empty patterns — see `glob_matches_text`).
    skip_leading_empty: bool,
}

/// The boxed state behind a `TYPE_ID_GLOB_ITER` object: a stack of frames
/// (top = last) plus shared config. `next_path` drives it lazily.
pub(crate) struct GlobIterState {
    stack: Vec<GlobFrame>,
    cfg: GlobIterConfig,
    leading_skip_done: bool,
}

/// Outcome of advancing the top frame one step.
enum GlobStep {
    /// A complete relative path is ready.
    Yield(Vec<u8>),
    /// Push this child frame and keep driving.
    Push(GlobFrame),
    /// The top frame is exhausted; pop it.
    Pop,
}

impl GlobIterState {
    /// Build a fresh streaming iglob for `pathname`. `dironly=false` for the
    /// user-facing walk. Mirrors `glob_iglob_text`'s entry.
    fn new(
        _py: &PyToken<'_>,
        pathname: &[u8],
        cfg: GlobIterConfig,
        dironly: bool,
    ) -> Result<Self, u64> {
        let frame = glob_build_iglob_frame(_py, pathname, &cfg, dironly)?;
        Ok(GlobIterState {
            stack: vec![frame],
            cfg,
            leading_skip_done: false,
        })
    }

    /// Public driver used by the eager `glob()` drain (`list(iglob(...))`).
    pub(crate) fn next_path_public(&mut self, _py: &PyToken<'_>) -> Result<Option<Vec<u8>>, u64> {
        self.next_path(_py)
    }

    /// Produce the next matching path (root-relative), or `None` at exhaustion.
    fn next_path(&mut self, _py: &PyToken<'_>) -> Result<Option<Vec<u8>>, u64> {
        loop {
            match self.drive(_py)? {
                None => return Ok(None),
                Some(path) => {
                    if !self.leading_skip_done {
                        self.leading_skip_done = true;
                        if self.cfg.skip_leading_empty && path.is_empty() {
                            continue;
                        }
                    }
                    return Ok(Some(path));
                }
            }
        }
    }

    /// Advance the frame stack until one path is produced or it empties.
    fn drive(&mut self, _py: &PyToken<'_>) -> Result<Option<Vec<u8>>, u64> {
        loop {
            if self.stack.is_empty() {
                return Ok(None);
            }
            let idx = self.stack.len() - 1;
            match self.step_top(_py, idx)? {
                GlobStep::Yield(path) => return Ok(Some(path)),
                GlobStep::Push(child) => self.stack.push(child),
                GlobStep::Pop => {
                    self.stack.pop();
                }
            }
        }
    }

    fn step_top(&mut self, _py: &PyToken<'_>, idx: usize) -> Result<GlobStep, u64> {
        match &self.stack[idx] {
            GlobFrame::Names(_) => {
                let GlobFrame::Names(cursor) = &mut self.stack[idx] else {
                    unreachable!()
                };
                match cursor.next() {
                    Some(name) => Ok(GlobStep::Yield(name)),
                    None => Ok(GlobStep::Pop),
                }
            }
            GlobFrame::Glob2 { .. } => self.step_glob2(_py, idx),
            GlobFrame::Rlistdir { .. } => self.step_rlistdir(_py, idx),
            GlobFrame::Iglob { .. } => self.step_iglob(_py, idx),
        }
    }

    fn step_glob2(&mut self, _py: &PyToken<'_>, idx: usize) -> Result<GlobStep, u64> {
        let cfg = &self.cfg;
        let GlobFrame::Glob2 {
            prefix,
            dir,
            dironly,
            emitted_empty,
            started,
        } = &mut self.stack[idx]
        else {
            unreachable!()
        };
        let dironly = *dironly;
        if !*emitted_empty {
            *emitted_empty = true;
            let is_dir =
                dir.is_empty() || glob_is_dir_text(_py, dir, &cfg.dir_fd, cfg.bytes_mode, cfg.sep)?;
            if is_dir {
                // `_glob2` yields the bare prefix (the "**" match for the dir itself).
                return Ok(GlobStep::Yield(prefix.clone()));
            }
        }
        if !*started {
            *started = true;
            let prefix = prefix.clone();
            let dir = dir.clone();
            let listed =
                glob_listdir_text(_py, &dir, &cfg.dir_fd, dironly, cfg.bytes_mode, cfg.sep)?;
            return Ok(GlobStep::Push(GlobFrame::Rlistdir {
                prefix,
                dir,
                dironly,
                names: GlobNameCursor {
                    names: listed.names,
                    idx: 0,
                },
            }));
        }
        Ok(GlobStep::Pop)
    }

    fn step_rlistdir(&mut self, _py: &PyToken<'_>, idx: usize) -> Result<GlobStep, u64> {
        let cfg = &self.cfg;
        // Pull the next non-hidden name; yield join(prefix,name) and push the
        // subtree (Rlistdir for dir/name with prefix join(prefix,name)).
        let (name, prefix, dir, dironly) = {
            let GlobFrame::Rlistdir {
                prefix,
                dir,
                dironly,
                names,
            } = &mut self.stack[idx]
            else {
                unreachable!()
            };
            let dironly = *dironly;
            loop {
                match names.next() {
                    None => return Ok(GlobStep::Pop),
                    Some(name) => {
                        if !cfg.include_hidden && name.starts_with(b".") {
                            continue;
                        }
                        break (name, prefix.clone(), dir.clone(), dironly);
                    }
                }
            }
        };
        let child_dir = if dir.is_empty() {
            name.clone()
        } else {
            glob_join_path(_py, &dir, &name, cfg.sep, cfg.bytes_mode, cfg.coordinate)?
        };
        let child_prefix =
            glob_join_path(_py, &prefix, &name, cfg.sep, cfg.bytes_mode, cfg.coordinate)?;
        // CPython's `_rlistdir` recurses into every yielded name with the same
        // `dironly`; recursing into a non-directory simply lists nothing.
        let listed = glob_listdir_text(
            _py,
            &child_dir,
            &cfg.dir_fd,
            dironly,
            cfg.bytes_mode,
            cfg.sep,
        )?;
        // Push the subtree ABOVE this frame so it streams next; this frame then
        // resumes for the following sibling name. The child's results all carry
        // `child_prefix`, so ordering matches CPython (`yield x` before its
        // subtree, subtree before the next sibling).
        self.stack.push(GlobFrame::Rlistdir {
            prefix: child_prefix,
            dir: child_dir,
            dironly,
            names: GlobNameCursor {
                names: listed.names,
                idx: 0,
            },
        });
        Ok(GlobStep::Yield(glob_join_path(
            _py,
            &prefix,
            &name,
            cfg.sep,
            cfg.bytes_mode,
            cfg.coordinate,
        )?))
    }

    fn step_iglob(&mut self, _py: &PyToken<'_>, idx: usize) -> Result<GlobStep, u64> {
        loop {
            // 1) Drain the current parent's bounded in-dir cursor.
            {
                let GlobFrame::Iglob {
                    cur_dir, cur_names, ..
                } = &mut self.stack[idx]
                else {
                    unreachable!()
                };
                if let (Some(parent), Some(cursor)) = (cur_dir.clone(), cur_names.as_mut()) {
                    if let Some(name) = cursor.next() {
                        return Ok(GlobStep::Yield(glob_join_path(
                            _py,
                            &parent,
                            &name,
                            self.cfg.sep,
                            self.cfg.bytes_mode,
                            self.cfg.coordinate,
                        )?));
                    }
                    *cur_names = None;
                    *cur_dir = None;
                }
            }

            // 2) Advance to the next parent directory.
            let parent = match self.iglob_next_parent(_py, idx)? {
                Some(p) => p,
                None => return Ok(GlobStep::Pop),
            };

            // 3) glob_in_dir(parent, basename).
            let (kind, basename, dironly) = {
                let GlobFrame::Iglob {
                    kind,
                    basename,
                    dironly,
                    ..
                } = &self.stack[idx]
                else {
                    unreachable!()
                };
                (*kind, basename.clone(), *dironly)
            };

            if kind == GlobInDir::Glob2 {
                // Recursive `**` basename: stream via a Glob2 child whose
                // results are prefixed by `parent` (so they bubble up as
                // complete `join(parent, fragment)` paths).
                let search_dir = match self.cfg.root_dir.as_deref() {
                    Some(root) => glob_join_path(
                        _py,
                        root,
                        &parent,
                        self.cfg.sep,
                        self.cfg.bytes_mode,
                        self.cfg.coordinate,
                    )?,
                    None => parent.clone(),
                };
                return Ok(GlobStep::Push(GlobFrame::Glob2 {
                    prefix: parent,
                    dir: search_dir,
                    dironly,
                    emitted_empty: false,
                    started: false,
                }));
            }

            // Bounded glob1/glob0 for this parent.
            let search_dir = match self.cfg.root_dir.as_deref() {
                Some(root) => glob_join_path(
                    _py,
                    root,
                    &parent,
                    self.cfg.sep,
                    self.cfg.bytes_mode,
                    self.cfg.coordinate,
                )?,
                None => parent.clone(),
            };
            let names = match kind {
                GlobInDir::Glob1 => glob1_text(
                    _py,
                    &search_dir,
                    &basename,
                    &self.cfg.dir_fd,
                    dironly,
                    self.cfg.include_hidden,
                    self.cfg.bytes_mode,
                    self.cfg.sep,
                )?,
                GlobInDir::Glob0 => glob0_text(
                    _py,
                    &search_dir,
                    &basename,
                    &self.cfg.dir_fd,
                    self.cfg.bytes_mode,
                    self.cfg.sep,
                    self.cfg.coordinate,
                )?,
                GlobInDir::Glob2 => unreachable!(),
            };
            let GlobFrame::Iglob {
                cur_dir, cur_names, ..
            } = &mut self.stack[idx]
            else {
                unreachable!()
            };
            *cur_dir = Some(parent);
            *cur_names = Some(GlobNameCursor { names, idx: 0 });
            // Loop to drain the cursor we just filled.
        }
    }

    /// Produce the next parent directory for an `Iglob` frame (literal or from
    /// the nested recursive `iglob(dirname, dironly=True)`).
    fn iglob_next_parent(&mut self, _py: &PyToken<'_>, idx: usize) -> Result<Option<Vec<u8>>, u64> {
        // Literal source: take the single dirname once.
        {
            let GlobFrame::Iglob { dirs, .. } = &mut self.stack[idx] else {
                unreachable!()
            };
            if let GlobDirSource::Literal(slot) = dirs {
                return Ok(slot.take());
            }
        }
        // Recursive source: drive the nested state one path. We temporarily
        // move the boxed nested state out to satisfy the borrow checker, then
        // put it back (it owns its own frame stack across calls).
        let mut nested = {
            let GlobFrame::Iglob { dirs, .. } = &mut self.stack[idx] else {
                unreachable!()
            };
            match dirs {
                GlobDirSource::Recursive(b) => std::mem::replace(
                    b.as_mut(),
                    GlobIterState {
                        stack: Vec::new(),
                        cfg: glob_cfg_clone(&self.cfg, false),
                        leading_skip_done: true,
                    },
                ),
                GlobDirSource::Literal(_) => unreachable!(),
            }
        };
        let produced = nested.next_path(_py)?;
        let GlobFrame::Iglob { dirs, .. } = &mut self.stack[idx] else {
            unreachable!()
        };
        if let GlobDirSource::Recursive(b) = dirs {
            *b.as_mut() = nested;
        }
        Ok(produced)
    }
}

/// Clone the per-walk config (deep-clones the `dir_fd` arg). `skip` overrides
/// `skip_leading_empty` for nested dir-expansion walks (which never skip).
fn glob_cfg_clone(cfg: &GlobIterConfig, skip: bool) -> GlobIterConfig {
    GlobIterConfig {
        coordinate: cfg.coordinate,
        root_dir: cfg.root_dir.clone(),
        dir_fd: cfg.dir_fd.clone(),
        recursive: cfg.recursive,
        include_hidden: cfg.include_hidden,
        bytes_mode: cfg.bytes_mode,
        sep: cfg.sep,
        skip_leading_empty: skip,
    }
}

/// Build the initial `Iglob`/`Names` frame for `pathname` (mirrors the head of
/// `glob_iglob_text`, including the no-magic literal short-circuit).
fn glob_build_iglob_frame(
    _py: &PyToken<'_>,
    pathname: &[u8],
    cfg: &GlobIterConfig,
    dironly: bool,
) -> Result<GlobFrame, u64> {
    let sep = cfg.sep;
    let (dirname, basename) = glob_split_path(_py, pathname, sep, cfg.bytes_mode, cfg.coordinate)?;

    if !molt_runtime_platform::path_text::glob_has_magic_raw(pathname) {
        // Non-magic: bounded literal existence check.
        let names: Vec<Vec<u8>> = if !basename.is_empty() {
            let full = match cfg.root_dir.as_deref() {
                Some(root) => {
                    glob_join_path(_py, root, pathname, sep, cfg.bytes_mode, cfg.coordinate)?
                }
                None => pathname.to_vec(),
            };
            if glob_lexists_text(_py, &full, &cfg.dir_fd, cfg.bytes_mode, sep)? {
                vec![pathname.to_vec()]
            } else {
                Vec::new()
            }
        } else {
            let full_dir = match cfg.root_dir.as_deref() {
                Some(root) => {
                    glob_join_path(_py, root, &dirname, sep, cfg.bytes_mode, cfg.coordinate)?
                }
                None => dirname.clone(),
            };
            if glob_is_dir_text(_py, &full_dir, &cfg.dir_fd, cfg.bytes_mode, sep)? {
                vec![pathname.to_vec()]
            } else {
                Vec::new()
            }
        };
        return Ok(GlobFrame::Names(GlobNameCursor { names, idx: 0 }));
    }

    let basename_recursive = cfg.recursive && basename == b"**";
    let kind = if molt_runtime_platform::path_text::glob_has_magic_raw(&basename) {
        if basename_recursive {
            GlobInDir::Glob2
        } else {
            GlobInDir::Glob1
        }
    } else {
        GlobInDir::Glob0
    };

    let dirs = if dirname.is_empty() {
        // basename globbed directly in root_dir (represented by literal "").
        GlobDirSource::Literal(Some(Vec::new()))
    } else if dirname != pathname && molt_runtime_platform::path_text::glob_has_magic_raw(&dirname)
    {
        // Magic dirname: parents come from a nested streaming dir-expansion.
        let inner = GlobIterState::new(_py, &dirname, glob_cfg_clone(cfg, false), true)?;
        GlobDirSource::Recursive(Box::new(inner))
    } else {
        GlobDirSource::Literal(Some(dirname.clone()))
    };

    Ok(GlobFrame::Iglob {
        kind,
        basename,
        dironly,
        dirs,
        cur_dir: None,
        cur_names: None,
    })
}

/// Build the boxed streaming state for `glob.iglob(...)`. Mirrors
/// `glob_matches_text`'s configuration; `skip_leading_empty` reproduces the
/// `iglob` leading-`next(it)` skip for `**`/empty patterns.
#[allow(clippy::too_many_arguments)]
pub(crate) fn glob_iter_new_state(
    _py: &PyToken<'_>,
    pathname: &[u8],
    root_dir: Option<&[u8]>,
    dir_fd: GlobDirFdArg,
    recursive: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Box<GlobIterState>, u64> {
    let skip_leading_empty = pathname.is_empty() || (recursive && pathname.starts_with(b"**"));
    let cfg = GlobIterConfig {
        coordinate: path_root_coordinate(_py, bytes_mode),
        root_dir: root_dir.map(<[u8]>::to_vec),
        dir_fd,
        recursive,
        include_hidden,
        bytes_mode,
        sep,
        skip_leading_empty,
    };
    let state = GlobIterState::new(_py, pathname, cfg, false)?;
    Ok(Box::new(state))
}

/// Advance a `TYPE_ID_GLOB_ITER` object's state by one path and return the
/// value bits (str or bytes per the walk's mode). Returns None-bits at
/// end-of-stream; raises (sets exception) on capability/dir_fd errors.
///
/// # Safety
/// `iter_ptr` must be a live `TYPE_ID_GLOB_ITER` object data pointer.
pub(crate) unsafe fn glob_iter_next_value(_py: &PyToken<'_>, iter_ptr: *mut u8) -> u64 {
    let state_ptr = unsafe { crate::object::glob_iter_state_ptr(iter_ptr) };
    if state_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let state = unsafe { &mut *state_ptr };
    let bytes_mode = state.cfg.bytes_mode;
    match state.next_path(_py) {
        Err(_bits) => MoltObject::none().bits(), // exception already pending
        Ok(None) => MoltObject::none().bits(),
        Ok(Some(path)) => {
            let ptr = if bytes_mode {
                alloc_bytes(_py, &path)
            } else {
                alloc_string(_py, &path)
            };
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    }
}

/// Allocate a `TYPE_ID_GLOB_ITER` object owning `state`. On allocation failure
/// the state is dropped and None-bits returned.
pub(crate) fn glob_iter_alloc_object(_py: &PyToken<'_>, state: Box<GlobIterState>) -> u64 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut GlobIterState>();
    let ptr = alloc_object(_py, total, TYPE_ID_GLOB_ITER);
    if ptr.is_null() {
        // `state` drops here, freeing the work-stack.
        return MoltObject::none().bits();
    }
    let state_ptr = Box::into_raw(state);
    unsafe {
        *(ptr as *mut *mut GlobIterState) = state_ptr;
    }
    MoltObject::from_ptr(ptr).bits()
}

#[cfg(unix)]
pub(crate) fn create_symlink_path(
    src: &std::path::Path,
    dst: &std::path::Path,
    _target_is_directory: bool,
) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
pub(crate) fn create_symlink_path(
    src: &std::path::Path,
    dst: &std::path::Path,
    target_is_directory: bool,
) -> std::io::Result<()> {
    if target_is_directory {
        std::os::windows::fs::symlink_dir(src, dst)
    } else {
        std::os::windows::fs::symlink_file(src, dst)
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn create_symlink_path(
    _src: &std::path::Path,
    _dst: &std::path::Path,
    _target_is_directory: bool,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "symlink is not supported on this host",
    ))
}

pub(crate) fn path_relpath_text(
    _py: &PyToken<'_>,
    path: &str,
    start: &str,
    sep: char,
) -> Result<String, u64> {
    if path.is_empty() {
        return Err(raise_exception::<_>(_py, "ValueError", "no path specified"));
    }
    let start_abs = path_abspath_text(_py, start, sep)?;
    let path_abs = path_abspath_text(_py, path, sep)?;
    let start_parts = start_abs
        .split(sep)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect::<Vec<_>>();
    let path_parts = path_abs
        .split(sep)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect::<Vec<_>>();
    let mut common = 0usize;
    let limit = start_parts.len().min(path_parts.len());
    while common < limit && start_parts[common] == path_parts[common] {
        common += 1;
    }
    let mut rel_parts: Vec<String> = Vec::new();
    for _ in common..start_parts.len() {
        rel_parts.push("..".to_string());
    }
    for part in &path_parts[common..] {
        rel_parts.push(part.clone());
    }
    if rel_parts.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(rel_parts.join(&sep.to_string()))
    }
}
