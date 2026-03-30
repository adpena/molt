// Path, glob, and filesystem utility functions.
// Extracted from io.rs for compilation-unit size reduction and tree shaking.

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::Sign;
use num_traits::ToPrimitive;
use std::io::ErrorKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathFlavor {
    Str,
    Bytes,
}

pub(crate) fn path_from_bits_with_flavor(
    _py: &PyToken<'_>,
    file_bits: u64,
) -> Result<(std::path::PathBuf, PathFlavor), String> {
    let obj = obj_from_bits(file_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok((std::path::PathBuf::from(text), PathFlavor::Str));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    let path = std::ffi::OsString::from_vec(bytes.to_vec());
                    return Ok((std::path::PathBuf::from(path), PathFlavor::Bytes));
                }
                #[cfg(windows)]
                {
                    let path = std::str::from_utf8(bytes)
                        .map_err(|_| "open path bytes must be utf-8".to_string())?;
                    return Ok((std::path::PathBuf::from(path), PathFlavor::Bytes));
                }
            }
            let fspath_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.fspath_name, b"__fspath__");
            if let Some(call_bits) = attr_lookup_ptr(_py, ptr, fspath_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return Err("open failed".to_string());
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(text) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(_py, res_bits);
                    return Ok((std::path::PathBuf::from(text), PathFlavor::Str));
                }
                if let Some(res_ptr) = res_obj.as_ptr()
                    && object_type_id(res_ptr) == TYPE_ID_BYTES
                {
                    let len = bytes_len(res_ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                    #[cfg(unix)]
                    {
                        use std::os::unix::ffi::OsStringExt;
                        let path = std::ffi::OsString::from_vec(bytes.to_vec());
                        dec_ref_bits(_py, res_bits);
                        return Ok((std::path::PathBuf::from(path), PathFlavor::Bytes));
                    }
                    #[cfg(windows)]
                    {
                        let path = std::str::from_utf8(bytes)
                            .map_err(|_| "open path bytes must be utf-8".to_string())?;
                        dec_ref_bits(_py, res_bits);
                        return Ok((std::path::PathBuf::from(path), PathFlavor::Bytes));
                    }
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                dec_ref_bits(_py, res_bits);
                let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
                return Err(format!(
                    "expected {obj_type}.__fspath__() to return str or bytes, not {res_type}"
                ));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
    Err(format!(
        "expected str, bytes or os.PathLike object, not {obj_type}"
    ))
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
        let fspath_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.fspath_name, b"__fspath__");
        if let Some(call_bits) = attr_lookup_ptr(_py, ptr, fspath_name_bits) {
            let res_bits = call_callable0(_py, call_bits);
            dec_ref_bits(_py, call_bits);
            if exception_pending(_py) {
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
            dec_ref_bits(_py, res_bits);
            let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
            let msg =
                format!("expected {obj_type}.__fspath__() to return str or bytes, not {res_type}");
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
        }
    }

    let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
    let msg = format!("expected str, bytes or os.PathLike object, not {obj_type}");
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

pub(crate) fn path_from_bits(
    _py: &PyToken<'_>,
    file_bits: u64,
) -> Result<std::path::PathBuf, String> {
    path_from_bits_with_flavor(_py, file_bits).map(|(path, _flavor)| path)
}

pub(crate) fn filesystem_encoding() -> &'static str {
    "utf-8"
}

pub(crate) fn filesystem_encode_errors() -> &'static str {
    #[cfg(windows)]
    {
        "surrogatepass"
    }
    #[cfg(not(windows))]
    {
        "surrogateescape"
    }
}

pub(crate) fn path_sep_char() -> char {
    std::path::MAIN_SEPARATOR
}

#[cfg(unix)]
fn bytes_text_from_raw(raw: &[u8]) -> String {
    raw.iter().map(|byte| char::from(*byte)).collect()
}

#[cfg(unix)]
pub(crate) fn raw_from_bytes_text(text: &str) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let code = ch as u32;
        if code > 0xFF {
            return None;
        }
        out.push(code as u8);
    }
    Some(out)
}

#[cfg(not(unix))]
fn bytes_text_from_raw(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

#[cfg(not(unix))]
pub(crate) fn raw_from_bytes_text(text: &str) -> Option<Vec<u8>> {
    Some(text.as_bytes().to_vec())
}

#[cfg(unix)]
fn path_text_with_flavor(path: &std::path::Path, flavor: PathFlavor) -> String {
    if flavor == PathFlavor::Bytes {
        use std::os::unix::ffi::OsStrExt;
        return bytes_text_from_raw(path.as_os_str().as_bytes());
    }
    path.to_string_lossy().into_owned()
}

#[cfg(not(unix))]
fn path_text_with_flavor(path: &std::path::Path, _flavor: PathFlavor) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn path_string_with_flavor_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
) -> Result<(String, PathFlavor), u64> {
    match path_from_bits_with_flavor(_py, bits) {
        Ok((path, flavor)) => Ok((path_text_with_flavor(path.as_path(), flavor), flavor)),
        Err(msg) => Err(raise_exception::<_>(_py, "TypeError", &msg)),
    }
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

pub(crate) fn path_sequence_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    label: &str,
) -> Result<Vec<String>, u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        let msg = format!("{label} must be tuple or list, not NoneType");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("{label} must be tuple or list, not {type_name}");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    }
    let elems = unsafe { seq_vec_ref(ptr) };
    let mut out = Vec::with_capacity(elems.len());
    for item_bits in elems {
        let value = path_string_from_bits(_py, *item_bits)?;
        out.push(value);
    }
    Ok(out)
}

pub(crate) fn path_join_text(mut base: String, part: &str, sep: char) -> String {
    if part.starts_with(sep) {
        return part.to_string();
    }
    if !base.is_empty() && !base.ends_with(sep) {
        base.push(sep);
    }
    base.push_str(part);
    base
}

pub(crate) fn path_join_many_text(mut base: String, parts: &[String], sep: char) -> String {
    for part in parts {
        base = path_join_text(base, part, sep);
    }
    base
}

/// Join two raw byte paths using the given separator byte (posixpath-style).
pub(crate) fn path_join_raw(base: &[u8], part: &[u8], sep: u8) -> Vec<u8> {
    if part.first() == Some(&sep) {
        return part.to_vec();
    }
    let mut out = base.to_vec();
    if !out.is_empty() && out.last() != Some(&sep) {
        out.push(sep);
    }
    out.extend_from_slice(part);
    out
}

/// Extract raw byte slice from a bytes object bits value.  Returns `None` if not bytes.
pub(crate) fn bytes_slice_from_bits(bits: u64) -> Option<Vec<u8>> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
        return None;
    }
    let len = unsafe { bytes_len(ptr) };
    let data = unsafe { std::slice::from_raw_parts(bytes_data(ptr), len) };
    Some(data.to_vec())
}

/// Extract a sequence of raw byte vecs from a tuple/list of bytes objects.
pub(crate) fn bytes_sequence_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    label: &str,
) -> Result<Vec<Vec<u8>>, u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        let msg = format!("{label} must be tuple or list, not NoneType");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("{label} must be tuple or list, not {type_name}");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    }
    let elems = unsafe { seq_vec_ref(ptr) };
    let mut out = Vec::with_capacity(elems.len());
    for item_bits in elems {
        match bytes_slice_from_bits(*item_bits) {
            Some(raw) => out.push(raw),
            None => {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "join: expected bytes for path component",
                ));
            }
        }
    }
    Ok(out)
}

pub(crate) fn alloc_string_list_bits(_py: &PyToken<'_>, values: &[String]) -> u64 {
    let mut out_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = alloc_string(_py, value.as_bytes());
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

pub(crate) fn alloc_path_list_bits(_py: &PyToken<'_>, values: &[String], bytes_out: bool) -> u64 {
    let mut out_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = if bytes_out {
            match raw_from_bytes_text(value) {
                Some(raw) => alloc_bytes(_py, raw.as_slice()),
                None => alloc_bytes(_py, value.as_bytes()),
            }
        } else {
            alloc_string(_py, value.as_bytes())
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

pub(crate) fn path_basename_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return String::new();
    }
    let stripped = path.trim_end_matches(sep);
    if stripped.is_empty() {
        return sep.to_string();
    }
    match stripped.rfind(sep) {
        Some(idx) => stripped[idx + sep.len_utf8()..].to_string(),
        None => stripped.to_string(),
    }
}

pub(crate) fn path_dirname_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return String::new();
    }
    let stripped = path.trim_end_matches(sep);
    if stripped.is_empty() {
        return sep.to_string();
    }
    match stripped.rfind(sep) {
        Some(0) => sep.to_string(),
        Some(idx) => stripped[..idx].to_string(),
        None => String::new(),
    }
}

pub(crate) fn path_splitext_text(path: &str, sep: char) -> (String, String) {
    let base = path_basename_text(path, sep);
    if !base.contains('.') || base == "." || base == ".." {
        return (path.to_string(), String::new());
    }
    let idx = match base.rfind('.') {
        Some(idx) => idx,
        None => return (path.to_string(), String::new()),
    };
    let root_len = path.len().saturating_sub(base.len()) + idx;
    let root = path[..root_len].to_string();
    let ext = base[idx..].to_string();
    (root, ext)
}

pub(crate) fn path_name_text(path: &str, sep: char) -> String {
    let parts = path_parts_text(path, sep);
    if parts.is_empty() {
        return String::new();
    }
    let sep_s = sep.to_string();
    if parts.len() == 1 && parts[0] == sep_s {
        return String::new();
    }
    parts.last().cloned().unwrap_or_default()
}

pub(crate) fn path_suffix_text(path: &str, sep: char) -> String {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return String::new();
    }
    let (_, suffix) = path_splitext_text(&name, sep);
    suffix
}

pub(crate) fn path_suffixes_text(path: &str, sep: char) -> Vec<String> {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return Vec::new();
    }
    let mut suffixes: Vec<String> = Vec::new();
    let mut stem = name;
    loop {
        let (next_stem, suffix) = path_splitext_text(&stem, sep);
        if suffix.is_empty() {
            break;
        }
        suffixes.push(suffix);
        stem = next_stem;
    }
    suffixes.reverse();
    suffixes
}

pub(crate) fn path_stem_text(path: &str, sep: char) -> String {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return String::new();
    }
    let (stem, _) = path_splitext_text(&name, sep);
    stem
}

pub(crate) fn path_as_uri_text(path: &str, sep: char) -> Result<String, String> {
    if !path.starts_with(sep) {
        return Err("relative path can't be expressed as a file URI".to_string());
    }
    let mut posix = if sep == '/' {
        path.to_string()
    } else {
        path.replace(sep, "/")
    };
    if !posix.starts_with('/') {
        posix.insert(0, '/');
    }
    Ok(format!("file://{posix}"))
}

pub(crate) fn path_normpath_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let absolute = path.starts_with(sep);
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(sep) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.last().is_some_and(|last| *last != "..") {
                parts.pop();
            } else if !absolute {
                parts.push(part);
            }
            continue;
        }
        parts.push(part);
    }
    let sep_s = sep.to_string();
    if absolute {
        let normalized = format!("{sep}{}", parts.join(&sep_s));
        if normalized.is_empty() {
            sep.to_string()
        } else {
            normalized
        }
    } else {
        let normalized = parts.join(&sep_s);
        if normalized.is_empty() {
            ".".to_string()
        } else {
            normalized
        }
    }
}

pub(crate) fn path_abspath_text(_py: &PyToken<'_>, path: &str, sep: char) -> Result<String, u64> {
    let mut current = path.to_string();
    if !path_isabs_text(&current, sep) {
        if !has_capability(_py, "fs.read") {
            return Err(raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read capability",
            ));
        }
        let cwd = match std::env::current_dir() {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(err) => {
                let msg = err.to_string();
                let bits = match err.kind() {
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::NotADirectory => {
                        raise_exception::<_>(_py, "NotADirectoryError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                };
                return Err(bits);
            }
        };
        current = path_join_text(cwd, &current, sep);
    }
    Ok(path_normpath_text(&current, sep))
}

pub(crate) fn path_resolve_text(
    _py: &PyToken<'_>,
    path: &str,
    sep: char,
    strict: bool,
) -> Result<String, u64> {
    let absolute = path_abspath_text(_py, path, sep)?;
    if !has_capability(_py, "fs.read") {
        if strict {
            return Err(raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read capability",
            ));
        }
        return Ok(absolute);
    }
    let resolved = std::path::Path::new(&absolute);
    match std::fs::canonicalize(resolved) {
        Ok(path_buf) => Ok(path_normpath_text(&path_buf.to_string_lossy(), sep)),
        Err(err)
            if !strict && matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
        {
            Ok(path_normpath_text(&absolute, sep))
        }
        Err(err) => {
            let msg = err.to_string();
            let bits = match err.kind() {
                ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                ErrorKind::PermissionDenied => raise_exception::<_>(_py, "PermissionError", &msg),
                ErrorKind::NotADirectory => raise_exception::<_>(_py, "NotADirectoryError", &msg),
                _ => raise_exception::<_>(_py, "OSError", &msg),
            };
            Err(bits)
        }
    }
}

pub(crate) fn path_parts_text(path: &str, sep: char) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let (drive, root, tail) = path_splitroot_text(path, sep);
    if !drive.is_empty() || !root.is_empty() {
        out.push(format!("{drive}{root}"));
    }
    for part in tail.split(sep) {
        if part.is_empty() || part == "." {
            continue;
        }
        out.push(part.to_string());
    }
    out
}

pub(crate) fn path_compare_text(lhs: &str, rhs: &str, sep: char) -> i64 {
    let lhs_parts = path_parts_text(lhs, sep);
    let rhs_parts = path_parts_text(rhs, sep);
    use std::cmp::Ordering;
    match lhs_parts.cmp(&rhs_parts) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

pub(crate) fn path_parents_text(path: &str, sep: char) -> Vec<String> {
    let (drive, root, tail) = path_splitroot_text(path, sep);
    let anchor = format!("{drive}{root}");
    let tail_parts = tail
        .split(sep)
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToOwned::to_owned)
        .collect::<Vec<String>>();
    if tail_parts.is_empty() {
        return Vec::new();
    }
    let sep_s = sep.to_string();
    let mut out: Vec<String> = Vec::new();
    let mut idx = tail_parts.len();
    while idx > 0 {
        idx -= 1;
        if idx == 0 {
            if anchor.is_empty() {
                out.push(".".to_string());
            } else {
                out.push(anchor.clone());
            }
            continue;
        }
        let prefix = tail_parts[..idx].join(&sep_s);
        if anchor.is_empty() {
            out.push(prefix);
        } else {
            out.push(format!("{anchor}{prefix}"));
        }
    }
    out
}

pub(crate) fn path_isabs_text(path: &str, sep: char) -> bool {
    #[cfg(windows)]
    {
        let text = path.replace('/', "\\");
        if text.starts_with("\\\\") || text.starts_with('\\') {
            return true;
        }
        let bytes = text.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
        false
    }
    #[cfg(not(windows))]
    {
        path.starts_with(sep)
    }
}

fn path_match_simple_pattern(name: &str, pat: &str) -> bool {
    type GlobCharClassParse = (Vec<char>, Vec<(char, char)>, bool, usize);

    fn parse_char_class(pat: &[char], mut idx: usize) -> Option<GlobCharClassParse> {
        if idx >= pat.len() || pat[idx] != '[' {
            return None;
        }
        idx += 1;
        if idx >= pat.len() {
            return None;
        }

        let mut negate = false;
        if pat[idx] == '!' {
            negate = true;
            idx += 1;
        }
        if idx >= pat.len() {
            return None;
        }

        let mut singles: Vec<char> = Vec::new();
        let mut ranges: Vec<(char, char)> = Vec::new();

        if pat[idx] == ']' {
            singles.push(']');
            idx += 1;
        }

        while idx < pat.len() && pat[idx] != ']' {
            if idx + 2 < pat.len() && pat[idx + 1] == '-' && pat[idx + 2] != ']' {
                let start = pat[idx];
                let end = pat[idx + 2];
                if start <= end {
                    ranges.push((start, end));
                }
                idx += 3;
                continue;
            }
            singles.push(pat[idx]);
            idx += 1;
        }
        if idx >= pat.len() || pat[idx] != ']' {
            return None;
        }
        Some((singles, ranges, negate, idx + 1))
    }

    fn char_class_hit(ch: char, singles: &[char], ranges: &[(char, char)], negate: bool) -> bool {
        let mut hit = singles.contains(&ch);
        if !hit {
            hit = ranges.iter().any(|(start, end)| *start <= ch && ch <= *end);
        }
        if negate { !hit } else { hit }
    }

    let name_chars: Vec<char> = name.chars().collect();
    let pat_chars: Vec<char> = pat.chars().collect();
    let mut pi: usize = 0;
    let mut ni: usize = 0;
    let mut star_idx: Option<usize> = None;
    let mut matched_from_star: usize = 0;

    while ni < name_chars.len() {
        if pi < pat_chars.len() && pat_chars[pi] == '*' {
            while pi < pat_chars.len() && pat_chars[pi] == '*' {
                pi += 1;
            }
            if pi == pat_chars.len() {
                return true;
            }
            star_idx = Some(pi);
            matched_from_star = ni;
            continue;
        }
        if pi < pat_chars.len()
            && pat_chars[pi] == '['
            && let Some((singles, ranges, negate, next_idx)) = parse_char_class(&pat_chars, pi)
        {
            let hit = char_class_hit(name_chars[ni], &singles, &ranges, negate);
            if hit {
                pi = next_idx;
                ni += 1;
                continue;
            }
            if let Some(star) = star_idx {
                matched_from_star += 1;
                ni = matched_from_star;
                pi = star;
                continue;
            }
            return false;
        }
        if pi < pat_chars.len() && (pat_chars[pi] == '?' || pat_chars[pi] == name_chars[ni]) {
            pi += 1;
            ni += 1;
            continue;
        }
        if let Some(star) = star_idx {
            matched_from_star += 1;
            ni = matched_from_star;
            pi = star;
            continue;
        }
        return false;
    }
    while pi < pat_chars.len() && pat_chars[pi] == '*' {
        pi += 1;
    }
    pi == pat_chars.len()
}

pub(crate) fn path_match_text(path: &str, pattern: &str, sep: char) -> bool {
    #[cfg(windows)]
    let pattern = pattern.replace('/', "\\");
    #[cfg(not(windows))]
    let pattern = pattern.to_string();
    let absolute = pattern.starts_with(sep);
    if absolute && !path.starts_with(sep) {
        return false;
    }
    let pat = if absolute {
        pattern.trim_start_matches(sep)
    } else {
        pattern.as_str()
    };
    let path_trimmed = path.trim_start_matches(sep);
    if !pat.contains(sep) && !pat.contains('/') {
        let name = path_basename_text(path, sep);
        if pat == "*" {
            return !name.is_empty();
        }
        if pat.starts_with("*.") && pat.matches('*').count() == 1 && !pat.contains('?') {
            return name.ends_with(&pat[1..]);
        }
        return path_match_simple_pattern(&name, pat);
    }

    fn split_components(text: &str, sep: char) -> Vec<&str> {
        text.split(sep)
            .filter(|part| !part.is_empty() && *part != ".")
            .collect()
    }

    fn match_components(path_parts: &[&str], pat_parts: &[&str]) -> bool {
        fn inner(path_parts: &[&str], pat_parts: &[&str], pi: usize, pj: usize) -> bool {
            if pj >= pat_parts.len() {
                return pi >= path_parts.len();
            }
            let pat = pat_parts[pj];
            if pat == "**" {
                if inner(path_parts, pat_parts, pi, pj + 1) {
                    return true;
                }
                return pi < path_parts.len() && inner(path_parts, pat_parts, pi + 1, pj);
            }
            if pi >= path_parts.len() {
                return false;
            }
            path_match_simple_pattern(path_parts[pi], pat)
                && inner(path_parts, pat_parts, pi + 1, pj + 1)
        }
        inner(path_parts, pat_parts, 0, 0)
    }

    let pat_parts = split_components(pat, sep);
    let path_parts = split_components(path_trimmed, sep);
    if absolute {
        return match_components(&path_parts, &pat_parts);
    }
    for start in 0..=path_parts.len() {
        if match_components(&path_parts[start..], &pat_parts) {
            return true;
        }
    }
    false
}

pub(crate) fn glob_has_magic_text(pathname: &str) -> bool {
    pathname
        .as_bytes()
        .iter()
        .any(|ch| matches!(*ch, b'*' | b'?' | b'['))
}

#[derive(Clone, Debug)]
pub(crate) enum GlobDirFdArg {
    None,
    Int(i64),
    PathLike {
        path: String,
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
    match path_string_with_flavor_from_bits(_py, dir_fd_bits) {
        Ok((path, flavor)) => {
            #[cfg(windows)]
            let path = path.replace('/', "\\");
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

#[cfg(unix)]
fn glob_text_to_path(text: &str, bytes_mode: bool) -> std::path::PathBuf {
    if bytes_mode && let Some(raw) = raw_from_bytes_text(text) {
        use std::os::unix::ffi::OsStringExt;
        return std::path::PathBuf::from(std::ffi::OsString::from_vec(raw));
    }
    std::path::PathBuf::from(text)
}

#[cfg(not(unix))]
fn glob_text_to_path(text: &str, _bytes_mode: bool) -> std::path::PathBuf {
    std::path::PathBuf::from(text)
}

#[cfg(unix)]
fn glob_dir_entry_name_text(name: &std::ffi::OsStr, bytes_mode: bool) -> String {
    if bytes_mode {
        use std::os::unix::ffi::OsStrExt;
        return bytes_text_from_raw(name.as_bytes());
    }
    name.to_string_lossy().into_owned()
}

#[cfg(not(unix))]
fn glob_dir_entry_name_text(name: &std::ffi::OsStr, _bytes_mode: bool) -> String {
    name.to_string_lossy().into_owned()
}

#[cfg(all(unix, target_vendor = "apple"))]
pub(crate) fn glob_dir_fd_root_text(fd: i64, bytes_mode: bool) -> Option<String> {
    if fd < 0 {
        return None;
    }
    // Apple targets do not provide a stable /proc/self/fd lane; use fcntl(F_GETPATH).
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    let rc = unsafe {
        libc::fcntl(
            fd as libc::c_int,
            libc::F_GETPATH,
            buf.as_mut_ptr() as *mut libc::c_char,
        )
    };
    if rc != -1
        && let Some(nul_idx) = buf.iter().position(|byte| *byte == 0)
        && nul_idx > 0
    {
        return Some(bytes_text_from_raw(&buf[..nul_idx]));
    }
    for candidate in [format!("/proc/self/fd/{fd}"), format!("/dev/fd/{fd}")] {
        if let Ok(path) = std::fs::read_link(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
        if let Ok(path) = std::fs::canonicalize(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
    }
    None
}

#[cfg(all(unix, not(target_vendor = "apple")))]
pub(crate) fn glob_dir_fd_root_text(fd: i64, bytes_mode: bool) -> Option<String> {
    if fd < 0 {
        return None;
    }
    for candidate in [format!("/proc/self/fd/{fd}"), format!("/dev/fd/{fd}")] {
        if let Ok(path) = std::fs::read_link(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
        if let Ok(path) = std::fs::canonicalize(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
    }
    None
}

#[cfg(windows)]
pub(crate) fn glob_dir_fd_root_text(fd: i64, _bytes_mode: bool) -> Option<String> {
    if fd < 0 {
        return None;
    }
    let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
    if handle == -1 {
        return None;
    }
    windows_path_from_handle(handle as *mut std::ffi::c_void)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn glob_dir_fd_root_text(fd: i64, bytes_mode: bool) -> Option<String> {
    if fd < 0 {
        return None;
    }
    for candidate in [format!("/proc/self/fd/{fd}"), format!("/dev/fd/{fd}")] {
        if let Ok(path) = std::fs::read_link(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
        if let Ok(path) = std::fs::canonicalize(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
    }
    None
}

#[cfg(all(not(unix), not(windows), not(target_arch = "wasm32")))]
pub(crate) fn glob_dir_fd_root_text(_fd: i64, _bytes_mode: bool) -> Option<String> {
    None
}

fn glob_is_hidden_text(name: &str) -> bool {
    name.starts_with('.')
}

fn glob_split_path_text(pathname: &str, sep: char) -> (String, String) {
    let (drive, root, tail) = path_splitroot_text(pathname, sep);
    if tail.is_empty() {
        return (format!("{drive}{root}"), String::new());
    }

    let mut head = String::new();
    let mut base = tail.clone();
    if let Some(idx) = tail.rfind(sep) {
        head = tail[..idx + sep.len_utf8()].to_string();
        base = tail[idx + sep.len_utf8()..].to_string();
    }

    if !head.is_empty() {
        let all_sep = head.chars().all(|ch| ch == sep);
        if !all_sep {
            head = head.trim_end_matches(sep).to_string();
        }
    }

    let dirname = format!("{drive}{root}{head}");
    (dirname, base)
}

fn glob_join_text(base: &str, part: &str, sep: char) -> String {
    if base.is_empty() {
        return part.to_string();
    }
    if path_isabs_text(part, sep) {
        return part.to_string();
    }
    #[cfg(windows)]
    {
        let (part_drive, _part_root, _part_tail) = path_splitroot_text(part, sep);
        if !part_drive.is_empty() {
            return part.to_string();
        }
    }
    path_join_text(base.to_string(), part, sep)
}

fn glob_lexists_text(
    _py: &PyToken<'_>,
    path: &str,
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    sep: char,
) -> Result<bool, u64> {
    if path.is_empty() {
        return Ok(false);
    }
    let resolved = match dir_fd {
        GlobDirFdArg::None => path.to_string(),
        GlobDirFdArg::Int(fd) => {
            if path_isabs_text(path, sep) {
                path.to_string()
            } else if let Some(root) = glob_dir_fd_root_text(*fd, bytes_mode) {
                glob_join_text(&root, path, sep)
            } else {
                return Ok(false);
            }
        }
        GlobDirFdArg::PathLike { type_name, .. } | GlobDirFdArg::BadType { type_name } => {
            return Err(glob_dir_fd_type_error_bits(_py, type_name));
        }
    };
    let resolved_path = glob_text_to_path(&resolved, bytes_mode);
    Ok(std::fs::symlink_metadata(resolved_path).is_ok())
}

fn glob_is_dir_text(
    _py: &PyToken<'_>,
    path: &str,
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    sep: char,
) -> Result<bool, u64> {
    if path.is_empty() {
        return Ok(false);
    }
    let resolved = match dir_fd {
        GlobDirFdArg::None => path.to_string(),
        GlobDirFdArg::Int(fd) => {
            if path_isabs_text(path, sep) {
                path.to_string()
            } else if let Some(root) = glob_dir_fd_root_text(*fd, bytes_mode) {
                glob_join_text(&root, path, sep)
            } else {
                return Ok(false);
            }
        }
        GlobDirFdArg::PathLike { type_name, .. } | GlobDirFdArg::BadType { type_name } => {
            return Err(glob_dir_fd_type_error_bits(_py, type_name));
        }
    };
    let resolved_path = glob_text_to_path(&resolved, bytes_mode);
    Ok(std::fs::metadata(resolved_path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false))
}

struct GlobListdirResult {
    names: Vec<String>,
    names_are_bytes: bool,
}

fn glob_listdir_text(
    _py: &PyToken<'_>,
    dirname: &str,
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<GlobListdirResult, u64> {
    let target: String;
    let mut target_bytes_mode = bytes_mode;
    let arg_is_bytes;

    match dir_fd {
        GlobDirFdArg::None => {
            if dirname.is_empty() {
                target = ".".to_string();
                arg_is_bytes = bytes_mode;
            } else {
                target = dirname.to_string();
                arg_is_bytes = bytes_mode;
            }
        }
        GlobDirFdArg::Int(fd) => {
            if dirname.is_empty() {
                if let Some(root) = glob_dir_fd_root_text(*fd, bytes_mode) {
                    target = root;
                } else if *fd == -1 {
                    // CPython's scandir(-1) can expose CWD on some hosts.
                    target = ".".to_string();
                } else {
                    return Ok(GlobListdirResult {
                        names: Vec::new(),
                        names_are_bytes: bytes_mode,
                    });
                }
                arg_is_bytes = false;
            } else if path_isabs_text(dirname, sep) {
                target = dirname.to_string();
                arg_is_bytes = bytes_mode;
            } else if let Some(root) = glob_dir_fd_root_text(*fd, bytes_mode) {
                target = glob_join_text(&root, dirname, sep);
                arg_is_bytes = bytes_mode;
            } else {
                return Ok(GlobListdirResult {
                    names: Vec::new(),
                    names_are_bytes: bytes_mode,
                });
            }
        }
        GlobDirFdArg::PathLike {
            path,
            flavor,
            type_name,
        } => {
            if !dirname.is_empty() {
                return Err(glob_dir_fd_type_error_bits(_py, type_name));
            }
            target = path.clone();
            target_bytes_mode = *flavor == PathFlavor::Bytes;
            arg_is_bytes = *flavor == PathFlavor::Bytes;
        }
        GlobDirFdArg::BadType { type_name } => {
            if dirname.is_empty() {
                return Err(glob_scandir_type_error_bits(_py, type_name));
            }
            return Err(glob_dir_fd_type_error_bits(_py, type_name));
        }
    }

    let names_are_bytes = bytes_mode || arg_is_bytes;
    let target_path = glob_text_to_path(&target, target_bytes_mode);
    let mut out: Vec<String> = Vec::new();
    let iter = match std::fs::read_dir(target_path) {
        Ok(iter) => iter,
        Err(_) => {
            return Ok(GlobListdirResult {
                names: out,
                names_are_bytes,
            });
        }
    };
    for entry_res in iter {
        let Ok(entry) = entry_res else {
            continue;
        };
        if dironly {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
        }
        out.push(glob_dir_entry_name_text(
            entry.file_name().as_os_str(),
            names_are_bytes,
        ));
    }
    Ok(GlobListdirResult {
        names: out,
        names_are_bytes,
    })
}

#[allow(clippy::too_many_arguments)]
fn glob1_text(
    _py: &PyToken<'_>,
    dirname: &str,
    pattern: &str,
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
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
    if !(pattern.starts_with('.') || include_hidden) {
        names.retain(|name| !glob_is_hidden_text(name));
    }
    names.retain(|name| path_match_simple_pattern(name, pattern));
    Ok(names)
}

fn glob0_text(
    _py: &PyToken<'_>,
    dirname: &str,
    basename: &str,
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    if !basename.is_empty() {
        let full = glob_join_text(dirname, basename, sep);
        if glob_lexists_text(_py, &full, dir_fd, bytes_mode, sep)? {
            return Ok(vec![basename.to_string()]);
        }
        return Ok(Vec::new());
    }
    if glob_is_dir_text(_py, dirname, dir_fd, bytes_mode, sep)? {
        return Ok(vec![String::new()]);
    }
    Ok(Vec::new())
}

fn glob_rlistdir_text(
    _py: &PyToken<'_>,
    dirname: &str,
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let mut out: Vec<String> = Vec::new();
    let listed = glob_listdir_text(_py, dirname, dir_fd, dironly, bytes_mode, sep)?;
    let names = listed.names;
    for name in names {
        if !include_hidden && glob_is_hidden_text(&name) {
            continue;
        }
        out.push(name.clone());
        let path = if dirname.is_empty() {
            name.clone()
        } else {
            glob_join_text(dirname, &name, sep)
        };
        for child in
            glob_rlistdir_text(_py, &path, dir_fd, dironly, include_hidden, bytes_mode, sep)?
        {
            out.push(glob_join_text(&name, &child, sep));
        }
    }
    Ok(out)
}

fn glob2_text(
    _py: &PyToken<'_>,
    dirname: &str,
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let mut out: Vec<String> = Vec::new();
    if dirname.is_empty() || glob_is_dir_text(_py, dirname, dir_fd, bytes_mode, sep)? {
        out.push(String::new());
    }
    out.extend(glob_rlistdir_text(
        _py,
        dirname,
        dir_fd,
        dironly,
        include_hidden,
        bytes_mode,
        sep,
    )?);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn glob_iglob_text(
    _py: &PyToken<'_>,
    pathname: &str,
    root_dir: Option<&str>,
    dir_fd: &GlobDirFdArg,
    recursive: bool,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let (dirname, basename) = glob_split_path_text(pathname, sep);
    if !glob_has_magic_text(pathname) {
        if !basename.is_empty() {
            let full = match root_dir {
                Some(root) => glob_join_text(root, pathname, sep),
                None => pathname.to_string(),
            };
            if glob_lexists_text(_py, &full, dir_fd, bytes_mode, sep)? {
                return Ok(vec![pathname.to_string()]);
            }
        } else {
            let full_dir = match root_dir {
                Some(root) => glob_join_text(root, &dirname, sep),
                None => dirname.clone(),
            };
            if glob_is_dir_text(_py, &full_dir, dir_fd, bytes_mode, sep)? {
                return Ok(vec![pathname.to_string()]);
            }
        }
        return Ok(Vec::new());
    }

    if dirname.is_empty() {
        let in_dir = root_dir.unwrap_or("");
        if recursive && basename == "**" {
            return glob2_text(
                _py,
                in_dir,
                dir_fd,
                dironly,
                include_hidden,
                bytes_mode,
                sep,
            );
        }
        return glob1_text(
            _py,
            in_dir,
            &basename,
            dir_fd,
            dironly,
            include_hidden,
            bytes_mode,
            sep,
        );
    }

    let mut dirs: Vec<String> = Vec::new();
    if dirname != pathname && glob_has_magic_text(&dirname) {
        dirs = glob_iglob_text(
            _py,
            &dirname,
            root_dir,
            dir_fd,
            recursive,
            true,
            include_hidden,
            bytes_mode,
            sep,
        )?;
    } else {
        dirs.push(dirname.clone());
    }

    let basename_has_magic = glob_has_magic_text(&basename);
    let basename_recursive = recursive && basename == "**";
    let mut out: Vec<String> = Vec::new();
    for parent in dirs {
        let search_dir = match root_dir {
            Some(root) => glob_join_text(root, &parent, sep),
            None => parent.clone(),
        };
        let names = if basename_has_magic {
            if basename_recursive {
                glob2_text(
                    _py,
                    &search_dir,
                    dir_fd,
                    dironly,
                    include_hidden,
                    bytes_mode,
                    sep,
                )?
            } else {
                glob1_text(
                    _py,
                    &search_dir,
                    &basename,
                    dir_fd,
                    dironly,
                    include_hidden,
                    bytes_mode,
                    sep,
                )?
            }
        } else {
            glob0_text(_py, &search_dir, &basename, dir_fd, bytes_mode, sep)?
        };
        for name in names {
            out.push(glob_join_text(&parent, &name, sep));
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn glob_matches_text(
    _py: &PyToken<'_>,
    pathname: &str,
    root_dir: Option<&str>,
    dir_fd: &GlobDirFdArg,
    recursive: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let mut out = glob_iglob_text(
        _py,
        pathname,
        root_dir,
        dir_fd,
        recursive,
        false,
        include_hidden,
        bytes_mode,
        sep,
    )?;
    if (pathname.is_empty() || (recursive && pathname.starts_with("**")))
        && out.first().is_some_and(String::is_empty)
    {
        out.remove(0);
    }
    Ok(out)
}

pub(crate) fn glob_escape_text(pathname: &str, sep: char) -> String {
    let (drive, root, tail) = path_splitroot_text(pathname, sep);
    let mut out = String::new();
    out.push_str(&drive);
    out.push_str(&root);
    for ch in tail.chars() {
        if matches!(ch, '*' | '?' | '[') {
            out.push('[');
            out.push(ch);
            out.push(']');
        } else {
            out.push(ch);
        }
    }
    out
}

fn glob_regex_escape_char(out: &mut String, ch: char) {
    if matches!(
        ch,
        '.' | '^' | '$' | '+' | '{' | '}' | '(' | ')' | '|' | '\\' | '[' | ']'
    ) {
        out.push('\\');
    }
    out.push(ch);
}

fn glob_regex_escape_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        glob_regex_escape_char(&mut out, ch);
    }
    out
}

fn glob_split_on_seps(pat: &str, seps: &[char]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in pat.chars() {
        if seps.contains(&ch) {
            out.push(cur);
            cur = String::new();
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

type GlobTranslateCharClassParse = (Vec<char>, Vec<(char, char)>, bool, usize);

fn glob_translate_parse_char_class(
    pat: &[char],
    mut idx: usize,
) -> Option<GlobTranslateCharClassParse> {
    if idx >= pat.len() || pat[idx] != '[' {
        return None;
    }
    idx += 1;
    if idx >= pat.len() {
        return None;
    }

    let mut negate = false;
    if pat[idx] == '!' {
        negate = true;
        idx += 1;
    }
    if idx >= pat.len() {
        return None;
    }

    let mut singles: Vec<char> = Vec::new();
    let mut ranges: Vec<(char, char)> = Vec::new();

    if pat[idx] == ']' {
        singles.push(']');
        idx += 1;
    }
    while idx < pat.len() && pat[idx] != ']' {
        if idx + 2 < pat.len() && pat[idx + 1] == '-' && pat[idx + 2] != ']' {
            let start = pat[idx];
            let end = pat[idx + 2];
            if start <= end {
                ranges.push((start, end));
            }
            idx += 3;
            continue;
        }
        singles.push(pat[idx]);
        idx += 1;
    }
    if idx >= pat.len() || pat[idx] != ']' {
        return None;
    }
    Some((singles, ranges, negate, idx + 1))
}

fn glob_translate_char_class(
    singles: Vec<char>,
    ranges: Vec<(char, char)>,
    negate: bool,
) -> String {
    let mut out = String::new();
    out.push('[');
    if negate {
        out.push('^');
    }
    for ch in singles {
        if matches!(ch, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(ch);
    }
    for (start, end) in ranges {
        if matches!(start, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(start);
        out.push('-');
        if matches!(end, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(end);
    }
    out.push(']');
    out
}

fn glob_translate_segment(part: &str, star_expr: &str, ques_expr: &str) -> String {
    let chars: Vec<char> = part.chars().collect();
    let mut out = String::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        match chars[idx] {
            '*' => out.push_str(star_expr),
            '?' => out.push_str(ques_expr),
            '[' => {
                if let Some((singles, ranges, negate, next_idx)) =
                    glob_translate_parse_char_class(&chars, idx)
                {
                    out.push_str(&glob_translate_char_class(singles, ranges, negate));
                    idx = next_idx;
                    continue;
                } else {
                    out.push_str("\\[");
                }
            }
            ch => glob_regex_escape_char(&mut out, ch),
        }
        idx += 1;
    }
    out
}

fn glob_default_seps_text() -> String {
    #[cfg(windows)]
    {
        "\\/".to_string()
    }
    #[cfg(not(windows))]
    {
        "/".to_string()
    }
}

pub(crate) fn glob_translate_text(
    pat: &str,
    recursive: bool,
    include_hidden: bool,
    seps: Option<&str>,
) -> String {
    let seps_text = if let Some(raw) = seps {
        if raw.is_empty() {
            glob_default_seps_text()
        } else {
            raw.to_string()
        }
    } else {
        glob_default_seps_text()
    };
    let sep_chars: Vec<char> = seps_text.chars().collect();
    let escaped_seps = glob_regex_escape_text(&seps_text);
    let any_sep = if sep_chars.len() > 1 {
        format!("[{escaped_seps}]")
    } else {
        escaped_seps.clone()
    };
    let not_sep = format!("[^{escaped_seps}]");
    let (one_last_segment, one_segment, any_segments, any_last_segments) = if include_hidden {
        let one_last_segment = format!("{not_sep}+");
        let one_segment = format!("{one_last_segment}{any_sep}");
        let any_segments = format!("(?:.+{any_sep})?");
        let any_last_segments = ".*".to_string();
        (
            one_last_segment,
            one_segment,
            any_segments,
            any_last_segments,
        )
    } else {
        let one_last_segment = format!("[^{escaped_seps}.]{not_sep}*");
        let one_segment = format!("{one_last_segment}{any_sep}");
        let any_segments = format!("(?:{one_segment})*");
        let any_last_segments = format!("{any_segments}(?:{one_last_segment})?");
        (
            one_last_segment,
            one_segment,
            any_segments,
            any_last_segments,
        )
    };

    let parts = glob_split_on_seps(pat, &sep_chars);
    let last_part_idx = parts.len().saturating_sub(1);
    let mut results: Vec<String> = Vec::new();
    for (idx, part) in parts.iter().enumerate() {
        if part == "*" {
            if idx < last_part_idx {
                results.push(one_segment.clone());
            } else {
                results.push(one_last_segment.clone());
            }
        } else if recursive && part == "**" {
            if idx < last_part_idx {
                if parts[idx + 1] != "**" {
                    results.push(any_segments.clone());
                }
            } else {
                results.push(any_last_segments.clone());
            }
        } else {
            if !part.is_empty() {
                if !include_hidden && part.chars().next().is_some_and(|ch| ch == '*' || ch == '?') {
                    results.push(r"(?!\.)".to_string());
                }
                let star_expr = format!("{not_sep}*");
                results.push(glob_translate_segment(part, &star_expr, &not_sep));
            }
            if idx < last_part_idx {
                results.push(any_sep.clone());
            }
        }
    }
    let body = results.join("");
    format!("(?s:{body})\\Z")
}

fn glob_split_components(text: &str, sep: char) -> Vec<String> {
    text.split(sep)
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToOwned::to_owned)
        .collect()
}

fn glob_walk(
    dir: &std::path::Path,
    rel_parts: &mut Vec<String>,
    pat_parts: &[String],
    pi: usize,
    sep: char,
    out: &mut Vec<String>,
) -> std::io::Result<()> {
    let sep_s = sep.to_string();
    if pi >= pat_parts.len() {
        if !rel_parts.is_empty() {
            out.push(rel_parts.join(&sep_s));
        }
        return Ok(());
    }
    let pat = &pat_parts[pi];
    if pat == "**" {
        glob_walk(dir, rel_parts, pat_parts, pi + 1, sep, out)?;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            rel_parts.push(name);
            glob_walk(&entry.path(), rel_parts, pat_parts, pi, sep, out)?;
            rel_parts.pop();
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path_match_simple_pattern(&name, pat) {
            continue;
        }
        let file_type = entry.file_type()?;
        rel_parts.push(name);
        if pi + 1 >= pat_parts.len() {
            out.push(rel_parts.join(&sep_s));
        } else if file_type.is_dir() {
            glob_walk(&entry.path(), rel_parts, pat_parts, pi + 1, sep, out)?;
        }
        rel_parts.pop();
    }
    Ok(())
}

pub(crate) fn path_glob_matches(
    dir: &std::path::Path,
    pattern: &str,
    sep: char,
) -> std::io::Result<Vec<String>> {
    #[cfg(windows)]
    let pattern = pattern.replace('/', "\\");
    #[cfg(not(windows))]
    let pattern = pattern.to_string();
    let pat_parts = glob_split_components(&pattern, sep);
    let mut matches: Vec<String> = Vec::new();
    if !pat_parts.is_empty() {
        let mut rel_parts: Vec<String> = Vec::new();
        glob_walk(dir, &mut rel_parts, &pat_parts, 0, sep, &mut matches)?;
    }
    Ok(matches)
}

pub(crate) fn raise_io_error_for_glob(_py: &PyToken<'_>, err: std::io::Error) -> u64 {
    let msg = err.to_string();
    match err.kind() {
        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
        ErrorKind::PermissionDenied => raise_exception::<_>(_py, "PermissionError", &msg),
        ErrorKind::NotADirectory => raise_exception::<_>(_py, "NotADirectoryError", &msg),
        _ => raise_exception::<_>(_py, "OSError", &msg),
    }
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

pub(crate) fn path_splitroot_text(path: &str, sep: char) -> (String, String, String) {
    #[cfg(windows)]
    {
        let text = path.replace('/', "\\");
        if text.is_empty() {
            return (String::new(), String::new(), String::new());
        }
        let mut drive = String::new();
        let mut root = String::new();
        let mut rest = text.as_str();
        if rest.starts_with("\\\\") {
            let unc = &rest[2..];
            let mut parts = unc.split('\\');
            let server = parts.next().unwrap_or_default();
            let share = parts.next().unwrap_or_default();
            if !server.is_empty() && !share.is_empty() {
                drive = format!("\\\\{server}\\{share}");
                let consumed = 2 + server.len() + 1 + share.len();
                rest = &rest[consumed..];
            }
        } else {
            let bytes = rest.as_bytes();
            if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
                drive = rest[..2].to_string();
                rest = &rest[2..];
            }
        }
        if rest.starts_with('\\') {
            root = sep.to_string();
            rest = rest.trim_start_matches('\\');
        }
        return (drive, root, rest.to_string());
    }
    #[cfg(not(windows))]
    {
        if path.is_empty() {
            return (String::new(), String::new(), String::new());
        }
        if path.starts_with("//") && !path.starts_with("///") {
            let tail = path.trim_start_matches('/').to_string();
            return (String::new(), "//".to_string(), tail);
        }
        if path.starts_with(sep) {
            return (
                String::new(),
                sep.to_string(),
                path.trim_start_matches(sep).to_string(),
            );
        }
        (String::new(), String::new(), path.to_string())
    }
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

pub(crate) fn path_relative_to_text(path: &str, base: &str, sep: char) -> Result<String, String> {
    let sep_s = sep.to_string();
    let target_parts = path_parts_text(path, sep);
    let base_parts = path_parts_text(base, sep);
    let target_abs = target_parts.first().is_some_and(|part| part == &sep_s);
    let base_abs = base_parts.first().is_some_and(|part| part == &sep_s);
    if (base_abs && !target_abs) || (!base_abs && target_abs) {
        return Err(format!("{path:?} is not in the subpath of {base:?}"));
    }
    if base_parts.len() > target_parts.len() {
        return Err(format!("{path:?} is not in the subpath of {base:?}"));
    }
    for (idx, part) in base_parts.iter().enumerate() {
        if target_parts.get(idx) != Some(part) {
            return Err(format!("{path:?} is not in the subpath of {base:?}"));
        }
    }
    let rel_parts = &target_parts[base_parts.len()..];
    if rel_parts.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(rel_parts.join(&sep_s))
    }
}

pub(crate) fn path_expandvars_with_lookup(
    path: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> String {
    if !path.contains('$') {
        return path.to_string();
    }
    let is_var_char = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let chars: Vec<char> = path.chars().collect();
    let mut out = String::with_capacity(path.len());
    let mut idx = 0usize;
    while idx < chars.len() {
        let ch = chars[idx];
        if ch != '$' {
            out.push(ch);
            idx += 1;
            continue;
        }
        if idx + 1 >= chars.len() {
            out.push('$');
            idx += 1;
            continue;
        }
        let next = chars[idx + 1];
        if next == '{' {
            let mut end = idx + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            if end >= chars.len() {
                for c in &chars[idx..] {
                    out.push(*c);
                }
                break;
            }
            let name: String = chars[idx + 2..end].iter().collect();
            if name.is_empty() {
                for c in &chars[idx..=end] {
                    out.push(*c);
                }
            } else if let Some(value) = lookup(&name) {
                out.push_str(&value);
            } else {
                for c in &chars[idx..=end] {
                    out.push(*c);
                }
            }
            idx = end + 1;
            continue;
        }
        if next == '$' {
            out.push('$');
            out.push('$');
            idx += 2;
            continue;
        }
        let start = idx + 1;
        let mut end = start;
        while end < chars.len() && is_var_char(chars[end]) {
            end += 1;
        }
        if end == start {
            out.push('$');
            idx += 1;
            continue;
        }
        let name: String = chars[start..end].iter().collect();
        if let Some(value) = lookup(&name) {
            out.push_str(&value);
        } else {
            for c in &chars[idx..end] {
                out.push(*c);
            }
        }
        idx = end;
    }
    out
}

pub(crate) fn path_expandvars_text(_py: &PyToken<'_>, path: &str) -> Result<String, u64> {
    let allowed = has_capability(_py, "env.read");
    audit_capability_decision("env.expandvars", "env.read", AuditArgs::None, allowed);
    if !allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing env.read capability",
        ));
    }
    Ok(path_expandvars_with_lookup(path, |name| {
        std::env::var(name).ok()
    }))
}
