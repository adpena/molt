//! String API — PyUnicode_*, PyBytes_*.

use crate::abi_types::{Py_ssize_t, PyByteArrayObject, PyObject, PyVarObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::cmp::Ordering;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::ptr;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

fn unicode_range(len: usize, start: Py_ssize_t, end: Py_ssize_t) -> (usize, usize) {
    let len_i = len as Py_ssize_t;
    let mut lo = if start < 0 { start + len_i } else { start };
    let mut hi = if end < 0 { end + len_i } else { end };
    lo = lo.clamp(0, len_i);
    hi = hi.clamp(0, len_i);
    if hi < lo {
        hi = lo;
    }
    (lo as usize, hi as usize)
}

unsafe fn unicode_bytes(op: *mut PyObject) -> Option<&'static [u8]> {
    if op.is_null() {
        return None;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = bridge.pyobj_to_handle(op)?;
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let data = unsafe { (h.str_data)(bits, &raw mut len) };
    if data.is_null() {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(data, len) })
    }
}

fn replace_bytes(
    haystack: &[u8],
    needle: &[u8],
    replacement: &[u8],
    maxcount: Py_ssize_t,
) -> Vec<u8> {
    if maxcount == 0 {
        return haystack.to_vec();
    }
    let limit = if maxcount < 0 {
        usize::MAX
    } else {
        maxcount as usize
    };
    if needle.is_empty() {
        let mut out = Vec::with_capacity(haystack.len() + replacement.len());
        let mut count = 0usize;
        for index in 0..=haystack.len() {
            if count < limit {
                out.extend_from_slice(replacement);
                count += 1;
            }
            if index < haystack.len() {
                out.push(haystack[index]);
            }
        }
        return out;
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut cursor = 0usize;
    let mut count = 0usize;
    while cursor < haystack.len() {
        if count < limit && haystack[cursor..].starts_with(needle) {
            out.extend_from_slice(replacement);
            cursor += needle.len();
            count += 1;
        } else {
            out.push(haystack[cursor]);
            cursor += 1;
        }
    }
    out
}

fn compare_unicode_bytes(left: &[u8], right: &[u8]) -> c_int {
    match left.cmp(right) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

fn latin1_encode_utf8_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let code = ch as u32;
        if code > 0xff {
            return None;
        }
        out.push(code as u8);
    }
    Some(out)
}

fn compact_ascii_encoding_name(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .copied()
        .filter(|b| *b != b'-' && *b != b'_')
        .map(|b| b.to_ascii_lowercase())
        .collect()
}

fn encoding_name_matches(bytes: &[u8], aliases: &[&[u8]]) -> bool {
    let compacted = compact_ascii_encoding_name(bytes);
    aliases
        .iter()
        .any(|alias| compacted == compact_ascii_encoding_name(alias))
}

fn push_codepoint_utf8(out: &mut Vec<u8>, code: u32) -> Option<()> {
    let ch = char::from_u32(code)?;
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    Some(())
}

fn unicode_kind_data_to_utf8(
    kind: c_int,
    data: *const c_void,
    size: Py_ssize_t,
) -> Option<Vec<u8>> {
    if size < 0 || (data.is_null() && size != 0) {
        return None;
    }
    let size = size as usize;
    let mut out = Vec::with_capacity(size);
    unsafe {
        match kind {
            1 => {
                let src = std::slice::from_raw_parts(data.cast::<u8>(), size);
                for &unit in src {
                    push_codepoint_utf8(&mut out, unit as u32)?;
                }
            }
            2 => {
                let src = std::slice::from_raw_parts(data.cast::<u16>(), size);
                for &unit in src {
                    push_codepoint_utf8(&mut out, unit as u32)?;
                }
            }
            4 => {
                let src = std::slice::from_raw_parts(data.cast::<u32>(), size);
                for &unit in src {
                    push_codepoint_utf8(&mut out, unit)?;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

fn utf8_bytes_to_ucs4(bytes: &[u8]) -> Option<Vec<u32>> {
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.chars().map(|ch| ch as u32).collect())
}

fn unicode_range_contains(ranges: &[(u32, u32)], code: u32) -> bool {
    let mut lo = 0usize;
    let mut hi = ranges.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (start, end) = ranges[mid];
        if code < start {
            hi = mid;
        } else if code > end {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

#[allow(dead_code)]
mod unicode_digit_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_digit_ranges.rs"));

    pub(super) fn is_digit(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_DIGIT_RANGES, code)
    }
}

#[allow(dead_code)]
mod unicode_decimal_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_decimal_ranges.rs"));

    pub(super) fn is_decimal(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_DECIMAL_RANGES, code)
    }
}

#[allow(dead_code)]
mod unicode_numeric_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_numeric_ranges.rs"));

    pub(super) fn is_numeric(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_NUMERIC_RANGES, code)
    }
}

#[allow(dead_code)]
mod unicode_space_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_space_ranges.rs"));

    pub(super) fn is_space(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_SPACE_RANGES, code)
    }
}

#[allow(dead_code)]
mod unicode_printable_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_printable_ranges.rs"));

    pub(super) fn is_printable(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_PRINTABLE_RANGES, code)
    }
}

fn unicode_char(ch: u32) -> Option<char> {
    char::from_u32(ch)
}

fn unicode_general_category(ch: u32) -> Option<GeneralCategory> {
    Some(unicode_char(ch)?.general_category())
}

fn unicode_category_is_alpha(category: GeneralCategory) -> bool {
    matches!(
        category,
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

fn c_bool(value: bool) -> c_int {
    value as c_int
}

// ─── Unicode character predicates ─────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsLowercase(ch: u32) -> c_int {
    c_bool(unicode_char(ch).is_some_and(char::is_lowercase))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsUppercase(ch: u32) -> c_int {
    c_bool(unicode_char(ch).is_some_and(char::is_uppercase))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsTitlecase(ch: u32) -> c_int {
    c_bool(matches!(
        unicode_general_category(ch),
        Some(GeneralCategory::TitlecaseLetter)
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsWhitespace(ch: u32) -> c_int {
    c_bool(unicode_space_table::is_space(ch))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsLinebreak(ch: u32) -> c_int {
    c_bool(matches!(
        ch,
        0x000A | 0x000B | 0x000C | 0x000D | 0x001C | 0x001D | 0x001E | 0x0085 | 0x2028 | 0x2029
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsDecimalDigit(ch: u32) -> c_int {
    c_bool(unicode_decimal_table::is_decimal(ch))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsDigit(ch: u32) -> c_int {
    c_bool(unicode_digit_table::is_digit(ch))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsNumeric(ch: u32) -> c_int {
    c_bool(unicode_numeric_table::is_numeric(ch))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsPrintable(ch: u32) -> c_int {
    c_bool(unicode_printable_table::is_printable(ch))
}

#[unsafe(no_mangle)]
pub extern "C" fn _PyUnicode_IsAlpha(ch: u32) -> c_int {
    c_bool(unicode_general_category(ch).is_some_and(unicode_category_is_alpha))
}

// ─── PyUnicode ────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromString(s: *const c_char) -> *mut PyObject {
    if s.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(s).to_bytes() };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(bytes.as_ptr(), bytes.len()) };
    if bits == 0 {
        // Fallback: return a placeholder None handle so the caller doesn't crash.
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromStringAndSize(
    s: *const c_char,
    size: Py_ssize_t,
) -> *mut PyObject {
    if s.is_null() || size < 0 {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), size as usize) };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(bytes.as_ptr(), bytes.len()) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_New(size: Py_ssize_t, _maxchar: u32) -> *mut PyObject {
    if size < 0 {
        return ptr::null_mut();
    }
    let bytes = vec![b' '; size as usize];
    unsafe { PyUnicode_FromStringAndSize(bytes.as_ptr().cast(), size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_DecodeLatin1(
    s: *const c_char,
    size: Py_ssize_t,
    _errors: *const c_char,
) -> *mut PyObject {
    if s.is_null() || size < 0 {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), size as usize) };
    let text: String = bytes.iter().map(|byte| char::from(*byte)).collect();
    unsafe { PyUnicode_FromStringAndSize(text.as_ptr().cast(), text.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromOrdinal(ordinal: c_int) -> *mut PyObject {
    let Some(ch) = char::from_u32(ordinal as u32) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_ValueError,
                c"ordinal not in range".as_ptr(),
            );
        }
        return ptr::null_mut();
    };
    let mut bytes = [0u8; 4];
    let encoded = ch.encode_utf8(&mut bytes);
    unsafe { PyUnicode_FromStringAndSize(encoded.as_ptr().cast(), encoded.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8(op: *mut PyObject) -> *const c_char {
    if op.is_null() {
        return ptr::null();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return c"".as_ptr(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.str_data)(bits, &raw mut len) };
    if ptr.is_null() {
        c"".as_ptr()
    } else {
        ptr.cast()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8String(op: *mut PyObject) -> *mut PyObject {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        return ptr::null_mut();
    };
    unsafe { PyBytes_FromStringAndSize(bytes.as_ptr().cast(), bytes.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsASCIIString(op: *mut PyObject) -> *mut PyObject {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        return ptr::null_mut();
    };
    if !bytes.is_ascii() {
        return ptr::null_mut();
    }
    unsafe { PyBytes_FromStringAndSize(bytes.as_ptr().cast(), bytes.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsLatin1String(op: *mut PyObject) -> *mut PyObject {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        return ptr::null_mut();
    };
    let Some(encoded) = latin1_encode_utf8_bytes(bytes) else {
        return ptr::null_mut();
    };
    unsafe { PyBytes_FromStringAndSize(encoded.as_ptr().cast(), encoded.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8AndSize(
    op: *mut PyObject,
    size: *mut Py_ssize_t,
) -> *const c_char {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        return ptr::null();
    };
    if !size.is_null() {
        unsafe {
            *size = bytes.len() as Py_ssize_t;
        }
    }
    let ptr = unsafe { PyUnicode_AsUTF8(op) };
    ptr
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_GetLength(op: *mut PyObject) -> Py_ssize_t {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        return -1;
    };
    match std::str::from_utf8(bytes) {
        Ok(text) => text.chars().count() as Py_ssize_t,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PyUnicode_Type)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_CompareWithASCIIString(
    op: *mut PyObject,
    s: *const c_char,
) -> c_int {
    let obj_ptr = unsafe { PyUnicode_AsUTF8(op) };
    if obj_ptr.is_null() || s.is_null() {
        return -1;
    }
    unsafe {
        let a = CStr::from_ptr(obj_ptr).to_bytes();
        let b = CStr::from_ptr(s).to_bytes();
        compare_unicode_bytes(a, b)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromKindAndData(
    kind: c_int,
    buffer: *const c_void,
    size: Py_ssize_t,
) -> *mut PyObject {
    let Some(bytes) = unicode_kind_data_to_utf8(kind, buffer, size) else {
        return ptr::null_mut();
    };
    unsafe { PyUnicode_FromStringAndSize(bytes.as_ptr().cast(), bytes.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUCS4(
    unicode: *mut PyObject,
    target: *mut u32,
    targetsize: Py_ssize_t,
    copy_null: c_int,
) -> *mut u32 {
    if unicode.is_null() || target.is_null() || targetsize < 0 {
        return ptr::null_mut();
    }
    let Some(bytes) = (unsafe { unicode_bytes(unicode) }) else {
        return ptr::null_mut();
    };
    let Some(codepoints) = utf8_bytes_to_ucs4(bytes) else {
        return ptr::null_mut();
    };
    let required = codepoints.len() + usize::from(copy_null != 0);
    if (targetsize as usize) < required {
        return ptr::null_mut();
    }
    unsafe {
        for (index, codepoint) in codepoints.iter().copied().enumerate() {
            *target.add(index) = codepoint;
        }
        if copy_null != 0 {
            *target.add(codepoints.len()) = 0;
        }
    }
    target
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUCS4Copy(unicode: *mut PyObject) -> *mut u32 {
    let Some(bytes) = (unsafe { unicode_bytes(unicode) }) else {
        return ptr::null_mut();
    };
    let Some(codepoints) = utf8_bytes_to_ucs4(bytes) else {
        return ptr::null_mut();
    };
    let Some(units) = codepoints.len().checked_add(1) else {
        return ptr::null_mut();
    };
    let Some(bytes_len) = units.checked_mul(std::mem::size_of::<u32>()) else {
        return ptr::null_mut();
    };
    let out = unsafe { crate::api::memory::PyMem_Malloc(bytes_len) }.cast::<u32>();
    if out.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        for (index, codepoint) in codepoints.iter().copied().enumerate() {
            *out.add(index) = codepoint;
        }
        *out.add(codepoints.len()) = 0;
    }
    out
}

// ─── PyBytes ──────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Compare(left: *mut PyObject, right: *mut PyObject) -> c_int {
    let Some(left_bytes) = (unsafe { unicode_bytes(left) }) else {
        return -1;
    };
    let Some(right_bytes) = (unsafe { unicode_bytes(right) }) else {
        return -1;
    };
    compare_unicode_bytes(left_bytes, right_bytes)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Tailmatch(
    str_obj: *mut PyObject,
    substr: *mut PyObject,
    start: Py_ssize_t,
    end: Py_ssize_t,
    direction: c_int,
) -> Py_ssize_t {
    let Some(text) = (unsafe { unicode_bytes(str_obj) }) else {
        return -1;
    };
    let Some(needle) = (unsafe { unicode_bytes(substr) }) else {
        return -1;
    };
    let (lo, hi) = unicode_range(text.len(), start, end);
    let window = &text[lo..hi];
    if direction < 0 {
        window.starts_with(needle) as Py_ssize_t
    } else {
        window.ends_with(needle) as Py_ssize_t
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Replace(
    str_obj: *mut PyObject,
    substr: *mut PyObject,
    repl: *mut PyObject,
    maxcount: Py_ssize_t,
) -> *mut PyObject {
    let Some(text) = (unsafe { unicode_bytes(str_obj) }) else {
        return ptr::null_mut();
    };
    let Some(needle) = (unsafe { unicode_bytes(substr) }) else {
        return ptr::null_mut();
    };
    let Some(replacement) = (unsafe { unicode_bytes(repl) }) else {
        return ptr::null_mut();
    };
    let out = replace_bytes(text, needle, replacement, maxcount);
    unsafe { PyUnicode_FromStringAndSize(out.as_ptr().cast(), out.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Substring(
    str_obj: *mut PyObject,
    start: Py_ssize_t,
    end: Py_ssize_t,
) -> *mut PyObject {
    let Some(bytes) = (unsafe { unicode_bytes(str_obj) }) else {
        return ptr::null_mut();
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return ptr::null_mut();
    };
    let (lo, hi) = unicode_range(text.chars().count(), start, end);
    let out: String = text.chars().skip(lo).take(hi - lo).collect();
    unsafe { PyUnicode_FromStringAndSize(out.as_ptr().cast(), out.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_FromStringAndSize(
    s: *const c_char,
    len: Py_ssize_t,
) -> *mut PyObject {
    if len < 0 {
        return ptr::null_mut();
    }
    let data = if s.is_null() {
        vec![0u8; len as usize]
    } else {
        unsafe { std::slice::from_raw_parts(s.cast::<u8>(), len as usize).to_vec() }
    };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_bytes)(data.as_ptr(), data.len()) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_FromString(s: *const c_char) -> *mut PyObject {
    if s.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(s).to_bytes() };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_bytes)(bytes.as_ptr(), bytes.len()) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AsStringAndSize(
    op: *mut PyObject,
    buf: *mut *mut c_char,
    length: *mut Py_ssize_t,
) -> c_int {
    if op.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if ptr.is_null() {
        if !buf.is_null() {
            unsafe {
                *buf = ptr::null_mut();
            }
        }
        if !length.is_null() {
            unsafe {
                *length = 0;
            }
        }
        return -1;
    }
    if !buf.is_null() {
        unsafe {
            *buf = ptr as *mut c_char;
        }
    }
    if !length.is_null() {
        unsafe {
            *length = len as Py_ssize_t;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PyBytes_Type)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AS_STRING(op: *mut PyObject) -> *mut c_char {
    if op.is_null() {
        return ptr::null_mut();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let data = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if data.is_null() {
        ptr::null_mut()
    } else {
        data.cast_mut().cast()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AsString(op: *mut PyObject) -> *mut c_char {
    unsafe { PyBytes_AS_STRING(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyBytes_Size(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Size(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if ptr.is_null() { -1 } else { len as Py_ssize_t }
}

// ─── Additional PyUnicode functions ──────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Concat(
    left: *mut PyObject,
    right: *mut PyObject,
) -> *mut PyObject {
    let Some(left_s) = (unsafe { unicode_bytes(left) }) else {
        return ptr::null_mut();
    };
    let Some(right_s) = (unsafe { unicode_bytes(right) }) else {
        return ptr::null_mut();
    };
    let mut combined = Vec::with_capacity(left_s.len() + right_s.len());
    combined.extend_from_slice(left_s);
    combined.extend_from_slice(right_s);
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(combined.as_ptr(), combined.len()) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Join(
    separator: *mut PyObject,
    seq: *mut PyObject,
) -> *mut PyObject {
    // Minimal: return empty string — full join requires iterating seq.
    let _ = (separator, seq);
    unsafe { PyUnicode_FromString(c"".as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Contains(
    container: *mut PyObject,
    element: *mut PyObject,
) -> c_int {
    let Some(c_bytes) = (unsafe { unicode_bytes(container) }) else {
        return -1;
    };
    let Some(e_bytes) = (unsafe { unicode_bytes(element) }) else {
        return -1;
    };
    if e_bytes.is_empty() {
        return 1;
    }
    for window in c_bytes.windows(e_bytes.len()) {
        if window == e_bytes {
            return 1;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FindChar(
    unicode: *mut PyObject,
    ch: u32,
    start: Py_ssize_t,
    end: Py_ssize_t,
    direction: c_int,
) -> Py_ssize_t {
    let Some(bytes) = (unsafe { unicode_bytes(unicode) }) else {
        return -2;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return -1;
    };
    let Some(target) = char::from_u32(ch) else {
        return -1;
    };
    let chars: Vec<char> = text.chars().collect();
    let (lo, hi) = unicode_range(chars.len(), start, end);
    if direction >= 0 {
        for (offset, candidate) in chars[lo..hi].iter().enumerate() {
            if *candidate == target {
                return (lo + offset) as Py_ssize_t;
            }
        }
    } else {
        for index in (lo..hi).rev() {
            if chars[index] == target {
                return index as Py_ssize_t;
            }
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Decode(
    s: *const c_char,
    size: Py_ssize_t,
    _encoding: *const c_char,
    _errors: *const c_char,
) -> *mut PyObject {
    // Assume UTF-8 encoding — the common case.
    unsafe { PyUnicode_FromStringAndSize(s, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_DecodeUTF8(
    s: *const c_char,
    size: Py_ssize_t,
    _errors: *const c_char,
) -> *mut PyObject {
    unsafe { PyUnicode_FromStringAndSize(s, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromEncodedObject(
    obj: *mut PyObject,
    _encoding: *const c_char,
    _errors: *const c_char,
) -> *mut PyObject {
    if obj.is_null() {
        return ptr::null_mut();
    }
    if unsafe { PyUnicode_Check(obj) } != 0 {
        unsafe { crate::api::refcount::Py_INCREF(obj) };
        return obj;
    }
    if unsafe { PyBytes_Check(obj) } != 0 {
        let mut data: *mut c_char = ptr::null_mut();
        let mut len: Py_ssize_t = 0;
        if unsafe { PyBytes_AsStringAndSize(obj, &raw mut data, &raw mut len) } != 0 {
            return ptr::null_mut();
        }
        return unsafe { PyUnicode_FromStringAndSize(data.cast_const(), len) };
    }
    unsafe { crate::api::typeobj::PyObject_Str(obj) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsEncodedString(
    unicode: *mut PyObject,
    encoding: *const c_char,
    _errors: *const c_char,
) -> *mut PyObject {
    let Some(bytes) = (unsafe { unicode_bytes(unicode) }) else {
        return ptr::null_mut();
    };
    let encoding_bytes = if encoding.is_null() {
        b"utf-8".as_slice()
    } else {
        unsafe { CStr::from_ptr(encoding) }.to_bytes()
    };
    let encoded = if encoding_name_matches(encoding_bytes, &[b"utf8", b"utf-8"]) {
        Some(bytes.to_vec())
    } else if encoding_name_matches(encoding_bytes, &[b"ascii", b"us-ascii"]) {
        bytes.is_ascii().then(|| bytes.to_vec())
    } else if encoding_name_matches(
        encoding_bytes,
        &[
            b"latin1",
            b"latin-1",
            b"latin_1",
            b"iso8859-1",
            b"iso-8859-1",
        ],
    ) {
        latin1_encode_utf8_bytes(bytes)
    } else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_LookupError,
                c"unknown encoding".as_ptr(),
            );
        }
        None
    };
    let Some(encoded) = encoded else {
        return ptr::null_mut();
    };
    unsafe { PyBytes_FromStringAndSize(encoded.as_ptr().cast(), encoded.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_InternInPlace(_p: *mut *mut PyObject) {
    // Interning is a no-op in the bridge — strings are already de-duped by
    // Molt's string allocator when hooks are active.
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_InternFromString(s: *const c_char) -> *mut PyObject {
    unsafe { PyUnicode_FromString(s) }
}

unsafe fn unicode_format_arg(args: *mut PyObject, index: &mut Py_ssize_t) -> *mut PyObject {
    if args.is_null() {
        return ptr::null_mut();
    }
    if unsafe { crate::api::sequences::PyTuple_Check(args) } != 0 {
        let arg = unsafe { crate::api::sequences::PyTuple_GetItem(args, *index) };
        if !arg.is_null() {
            *index += 1;
        }
        return arg;
    }
    if *index == 0 {
        *index = 1;
        args
    } else {
        ptr::null_mut()
    }
}

unsafe fn unicode_format_object_bytes(arg: *mut PyObject, repr: bool) -> Option<Vec<u8>> {
    if let Some(bytes) = unsafe { unicode_bytes(arg) } {
        return Some(bytes.to_vec());
    }
    let rendered = if repr {
        unsafe { crate::api::typeobj::PyObject_Repr(arg) }
    } else {
        unsafe { crate::api::typeobj::PyObject_Str(arg) }
    };
    if rendered.is_null() {
        return None;
    }
    let text = unsafe { unicode_bytes(rendered) }.map(|bytes| bytes.to_vec());
    unsafe { crate::api::refcount::Py_DECREF(rendered) };
    text
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Format(
    format: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    let Some(format_bytes) = (unsafe { unicode_bytes(format) }) else {
        return ptr::null_mut();
    };
    let format_bytes = format_bytes.to_vec();
    let mut out = Vec::with_capacity(format_bytes.len());
    let mut cursor = 0usize;
    let mut arg_index: Py_ssize_t = 0;

    while cursor < format_bytes.len() {
        let ch = format_bytes[cursor];
        cursor += 1;
        if ch != b'%' {
            out.push(ch);
            continue;
        }
        if cursor >= format_bytes.len() {
            return ptr::null_mut();
        }
        if format_bytes[cursor] == b'%' {
            out.push(b'%');
            cursor += 1;
            continue;
        }
        if format_bytes[cursor] == b'(' {
            return ptr::null_mut();
        }
        while cursor < format_bytes.len()
            && matches!(format_bytes[cursor], b'#' | b'0' | b'-' | b' ' | b'+')
        {
            cursor += 1;
        }
        while cursor < format_bytes.len() && format_bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor < format_bytes.len() && format_bytes[cursor] == b'.' {
            cursor += 1;
            while cursor < format_bytes.len() && format_bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
        }
        if cursor >= format_bytes.len() {
            return ptr::null_mut();
        }
        let spec = format_bytes[cursor];
        cursor += 1;
        let arg = if matches!(spec, b's' | b'S' | b'r' | b'R' | b'd' | b'i') {
            unsafe { unicode_format_arg(args, &mut arg_index) }
        } else {
            ptr::null_mut()
        };
        if arg.is_null() {
            return ptr::null_mut();
        }
        match spec {
            b's' | b'S' => {
                let Some(text) = (unsafe { unicode_format_object_bytes(arg, false) }) else {
                    return ptr::null_mut();
                };
                out.extend_from_slice(&text);
            }
            b'r' | b'R' => {
                let Some(text) = (unsafe { unicode_format_object_bytes(arg, true) }) else {
                    return ptr::null_mut();
                };
                out.extend_from_slice(&text);
            }
            b'd' | b'i' => {
                let value = unsafe { crate::api::numbers::PyLong_AsLong(arg) };
                out.extend_from_slice(value.to_string().as_bytes());
            }
            _ => return ptr::null_mut(),
        }
    }

    unsafe { PyUnicode_FromStringAndSize(out.as_ptr().cast(), out.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_GET_LENGTH(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyUnicode_GetLength(op) }
}

// ─── Additional PyBytes functions ────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Concat(bytes: *mut *mut PyObject, newpart: *mut PyObject) {
    if bytes.is_null() || unsafe { *bytes }.is_null() || newpart.is_null() {
        return;
    }
    // Simplified: just keep the original bytes.
    // Full concat requires bytes_data + alloc_bytes.
    let _ = newpart;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_FromStringAndSize(
    s: *const c_char,
    len: Py_ssize_t,
) -> *mut PyObject {
    if len < 0 {
        return ptr::null_mut();
    }
    let size = len as usize;
    let Some(alloc) = size.checked_add(1) else {
        return ptr::null_mut();
    };
    let bytes = unsafe { crate::api::memory::PyMem_Calloc(1, alloc) }.cast::<c_char>();
    if bytes.is_null() {
        return ptr::null_mut();
    }
    if !s.is_null() && size != 0 {
        unsafe {
            ptr::copy_nonoverlapping(s, bytes, size);
        }
    }
    unsafe {
        *bytes.add(size) = 0;
    }
    let obj = Box::new(PyByteArrayObject {
        ob_base: PyVarObject {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut crate::abi_types::PyByteArray_Type,
            },
            ob_size: len,
        },
        ob_alloc: alloc as Py_ssize_t,
        ob_bytes: bytes,
        ob_start: bytes,
        ob_exports: 0,
    });
    Box::into_raw(obj).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    std::ptr::eq(ob_type, &raw const crate::abi_types::PyByteArray_Type) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_AsString(op: *mut PyObject) -> *mut c_char {
    if unsafe { PyByteArray_Check(op) } == 0 {
        return ptr::null_mut();
    }
    unsafe { (*op.cast::<PyByteArrayObject>()).ob_start }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_Size(op: *mut PyObject) -> Py_ssize_t {
    if unsafe { PyByteArray_Check(op) } == 0 {
        return -1;
    }
    unsafe { (*op.cast::<PyByteArrayObject>()).ob_base.ob_size }
}

pub unsafe extern "C" fn molt_bytearray_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let obj = op.cast::<PyByteArrayObject>();
    unsafe {
        if !(*obj).ob_bytes.is_null() {
            crate::api::memory::PyMem_Free((*obj).ob_bytes.cast());
            (*obj).ob_bytes = ptr::null_mut();
            (*obj).ob_start = ptr::null_mut();
        }
        drop(Box::from_raw(obj));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        _PyUnicode_IsAlpha, _PyUnicode_IsDecimalDigit, _PyUnicode_IsDigit, _PyUnicode_IsLinebreak,
        _PyUnicode_IsLowercase, _PyUnicode_IsNumeric, _PyUnicode_IsPrintable,
        _PyUnicode_IsTitlecase, _PyUnicode_IsUppercase, _PyUnicode_IsWhitespace,
        encoding_name_matches, latin1_encode_utf8_bytes, unicode_kind_data_to_utf8,
        utf8_bytes_to_ucs4,
    };

    #[test]
    fn latin1_encoder_preserves_ascii_and_latin1_scalar_values() {
        assert_eq!(
            latin1_encode_utf8_bytes(b"caf\xc3\xa9").as_deref(),
            Some(&b"caf\xe9"[..])
        );
    }

    #[test]
    fn latin1_encoder_rejects_non_latin1_scalar_values() {
        assert!(latin1_encode_utf8_bytes(b"\xe2\x82\xac").is_none());
    }

    #[test]
    fn latin1_encoder_rejects_invalid_utf8() {
        assert!(latin1_encode_utf8_bytes(b"\xff").is_none());
    }

    #[test]
    fn encoding_aliases_match_case_dash_and_underscore_variants() {
        assert!(encoding_name_matches(b"UTF_8", &[b"utf-8"]));
        assert!(encoding_name_matches(b"latin-1", &[b"latin1"]));
        assert!(encoding_name_matches(b"ISO_8859-1", &[b"iso8859-1"]));
        assert!(!encoding_name_matches(b"cp1252", &[b"latin1"]));
    }

    #[test]
    fn unicode_kind_data_imports_latin1_as_utf8() {
        let src = [b'c', b'a', b'f', 0xe9];
        let out = unicode_kind_data_to_utf8(1, src.as_ptr().cast(), src.len() as isize).unwrap();
        assert_eq!(std::str::from_utf8(&out).unwrap(), "caf\u{e9}");
    }

    #[test]
    fn unicode_kind_data_imports_ucs2_and_ucs4() {
        let ucs2 = [0x03c0u16, 0x002bu16, 0x0031u16];
        let out = unicode_kind_data_to_utf8(2, ucs2.as_ptr().cast(), ucs2.len() as isize).unwrap();
        assert_eq!(std::str::from_utf8(&out).unwrap(), "\u{3c0}+1");

        let ucs4 = [0x1f642u32];
        let out = unicode_kind_data_to_utf8(4, ucs4.as_ptr().cast(), ucs4.len() as isize).unwrap();
        assert_eq!(std::str::from_utf8(&out).unwrap(), "\u{1f642}");
    }

    #[test]
    fn unicode_kind_data_rejects_invalid_scalars() {
        let surrogate = [0xd800u16];
        assert!(unicode_kind_data_to_utf8(2, surrogate.as_ptr().cast(), 1).is_none());
        let too_large = [0x110000u32];
        assert!(unicode_kind_data_to_utf8(4, too_large.as_ptr().cast(), 1).is_none());
    }

    #[test]
    fn utf8_to_ucs4_counts_scalar_values() {
        assert_eq!(
            utf8_bytes_to_ucs4("a\u{3c0}\u{1f642}".as_bytes()).unwrap(),
            [0x61, 0x03c0, 0x1f642]
        );
        assert!(utf8_bytes_to_ucs4(b"\xff").is_none());
    }

    #[test]
    fn unicode_predicates_match_cpython_category_boundaries() {
        assert_eq!(_PyUnicode_IsAlpha('A' as u32), 1);
        assert_eq!(_PyUnicode_IsAlpha('é' as u32), 1);
        assert_eq!(_PyUnicode_IsAlpha('一' as u32), 1);
        assert_eq!(_PyUnicode_IsAlpha('1' as u32), 0);

        assert_eq!(_PyUnicode_IsUppercase('A' as u32), 1);
        assert_eq!(_PyUnicode_IsLowercase('é' as u32), 1);
        assert_eq!(_PyUnicode_IsTitlecase('\u{01c5}' as u32), 1);
        assert_eq!(_PyUnicode_IsTitlecase('A' as u32), 0);
    }

    #[test]
    fn unicode_numeric_predicates_preserve_decimal_digit_numeric_split() {
        assert_eq!(_PyUnicode_IsDecimalDigit('0' as u32), 1);
        assert_eq!(_PyUnicode_IsDecimalDigit('\u{0660}' as u32), 1);
        assert_eq!(_PyUnicode_IsDecimalDigit('\u{00b2}' as u32), 0);

        assert_eq!(_PyUnicode_IsDigit('\u{00b2}' as u32), 1);
        assert_eq!(_PyUnicode_IsDigit('\u{2160}' as u32), 0);

        assert_eq!(_PyUnicode_IsNumeric('\u{2160}' as u32), 1);
        assert_eq!(_PyUnicode_IsNumeric('一' as u32), 1);
        assert_eq!(_PyUnicode_IsNumeric('A' as u32), 0);
    }

    #[test]
    fn unicode_space_printable_linebreak_and_invalid_scalar_predicates() {
        assert_eq!(_PyUnicode_IsWhitespace(' ' as u32), 1);
        assert_eq!(_PyUnicode_IsWhitespace('\u{2003}' as u32), 1);
        assert_eq!(_PyUnicode_IsWhitespace('A' as u32), 0);

        assert_eq!(_PyUnicode_IsLinebreak('\n' as u32), 1);
        assert_eq!(_PyUnicode_IsLinebreak('\u{2028}' as u32), 1);
        assert_eq!(_PyUnicode_IsLinebreak(' ' as u32), 0);

        assert_eq!(_PyUnicode_IsPrintable('A' as u32), 1);
        assert_eq!(_PyUnicode_IsPrintable('\n' as u32), 0);
        assert_eq!(_PyUnicode_IsAlpha(0x11_0000), 0);
        assert_eq!(_PyUnicode_IsLowercase(0x11_0000), 0);
    }
}
