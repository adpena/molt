//! String API — PyUnicode_*, PyBytes_*.

use crate::abi_types::{Py_ssize_t, PyByteArrayObject, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use std::cmp::Ordering;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::ptr;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

/// Fail-closed helper for str/bytes constructors whose runtime allocation
/// returned 0 (out of memory). CPython's `PyUnicode_*`/`PyBytes_*` constructors
/// return NULL with `MemoryError` set on allocation failure (Objects/unicodeobject.c,
/// Objects/bytesobject.c). The previous code fabricated a `MoltObject::none()`
/// handle, which reads to the C caller as a non-NULL success and masks the OOM.
unsafe fn str_alloc_failed() -> *mut PyObject {
    unsafe { crate::api::errors::PyErr_NoMemory() }
}

enum UnicodeCodecErrorObject<'a> {
    Bytes(&'a [u8]),
    Text(PythonStringBytes<'a>),
}

/// Construct a structured Unicode codec exception through the same five-field
/// authority as CPython: ``(encoding, object, start, end, reason)``.  The
/// runtime's Unicode error class constructor owns validation and publishes the
/// corresponding attributes; the C ABI must never collapse this payload into
/// the one message argument accepted by ordinary exceptions.
unsafe fn raise_unicode_codec_error(
    exc_type: *mut PyObject,
    encoding: &str,
    object: UnicodeCodecErrorObject<'_>,
    start: usize,
    end: usize,
    reason: &str,
) {
    let encoding_obj = unsafe {
        PyUnicode_FromStringAndSize(encoding.as_ptr().cast(), encoding.len() as Py_ssize_t)
    };
    let object_obj = match object {
        UnicodeCodecErrorObject::Bytes(bytes) => unsafe {
            PyBytes_FromStringAndSize(bytes.as_ptr().cast(), bytes.len() as Py_ssize_t)
        },
        UnicodeCodecErrorObject::Text(text) => unsafe { unicode_from_python_text(text) },
    };
    let start_obj = unsafe { crate::api::numbers::PyLong_FromSsize_t(start as Py_ssize_t) };
    let end_obj = unsafe { crate::api::numbers::PyLong_FromSsize_t(end as Py_ssize_t) };
    let reason_obj =
        unsafe { PyUnicode_FromStringAndSize(reason.as_ptr().cast(), reason.len() as Py_ssize_t) };
    let fields = [encoding_obj, object_obj, start_obj, end_obj, reason_obj];
    if fields.iter().any(|field| field.is_null()) {
        unsafe { crate::api::errors::release_preserving_error(&fields) };
        return;
    }
    let args = unsafe { crate::api::sequences::native_call_args(&fields) };
    unsafe { crate::api::errors::release_preserving_error(&fields) };
    if args.is_null() {
        return;
    }
    unsafe {
        crate::api::errors::PyErr_SetObject(exc_type, args);
        crate::api::errors::release_preserving_error(&[args]);
    }
}

unsafe fn raise_utf8_decode_error(bytes: &[u8], error: std::str::Utf8Error) {
    let start = error.valid_up_to();
    let (end, reason) = match error.error_len() {
        None => (bytes.len(), "unexpected end of data"),
        Some(error_len) => {
            let first = bytes.get(start).copied().unwrap_or_default();
            let reason = if (0xc2..=0xf4).contains(&first) {
                "invalid continuation byte"
            } else {
                "invalid start byte"
            };
            (start + error_len, reason)
        }
    };
    unsafe {
        raise_unicode_codec_error(
            (&raw mut crate::abi_types::PyExc_UnicodeDecodeError)
                .cast::<crate::abi_types::PyObject>(),
            "utf-8",
            UnicodeCodecErrorObject::Bytes(bytes),
            start,
            end,
            reason,
        )
    };
}

/// Internal Python text: UTF-8 scalar encodings plus individually encoded
/// surrogate code points. Unlike a UTF-16/WTF-8 normalizer this view preserves
/// adjacent high/low surrogates as two Python characters. Public UTF-8 decoding
/// must first use `str::from_utf8`; it must never admit bytes through this view.
#[derive(Clone, Copy)]
pub(crate) struct PythonStringBytes<'a>(&'a [u8]);

impl<'a> PythonStringBytes<'a> {
    pub(crate) fn from_utf8(text: &'a str) -> Self {
        Self(text.as_bytes())
    }

    pub(crate) fn from_bytes(bytes: &'a [u8]) -> Option<Self> {
        let mut remaining = bytes;
        loop {
            match std::str::from_utf8(remaining) {
                Ok(_) => return Some(Self(bytes)),
                Err(error) => {
                    remaining = &remaining[error.valid_up_to()..];
                    if remaining.len() < 3
                        || remaining[0] != 0xed
                        || !(0xa0..=0xbf).contains(&remaining[1])
                        || !(0x80..=0xbf).contains(&remaining[2])
                    {
                        return None;
                    }
                    remaining = &remaining[3..];
                }
            }
        }
    }

    pub(crate) fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    fn code_point_indices(self) -> impl Iterator<Item = (usize, u32)> + 'a {
        let mut offset = 0;
        std::iter::from_fn(move || {
            let first = *self.0.get(offset)?;
            let start = offset;
            let (width, mut code) = match first {
                0x00..=0x7f => (1, u32::from(first)),
                0xc2..=0xdf => (2, u32::from(first & 0x1f)),
                0xe0..=0xef => (3, u32::from(first & 0x0f)),
                _ => (4, u32::from(first & 0x07)),
            };
            // Admission proves each complete, minimally encoded code point.
            for byte in &self.0[start + 1..start + width] {
                code = (code << 6) | u32::from(byte & 0x3f);
            }
            offset += width;
            Some((start, code))
        })
    }

    pub(crate) fn code_points(self) -> impl Iterator<Item = u32> + 'a {
        self.code_point_indices().map(|(_, code)| code)
    }

    fn prefix_bytes(self, count: usize) -> &'a [u8] {
        let end = self
            .code_point_indices()
            .nth(count)
            .map_or(self.0.len(), |(i, _)| i);
        &self.0[..end]
    }
}

/// The sole runtime string allocation/owned-bridge transfer authority.
pub(crate) unsafe fn unicode_from_python_text(text: PythonStringBytes<'_>) -> *mut PyObject {
    let bytes = text.as_bytes();
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(bytes.as_ptr(), bytes.len()) };
    if bits == 0 {
        return unsafe { str_alloc_failed() };
    }
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

/// Admit composed internal Python text without crossing the public UTF-8 codec.
pub(crate) unsafe fn unicode_from_python_bytes(bytes: &[u8]) -> *mut PyObject {
    let Some(text) = PythonStringBytes::from_bytes(bytes) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                c"invalid internal Python string encoding".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    unsafe { unicode_from_python_text(text) }
}

unsafe fn unicode_from_utf8_bytes(bytes: &[u8]) -> *mut PyObject {
    match std::str::from_utf8(bytes) {
        Ok(text) => unsafe { unicode_from_python_text(PythonStringBytes::from_utf8(text)) },
        Err(error) => {
            unsafe { raise_utf8_decode_error(bytes, error) };
            ptr::null_mut()
        }
    }
}

/// Set an exception of type `exc` with a runtime-formatted message.
unsafe fn set_exc(exc: *mut PyObject, msg: &str) {
    unsafe { crate::api::errors::set_python_error_bytes(exc, msg.as_bytes()) };
}

/// Escape Python codepoints for both ascii() and percent %a. ASCII bytes,
/// including embedded NUL, remain counted bytes rather than C strings.
pub(crate) fn ascii_escape(text: PythonStringBytes<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.as_bytes().len());
    for code in text.code_points() {
        if code < 0x80 {
            out.push(code as u8);
        } else if code <= 0xff {
            out.extend_from_slice(format!("\\x{code:02x}").as_bytes());
        } else if code <= 0xffff {
            out.extend_from_slice(format!("\\u{code:04x}").as_bytes());
        } else {
            out.extend_from_slice(format!("\\U{code:08x}").as_bytes());
        }
    }
    out
}

/// Best-effort `Py_TYPE(op)->tp_name` for diagnostics (mirrors CPython's
/// `%.NNNs` type-name interpolations), defaulting to "object".
unsafe fn pyobj_type_name(op: *mut PyObject) -> String {
    if op.is_null() {
        return "object".to_string();
    }
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        return "object".to_string();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        return "object".to_string();
    }
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

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

pub(crate) unsafe fn unicode_bytes(op: *mut PyObject) -> Option<&'static [u8]> {
    if op.is_null() {
        return None;
    }
    let bridge = &*GLOBAL_BRIDGE;
    let bits = bridge.observed_handle_for_pyobj(op)?;
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let data = unsafe { (h.str_data)(bits.bits(), &raw mut len) };
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
    match molt_lang_obj_model::byte_compare::compare_bytes(left, right) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

fn latin1_encode_utf8_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = PythonStringBytes::from_bytes(bytes)?;
    let mut out = Vec::with_capacity(bytes.len());
    for code in text.code_points() {
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

pub(crate) fn push_codepoint_utf8(out: &mut Vec<u8>, code: u32) -> Option<()> {
    if (0xd800..=0xdfff).contains(&code) {
        out.extend_from_slice(&[
            0xed,
            0x80 | ((code >> 6) as u8 & 0x3f),
            0x80 | (code as u8 & 0x3f),
        ]);
        return Some(());
    }
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

fn python_text_to_ucs4(bytes: &[u8]) -> Option<Vec<u32>> {
    Some(
        PythonStringBytes::from_bytes(bytes)?
            .code_points()
            .collect(),
    )
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

    pub(super) fn decimal_value(code: u32) -> Option<u8> {
        let index = UNICODE_DECIMAL_RANGES
            .binary_search_by(|&(start, end)| {
                if code < start {
                    std::cmp::Ordering::Greater
                } else if code > end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        let start = UNICODE_DECIMAL_RANGES[index].0;
        Some(((code - start) % 10) as u8)
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

pub(crate) fn unicode_decimal_digit_value(code: u32) -> Option<u8> {
    unicode_decimal_table::decimal_value(code)
}

pub(crate) fn unicode_is_space(code: u32) -> bool {
    unicode_space_table::is_space(code)
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

/// CPython `_Py_ascii_whitespace[128]` (Objects/bytes_methods.c): the ASCII
/// whitespace classification table behind `Py_ISSPACE`/bytes `.strip()`/
/// `.split()`. A C extension (numpy links it as DATA) indexes it by byte value.
/// Set for the exact CPython set: 0x09-0x0D (TAB/LF/VT/FF/CR), 0x1C-0x1F
/// (FS/GS/RS/US) and 0x20 (SPACE); every other byte is 0.
#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static _Py_ascii_whitespace: [std::os::raw::c_uchar; 128] = {
    let mut t = [0u8; 128];
    t[0x09] = 1;
    t[0x0a] = 1;
    t[0x0b] = 1;
    t[0x0c] = 1;
    t[0x0d] = 1;
    t[0x1c] = 1;
    t[0x1d] = 1;
    t[0x1e] = 1;
    t[0x1f] = 1;
    t[0x20] = 1;
    t
};

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
    unsafe { unicode_from_utf8_bytes(bytes) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromStringAndSize(
    s: *const c_char,
    size: Py_ssize_t,
) -> *mut PyObject {
    if size < 0 {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                "Negative size passed to PyUnicode_FromStringAndSize",
            )
        };
        return ptr::null_mut();
    }
    if s.is_null() {
        if size == 0 {
            return unsafe { unicode_from_utf8_bytes(&[]) };
        }
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                "NULL string with positive size passed to PyUnicode_FromStringAndSize",
            )
        };
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), size as usize) };
    unsafe { unicode_from_utf8_bytes(bytes) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_New(size: Py_ssize_t, maxchar: u32) -> *mut PyObject {
    if size == 0 {
        return unsafe { unicode_from_utf8_bytes(&[]) };
    }
    if maxchar > 0x10ffff {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                "invalid maximum character passed to PyUnicode_New",
            )
        };
        return ptr::null_mut();
    }
    if size < 0 {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                "Negative size passed to PyUnicode_New",
            )
        };
        return ptr::null_mut();
    }
    let result = unsafe { (hooks_or_stubs().unicode_new)(size as usize, maxchar) };
    let op = unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) };
    if op.is_null() {
        return op;
    }
    if !GLOBAL_BRIDGE.begin_unicode_construction(op, maxchar) {
        unsafe {
            crate::api::errors::PyErr_NoMemory();
            crate::api::errors::release_preserving_error(&[op]);
        }
        return ptr::null_mut();
    }
    op
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_unicode_kind(op: *mut PyObject) -> u32 {
    GLOBAL_BRIDGE
        .unicode_layout(op)
        .map_or(0, |(kind, _, _)| kind)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_unicode_data(op: *mut PyObject) -> *mut c_void {
    GLOBAL_BRIDGE
        .unicode_layout(op)
        .map_or(ptr::null_mut(), |(_, data, _)| data)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_unicode_maxchar(op: *mut PyObject) -> u32 {
    GLOBAL_BRIDGE.unicode_maxchar(op).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_WriteChar(
    op: *mut PyObject,
    index: Py_ssize_t,
    code: u32,
) -> c_int {
    let Some((_, _, len)) = GLOBAL_BRIDGE.unicode_layout(op) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return -1;
    };
    if index < 0 || index as usize >= len {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_IndexError).cast(),
                "string index out of range",
            )
        };
        return -1;
    }
    if !GLOBAL_BRIDGE.unicode_write(op, index as usize, code) {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                "character write requires unique open Unicode construction and an admitted code point",
            )
        };
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyUnicode_FastCopyCharacters(
    to: *mut PyObject,
    to_start: Py_ssize_t,
    from: *mut PyObject,
    from_start: Py_ssize_t,
    how_many: Py_ssize_t,
) {
    let (Some((_, _, target_len)), Some((kind, source, source_len))) = (
        GLOBAL_BRIDGE.unicode_layout(to),
        GLOBAL_BRIDGE.unicode_layout(from),
    ) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return;
    };
    if to_start < 0
        || from_start < 0
        || how_many < 0
        || (to_start as usize)
            .checked_add(how_many as usize)
            .is_none_or(|end| end > target_len)
        || (from_start as usize)
            .checked_add(how_many as usize)
            .is_none_or(|end| end > source_len)
        || !GLOBAL_BRIDGE.unicode_is_open(to)
    {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                "invalid Unicode construction copy",
            )
        };
        return;
    }
    // Snapshot overlap before writing, without guessing a physical PyASCIIObject
    // layout for the managed bridge header.
    let mut values = Vec::new();
    if values.try_reserve_exact(how_many as usize).is_err() {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return;
    }
    for offset in 0..how_many as usize {
        let index = from_start as usize + offset;
        values.push(unsafe {
            match kind {
                1 => u32::from(*source.cast::<u8>().add(index)),
                2 => u32::from(*source.cast::<u16>().add(index)),
                _ => *source.cast::<u32>().add(index),
            }
        });
    }
    for (offset, code) in values.into_iter().enumerate() {
        if unsafe { PyUnicode_WriteChar(to, to_start + offset as isize, code) } != 0 {
            return;
        }
    }
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

/// CPython ``PyUnicode_DecodeASCII``: decode ``size`` bytes of ASCII into a
/// ``str``. A byte >= 0x80 is not valid ASCII; with the default (strict) error
/// handler this raises ``UnicodeDecodeError`` (a ``ValueError`` subclass) and
/// returns NULL. ASCII is a strict subset of UTF-8, so the valid slice is
/// forwarded verbatim to the UTF-8 string allocator — never a stub.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_DecodeASCII(
    s: *const c_char,
    size: Py_ssize_t,
    _errors: *const c_char,
) -> *mut PyObject {
    if s.is_null() || size < 0 {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), size as usize) };
    if let Some(start) = bytes.iter().position(|byte| !byte.is_ascii()) {
        unsafe {
            raise_unicode_codec_error(
                (&raw mut crate::abi_types::PyExc_UnicodeDecodeError)
                    .cast::<crate::abi_types::PyObject>(),
                "ascii",
                UnicodeCodecErrorObject::Bytes(bytes),
                start,
                start + 1,
                "ordinal not in range(128)",
            )
        }
        return ptr::null_mut();
    }
    unsafe { PyUnicode_FromStringAndSize(s, size) }
}

/// CPython ``PyUnicode_DecodeUTF16``: decode ``size`` bytes of UTF-16 into a
/// ``str``. ``byteorder`` (when non-NULL) selects endianness: ``< 0`` little,
/// ``> 0`` big, ``0`` native with an optional leading BOM consumed; on return
/// ``*byteorder`` is updated to the order actually used. A dangling half-unit or
/// an unpaired surrogate raises ``UnicodeDecodeError`` and returns NULL. Real
/// decode via ``String::from_utf16`` — never a stub.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_DecodeUTF16(
    s: *const c_char,
    size: Py_ssize_t,
    _errors: *const c_char,
    byteorder: *mut c_int,
) -> *mut PyObject {
    if s.is_null() || size < 0 {
        return ptr::null_mut();
    }
    let original_bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), size as usize) };
    let mut bytes = original_bytes;
    let mut payload_offset = 0usize;
    // Resolve endianness. 0/native with a leading BOM overrides and is consumed.
    let mut big_endian = cfg!(target_endian = "big");
    let requested = if byteorder.is_null() {
        0
    } else {
        unsafe { *byteorder }
    };
    if requested < 0 {
        big_endian = false;
    } else if requested > 0 {
        big_endian = true;
    } else if bytes.len() >= 2 {
        match (bytes[0], bytes[1]) {
            (0xFF, 0xFE) => {
                big_endian = false;
                bytes = &bytes[2..];
                payload_offset = 2;
            }
            (0xFE, 0xFF) => {
                big_endian = true;
                bytes = &bytes[2..];
                payload_offset = 2;
            }
            _ => {}
        }
    }
    if !byteorder.is_null() {
        unsafe { *byteorder = if big_endian { 1 } else { -1 } };
    }
    let encoding = if big_endian { "utf-16-be" } else { "utf-16-le" };
    if bytes.len() % 2 != 0 {
        let start = original_bytes.len() - 1;
        unsafe {
            raise_unicode_codec_error(
                (&raw mut crate::abi_types::PyExc_UnicodeDecodeError)
                    .cast::<crate::abi_types::PyObject>(),
                encoding,
                UnicodeCodecErrorObject::Bytes(original_bytes),
                start,
                start + 1,
                "truncated data",
            )
        }
        return ptr::null_mut();
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if big_endian {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from_le_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    let mut index = 0usize;
    while index < units.len() {
        let unit = units[index];
        if (0xd800..=0xdbff).contains(&unit) {
            let start = payload_offset + index * 2;
            let Some(next) = units.get(index + 1).copied() else {
                unsafe {
                    raise_unicode_codec_error(
                        (&raw mut crate::abi_types::PyExc_UnicodeDecodeError)
                            .cast::<crate::abi_types::PyObject>(),
                        encoding,
                        UnicodeCodecErrorObject::Bytes(original_bytes),
                        start,
                        original_bytes.len(),
                        "unexpected end of data",
                    )
                };
                return ptr::null_mut();
            };
            if !(0xdc00..=0xdfff).contains(&next) {
                unsafe {
                    raise_unicode_codec_error(
                        (&raw mut crate::abi_types::PyExc_UnicodeDecodeError)
                            .cast::<crate::abi_types::PyObject>(),
                        encoding,
                        UnicodeCodecErrorObject::Bytes(original_bytes),
                        start,
                        start + 2,
                        "illegal UTF-16 surrogate",
                    )
                };
                return ptr::null_mut();
            }
            index += 2;
            continue;
        }
        if (0xdc00..=0xdfff).contains(&unit) {
            let start = payload_offset + index * 2;
            unsafe {
                raise_unicode_codec_error(
                    (&raw mut crate::abi_types::PyExc_UnicodeDecodeError)
                        .cast::<crate::abi_types::PyObject>(),
                    encoding,
                    UnicodeCodecErrorObject::Bytes(original_bytes),
                    start,
                    start + 2,
                    "illegal encoding",
                )
            };
            return ptr::null_mut();
        }
        index += 1;
    }
    let text = String::from_utf16(&units)
        .expect("surrogate validation must make the UTF-16 sequence well formed");
    unsafe { PyUnicode_FromStringAndSize(text.as_ptr().cast(), text.len() as Py_ssize_t) }
}

/// Report the projection's ASCII storage class without committing construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_IS_ASCII(op: *mut PyObject) -> c_int {
    c_int::from(GLOBAL_BRIDGE.unicode_is_ascii(op))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromOrdinal(ordinal: c_int) -> *mut PyObject {
    let mut bytes = Vec::new();
    let Some(()) = push_codepoint_utf8(&mut bytes, ordinal as u32) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
                c"ordinal not in range".as_ptr(),
            );
        }
        return ptr::null_mut();
    };
    unsafe { unicode_from_python_bytes(&bytes) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8(op: *mut PyObject) -> *const c_char {
    unsafe { PyUnicode_AsUTF8AndSize(op, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8String(op: *mut PyObject) -> *mut PyObject {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        return ptr::null_mut();
    };
    if std::str::from_utf8(bytes).is_err() {
        unsafe { raise_unicode_encode_error(bytes, "utf-8", "surrogates not allowed", 0x10ffff) };
        return ptr::null_mut();
    }
    unsafe { PyBytes_FromStringAndSize(bytes.as_ptr().cast(), bytes.len() as Py_ssize_t) }
}

/// Raise a structured `UnicodeEncodeError` for the first contiguous code-point
/// run outside `limit`, matching CPython's `unicode_encode_ucs1` span policy.
unsafe fn raise_unicode_encode_error(bytes: &[u8], codec: &str, reason: &str, limit: u32) {
    let Some(text) = PythonStringBytes::from_bytes(bytes) else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return;
    };
    let mut start = None;
    let mut end = 0usize;
    for (index, code) in text.code_points().enumerate() {
        if code > limit || (0xd800..=0xdfff).contains(&code) {
            start.get_or_insert(index);
            end = index + 1;
        } else if start.is_some() {
            break;
        }
    }
    let start = start.expect("encode error helper requires an unencodable code point");
    unsafe {
        raise_unicode_codec_error(
            (&raw mut crate::abi_types::PyExc_UnicodeEncodeError)
                .cast::<crate::abi_types::PyObject>(),
            codec,
            UnicodeCodecErrorObject::Text(text),
            start,
            end,
            reason,
        )
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsASCIIString(op: *mut PyObject) -> *mut PyObject {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    if !bytes.is_ascii() {
        // CPython unicode_encode_ucs1: UnicodeEncodeError, never a bare NULL.
        unsafe { raise_unicode_encode_error(bytes, "ascii", "ordinal not in range(128)", 0x7f) };
        return ptr::null_mut();
    }
    unsafe { PyBytes_FromStringAndSize(bytes.as_ptr().cast(), bytes.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsLatin1String(op: *mut PyObject) -> *mut PyObject {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    let Some(encoded) = latin1_encode_utf8_bytes(bytes) else {
        // CPython unicode_encode_ucs1: UnicodeEncodeError for code points > 0xFF.
        unsafe { raise_unicode_encode_error(bytes, "latin-1", "ordinal not in range(256)", 0xff) };
        return ptr::null_mut();
    };
    unsafe { PyBytes_FromStringAndSize(encoded.as_ptr().cast(), encoded.len() as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8AndSize(
    op: *mut PyObject,
    size: *mut Py_ssize_t,
) -> *const c_char {
    if unsafe { PyUnicode_Check(op) } == 0 {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null();
    }
    let resolved_bits = GLOBAL_BRIDGE.observed_handle_for_pyobj(op);
    let bits = match resolved_bits {
        Some(bits) => bits.bits(),
        None => {
            unsafe { crate::api::errors::PyErr_BadArgument() };
            return ptr::null();
        }
    };
    let h = hooks_or_stubs();
    let mut runtime_len = 0usize;
    let runtime_data = unsafe { (h.str_data)(bits, &raw mut runtime_len) };
    if runtime_data.is_null() {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null();
    }
    let bytes = unsafe { std::slice::from_raw_parts(runtime_data, runtime_len) };
    if std::str::from_utf8(bytes).is_err() {
        unsafe { raise_unicode_encode_error(bytes, "utf-8", "surrogates not allowed", 0x10ffff) };
        return ptr::null();
    }
    let Some((data, len)) = GLOBAL_BRIDGE.unicode_utf8_cache(bits, bytes) else {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return ptr::null();
    };
    if !size.is_null() {
        unsafe { *size = len as Py_ssize_t };
    }
    data.cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromObject(obj: *mut PyObject) -> *mut PyObject {
    if unsafe { PyUnicode_Check(obj) } == 0 {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    }
    if unsafe { PyUnicode_CheckExact(obj) } != 0 {
        unsafe { crate::api::refcount::Py_INCREF(obj) };
        return obj;
    }
    let Some(bytes) = (unsafe { unicode_bytes(obj) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    unsafe { unicode_from_python_bytes(bytes) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_GetLength(op: *mut PyObject) -> Py_ssize_t {
    match GLOBAL_BRIDGE.unicode_layout(op) {
        Some((_, _, len)) => len as Py_ssize_t,
        None => {
            unsafe { crate::api::errors::PyErr_BadArgument() };
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_ReadChar(op: *mut PyObject, index: Py_ssize_t) -> u32 {
    let Some((kind, data, len)) = GLOBAL_BRIDGE.unicode_layout(op) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return u32::MAX;
    };
    if index < 0 || index as usize >= len {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_IndexError).cast(),
                "string index out of range",
            )
        };
        return u32::MAX;
    }
    unsafe {
        match kind {
            1 => u32::from(*data.cast::<u8>().add(index as usize)),
            2 => u32::from(*data.cast::<u16>().add(index as usize)),
            _ => *data.cast::<u32>().add(index as usize),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Check(op: *mut PyObject) -> c_int {
    if GLOBAL_BRIDGE.unicode_layout(op).is_some() {
        return 1;
    }
    unsafe {
        crate::bridge::is_semantic_instance_of(op, &raw mut crate::abi_types::PyUnicode_Type)
            as c_int
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_CheckExact(op: *mut PyObject) -> c_int {
    if GLOBAL_BRIDGE.unicode_is_open(op) {
        return 1;
    }
    unsafe {
        crate::bridge::is_exact_semantic_type(op, &raw mut crate::abi_types::PyUnicode_Type)
            as c_int
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_CompareWithASCIIString(
    op: *mut PyObject,
    s: *const c_char,
) -> c_int {
    let Some(bytes) = (unsafe { unicode_bytes(op) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return -1;
    };
    if s.is_null() {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return -1;
    }
    compare_unicode_bytes(bytes, unsafe { CStr::from_ptr(s) }.to_bytes())
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
    unsafe { unicode_from_python_bytes(&bytes) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUCS4(
    unicode: *mut PyObject,
    target: *mut u32,
    targetsize: Py_ssize_t,
    copy_null: c_int,
) -> *mut u32 {
    if unicode.is_null() || target.is_null() || targetsize < 0 {
        // CPython: NULL target / negative size is PyErr_BadInternalCall().
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(bytes) = (unsafe { unicode_bytes(unicode) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    let Some(codepoints) = python_text_to_ucs4(bytes) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    let required = codepoints.len() + usize::from(copy_null != 0);
    if (targetsize as usize) < required {
        // CPython: PyErr_Format(SystemError, "string is longer than the buffer").
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                "string is longer than the buffer",
            )
        };
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
    let Some(codepoints) = python_text_to_ucs4(bytes) else {
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
    // CPython: non-str operands raise TypeError "Can't compare %.100s and
    // %.100s" — the -1 sentinel is also a valid "left < right" result, so it
    // MUST carry an exception to be distinguishable.
    let (Some(left_bytes), Some(right_bytes)) = (unsafe { unicode_bytes(left) }, unsafe {
        unicode_bytes(right)
    }) else {
        let msg = format!(
            "Can't compare {:.100} and {:.100}",
            unsafe { pyobj_type_name(left) },
            unsafe { pyobj_type_name(right) }
        );
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                &msg,
            )
        };
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
    let (Some(text), Some(needle)) = (unsafe { unicode_bytes(str_obj) }, unsafe {
        unicode_bytes(substr)
    }) else {
        // Non-str operand: TypeError, never a bare -1.
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return -1;
    };
    let (Some(text), Some(needle)) = (
        PythonStringBytes::from_bytes(text),
        PythonStringBytes::from_bytes(needle),
    ) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return -1;
    };
    // CPython ADJUST_INDICES operates on CODE-POINT indices (PyUnicode_GET_LENGTH),
    // not UTF-8 byte offsets — slice the window by chars, then match bytes.
    let char_count = text.code_points().count();
    let (lo, hi) = unicode_range(char_count, start, end);
    // Map code-point window bounds to byte offsets (single pass).
    let mut byte_lo = text.as_bytes().len();
    let mut byte_hi = text.as_bytes().len();
    for (chars_seen, (byte_idx, _)) in text.code_point_indices().enumerate() {
        if chars_seen == lo {
            byte_lo = byte_idx;
        }
        if chars_seen == hi {
            byte_hi = byte_idx;
            break;
        }
    }
    if lo == char_count {
        byte_lo = text.as_bytes().len();
    }
    if hi == char_count {
        byte_hi = text.as_bytes().len();
    }
    let window = &text.as_bytes()[byte_lo..byte_hi];
    // CPython tailmatch: direction > 0 matches the END (endswith); <= 0 the START.
    if direction > 0 {
        window.ends_with(needle.as_bytes()) as Py_ssize_t
    } else {
        window.starts_with(needle.as_bytes()) as Py_ssize_t
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
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    let Some(needle) = (unsafe { unicode_bytes(substr) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    let Some(replacement) = (unsafe { unicode_bytes(repl) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    let out = if needle.is_empty() {
        // CPython inserts the replacement between each CODE POINT (and at both
        // ends) for an empty needle — never inside a multi-byte UTF-8 sequence.
        let Some(text_str) = PythonStringBytes::from_bytes(text) else {
            unsafe { crate::api::errors::PyErr_BadArgument() };
            return ptr::null_mut();
        };
        let limit = if maxcount < 0 {
            usize::MAX
        } else {
            maxcount as usize
        };
        let mut out = Vec::with_capacity(text.len() + replacement.len());
        let mut count = 0usize;
        for (index, _) in text_str.code_point_indices() {
            if count < limit {
                out.extend_from_slice(replacement);
                count += 1;
            }
            let point = PythonStringBytes(&text[index..]).prefix_bytes(1);
            out.extend_from_slice(point);
        }
        if count < limit {
            out.extend_from_slice(replacement);
        }
        out
    } else {
        replace_bytes(text, needle, replacement, maxcount)
    };
    unsafe { unicode_from_python_bytes(&out) }
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
    let Some(text) = PythonStringBytes::from_bytes(bytes) else {
        return ptr::null_mut();
    };
    let (lo, hi) = unicode_range(text.code_points().count(), start, end);
    let begin = text.prefix_bytes(lo).len();
    let end = text.prefix_bytes(hi).len();
    unsafe { unicode_from_python_text(PythonStringBytes(&bytes[begin..end])) }
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
        // Out of memory: fail closed with NULL + MemoryError (CPython contract).
        return unsafe { str_alloc_failed() };
    }
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
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
        // Out of memory: fail closed with NULL + MemoryError (CPython contract).
        return unsafe { str_alloc_failed() };
    }
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AsStringAndSize(
    op: *mut PyObject,
    buf: *mut *mut c_char,
    length: *mut Py_ssize_t,
) -> c_int {
    // CPython Objects/bytesobject.c: buf == NULL is a BadInternalCall; a
    // non-bytes op raises TypeError "expected bytes, %.200s found"; and with
    // length == NULL an interior NUL raises ValueError "embedded null byte".
    if buf.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let expected_bytes_error = |op: *mut PyObject| {
        let msg = format!("expected bytes, {:.200} found", unsafe {
            pyobj_type_name(op)
        });
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                &msg,
            )
        };
    };
    if op.is_null() {
        expected_bytes_error(op);
        return -1;
    }
    let bridge = &*GLOBAL_BRIDGE;
    let bits = match bridge.molt_handle_for_pyobj(op) {
        Some(b) => b.bits(),
        None => {
            expected_bytes_error(op);
            return -1;
        }
    };
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let data = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if data.is_null() {
        unsafe {
            *buf = ptr::null_mut();
        }
        if !length.is_null() {
            unsafe {
                *length = 0;
            }
        }
        expected_bytes_error(op);
        return -1;
    }
    unsafe {
        *buf = data as *mut c_char;
    }
    if !length.is_null() {
        unsafe {
            *length = len as Py_ssize_t;
        }
    } else if unsafe { std::slice::from_raw_parts(data, len) }.contains(&0) {
        // length == NULL: the caller will strlen() the buffer, so an embedded
        // NUL silently truncates — CPython raises ValueError instead.
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
                "embedded null byte",
            )
        };
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Check(op: *mut PyObject) -> c_int {
    unsafe {
        crate::bridge::is_semantic_instance_of(op, &raw mut crate::abi_types::PyBytes_Type) as c_int
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_CheckExact(op: *mut PyObject) -> c_int {
    unsafe {
        crate::bridge::is_exact_semantic_type(op, &raw mut crate::abi_types::PyBytes_Type) as c_int
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AS_STRING(op: *mut PyObject) -> *mut c_char {
    if op.is_null() {
        return ptr::null_mut();
    }
    let bridge = &*GLOBAL_BRIDGE;
    let bits = match bridge.molt_handle_for_pyobj(op) {
        Some(b) => b.bits(),
        None => return ptr::null_mut(),
    };
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
    // CPython: `if (!PyBytes_Check(op)) { PyErr_Format(TypeError, "expected
    // bytes, %.200s found"); return -1; }` — the sentinel carries the exception.
    let expected_bytes_error = |op: *mut PyObject| {
        let msg = format!("expected bytes, {:.200} found", unsafe {
            pyobj_type_name(op)
        });
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                &msg,
            )
        };
    };
    if op.is_null() {
        expected_bytes_error(op);
        return -1;
    }
    let bridge = &*GLOBAL_BRIDGE;
    let bits = match bridge.molt_handle_for_pyobj(op) {
        Some(b) => b.bits(),
        None => {
            expected_bytes_error(op);
            return -1;
        }
    };
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let data = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if data.is_null() {
        expected_bytes_error(op);
        -1
    } else {
        len as Py_ssize_t
    }
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
    unsafe { unicode_from_python_bytes(&combined) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Join(
    separator: *mut PyObject,
    seq: *mut PyObject,
) -> *mut PyObject {
    // CPython Objects/unicodeobject.c PyUnicode_Join: iterate the sequence,
    // concatenating each item with the separator; a NULL separator falls back
    // to a single space; a non-str item raises TypeError
    // "sequence item %zd: expected str instance, %.80s found".
    if seq.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let sep: &[u8] = if separator.is_null() {
        b" "
    } else {
        match unsafe { unicode_bytes(separator) } {
            Some(bytes) => bytes,
            None => {
                unsafe {
                    set_exc(
                        (&raw mut crate::abi_types::PyExc_TypeError)
                            .cast::<crate::abi_types::PyObject>(),
                        "separator: expected str instance",
                    )
                };
                return ptr::null_mut();
            }
        }
    };
    // Fast-path exact list/tuple like CPython's `PySequence_Fast` (`PyUnicode_Join`
    // itself calls `PySequence_Fast` before iterating). This is not just a perf
    // shortcut here: the ABI's own native tuple layout (`PyTuple_New`'s boxed
    // `PyTupleObject`) is never bridge-registered and `PyTuple_Type`/`PyList_Type`
    // carry no `tp_as_sequence` slot, so the generic `PySequence_Size`/`GetItem`
    // protocol cannot see a tuple/list built through the C API at all — only
    // `PyTuple_Size`/`GetItem` and `PyList_Size`/`GetItem` resolve both the
    // ABI-native layout and a bridge-managed Molt list/tuple.
    let is_tuple = unsafe { crate::api::sequences::PyTuple_Check(seq) } != 0;
    let is_list = !is_tuple && unsafe { crate::api::sequences::PyList_Check(seq) } != 0;
    let n = if is_tuple {
        unsafe { crate::api::sequences::PyTuple_Size(seq) }
    } else if is_list {
        unsafe { crate::api::sequences::PyList_Size(seq) }
    } else {
        unsafe { crate::api::abstract_sequence::PySequence_Size(seq) }
    };
    if n < 0 {
        // PySequence_Size set (or will be fixed to set) the honest exception.
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { crate::api::errors::PyErr_BadArgument() };
        }
        return ptr::null_mut();
    }
    // Writer pattern: one output buffer, one final allocation via alloc_str.
    let mut out: Vec<u8> = Vec::new();
    for i in 0..n {
        // PyTuple_GetItem/PyList_GetItem return a BORROWED reference (unlike
        // PySequence_GetItem's new reference) — incref so the single DECREF
        // below is correct for all three paths.
        let item = if is_tuple {
            let borrowed = unsafe { crate::api::sequences::PyTuple_GetItem(seq, i) };
            if !borrowed.is_null() {
                unsafe { crate::api::refcount::Py_INCREF(borrowed) };
            }
            borrowed
        } else if is_list {
            let borrowed = unsafe { crate::api::sequences::PyList_GetItem(seq, i) };
            if !borrowed.is_null() {
                unsafe { crate::api::refcount::Py_INCREF(borrowed) };
            }
            borrowed
        } else {
            unsafe { crate::api::abstract_sequence::PySequence_GetItem(seq, i) }
        };
        if item.is_null() {
            if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                unsafe { crate::api::errors::PyErr_BadArgument() };
            }
            return ptr::null_mut();
        }
        let bytes = unsafe { unicode_bytes(item) };
        let Some(bytes) = bytes else {
            let msg = format!(
                "sequence item {i}: expected str instance, {:.80} found",
                unsafe { pyobj_type_name(item) }
            );
            unsafe { crate::api::refcount::Py_DECREF(item) };
            unsafe {
                set_exc(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    &msg,
                )
            };
            return ptr::null_mut();
        };
        if i > 0 {
            out.extend_from_slice(sep);
        }
        out.extend_from_slice(bytes);
        unsafe { crate::api::refcount::Py_DECREF(item) };
    }
    unsafe { unicode_from_python_bytes(&out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Contains(
    container: *mut PyObject,
    element: *mut PyObject,
) -> c_int {
    // CPython: a non-str element raises TypeError "'in <string>' requires
    // string as left operand, not %.100s"; the -1 sentinel always carries it.
    let Some(e_bytes) = (unsafe { unicode_bytes(element) }) else {
        let msg = format!(
            "'in <string>' requires string as left operand, not {:.100}",
            unsafe { pyobj_type_name(element) }
        );
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                &msg,
            )
        };
        return -1;
    };
    let Some(c_bytes) = (unsafe { unicode_bytes(container) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
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
    let Some(text) = PythonStringBytes::from_bytes(bytes) else {
        return -1;
    };
    let target = ch;
    let chars: Vec<u32> = text.code_points().collect();
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
    encoding: *const c_char,
    errors: *const c_char,
) -> *mut PyObject {
    // CPython dispatches to the codec named by `encoding` — the previous body
    // silently decoded EVERYTHING as UTF-8. Dispatch on the alias table used by
    // AsEncodedString; unknown encodings fail closed with LookupError.
    if encoding.is_null() {
        return unsafe { PyUnicode_DecodeUTF8(s, size, errors) };
    }
    let name = unsafe { CStr::from_ptr(encoding) }.to_bytes();
    if encoding_name_matches(name, &[b"utf8", b"utf-8"]) {
        return unsafe { PyUnicode_DecodeUTF8(s, size, errors) };
    }
    if encoding_name_matches(name, &[b"ascii", b"us-ascii"]) {
        return unsafe { PyUnicode_DecodeASCII(s, size, errors) };
    }
    if encoding_name_matches(
        name,
        &[
            b"latin1",
            b"latin-1",
            b"latin_1",
            b"iso8859-1",
            b"iso-8859-1",
        ],
    ) {
        return unsafe { PyUnicode_DecodeLatin1(s, size, errors) };
    }
    if encoding_name_matches(name, &[b"utf16", b"utf-16"]) {
        return unsafe { PyUnicode_DecodeUTF16(s, size, errors, ptr::null_mut()) };
    }
    let msg = format!("unknown encoding: {}", String::from_utf8_lossy(name));
    unsafe {
        set_exc(
            (&raw mut crate::abi_types::PyExc_LookupError).cast::<crate::abi_types::PyObject>(),
            &msg,
        )
    };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_DecodeUTF8(
    s: *const c_char,
    size: Py_ssize_t,
    _errors: *const c_char,
) -> *mut PyObject {
    if s.is_null() || size < 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    // CPython validates and (under the default strict handler) raises
    // UnicodeDecodeError on malformed input — never silently accepts it.
    let bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), size as usize) };
    if let Err(e) = std::str::from_utf8(bytes) {
        unsafe { raise_utf8_decode_error(bytes, e) };
        return ptr::null_mut();
    }
    unsafe { PyUnicode_FromStringAndSize(s, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromEncodedObject(
    obj: *mut PyObject,
    encoding: *const c_char,
    errors: *const c_char,
) -> *mut PyObject {
    // CPython Objects/unicodeobject.c: a str input is REJECTED ("decoding str
    // is not supported"), a non-bytes-like input raises TypeError, and the
    // bytes are decoded with the REQUESTED encoding (the old body incref'd str,
    // ignored the encoding, and fabricated str() of arbitrary objects).
    if obj.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    if unsafe { PyUnicode_Check(obj) } != 0 {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                "decoding str is not supported",
            )
        };
        return ptr::null_mut();
    }
    if unsafe { PyBytes_Check(obj) } != 0 {
        let mut data: *mut c_char = ptr::null_mut();
        let mut len: Py_ssize_t = 0;
        if unsafe { PyBytes_AsStringAndSize(obj, &raw mut data, &raw mut len) } != 0 {
            return ptr::null_mut();
        }
        return unsafe { PyUnicode_Decode(data.cast_const(), len, encoding, errors) };
    }
    let msg = format!(
        "decoding to str: need a bytes-like object, {:.80} found",
        unsafe { pyobj_type_name(obj) }
    );
    unsafe {
        set_exc(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            &msg,
        )
    };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsEncodedString(
    unicode: *mut PyObject,
    encoding: *const c_char,
    errors: *const c_char,
) -> *mut PyObject {
    let Some(bits) = GLOBAL_BRIDGE.observed_handle_for_pyobj(unicode) else {
        unsafe {
            crate::bridge::ensure_result_error(c"Unicode encoding requires a valid runtime string")
        };
        return ptr::null_mut();
    };
    if unsafe { PyUnicode_Check(unicode) } == 0 {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    }
    let encoding_obj = unsafe {
        PyUnicode_FromString(if encoding.is_null() {
            c"utf-8".as_ptr()
        } else {
            encoding
        })
    };
    if encoding_obj.is_null() {
        return ptr::null_mut();
    }
    let errors_obj = unsafe {
        PyUnicode_FromString(if errors.is_null() {
            c"strict".as_ptr()
        } else {
            errors
        })
    };
    if errors_obj.is_null() {
        unsafe { crate::api::errors::release_preserving_error(&[encoding_obj]) };
        return ptr::null_mut();
    }
    let encoding_bits = GLOBAL_BRIDGE.observed_handle_for_pyobj(encoding_obj);
    let errors_bits = GLOBAL_BRIDGE.observed_handle_for_pyobj(errors_obj);
    let result = match (encoding_bits, errors_bits) {
        (Some(encoding), Some(errors)) => unsafe {
            GLOBAL_BRIDGE.owned_result_to_pyobj((hooks_or_stubs().unicode_encode)(
                bits.bits(),
                encoding.bits(),
                errors.bits(),
            ))
        },
        _ => {
            unsafe { crate::api::errors::PyErr_BadArgument() };
            ptr::null_mut()
        }
    };
    unsafe { crate::api::errors::release_preserving_error(&[encoding_obj, errors_obj]) };
    result
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
    let rendered = if repr {
        unsafe { crate::api::typeobj::PyObject_Repr(arg) }
    } else {
        unsafe { crate::api::typeobj::PyObject_Str(arg) }
    };
    if rendered.is_null() {
        return None;
    }
    let text = unsafe { unicode_bytes(rendered) }
        .and_then(PythonStringBytes::from_bytes)
        .map(|text| text.as_bytes().to_vec());
    if text.is_none() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
    }
    unsafe { crate::api::errors::release_preserving_error(&[rendered]) };
    text
}

/// Parsed printf-style conversion spec: `%[flags][width][.prec]conv`.
#[derive(Default, Clone, Copy)]
struct PercentSpec {
    alt: bool,   // '#'
    zero: bool,  // '0'
    left: bool,  // '-'
    space: bool, // ' '
    plus: bool,  // '+'
    width: Option<usize>,
    prec: Option<usize>,
}

/// Pad admitted Python text to `width` CODE POINTS with spaces,
/// left- or right-justified. Appends into `out` (writer pattern, no extra
/// intermediate strings).
fn pad_field(out: &mut Vec<u8>, field: &[u8], spec: &PercentSpec) {
    let cp_len = PythonStringBytes::from_bytes(field)
        .expect("percent fields are admitted Python text")
        .code_points()
        .count();
    let width = spec.width.unwrap_or(0);
    let pad = width.saturating_sub(cp_len);
    if pad == 0 {
        out.extend_from_slice(field);
        return;
    }
    if spec.left {
        out.extend_from_slice(field);
        out.extend(std::iter::repeat_n(b' ', pad));
    } else {
        out.extend(std::iter::repeat_n(b' ', pad));
        out.extend_from_slice(field);
    }
}

/// Render a signed integer per C printf semantics (`%d/%i/%u/%o/%x/%X`),
/// honoring '#' (0o/0x prefix), precision (minimum digits), '+'/' ' sign,
/// '0' zero-fill, width, and '-' left-justify. Appends into `out`.
fn emit_formatted_int(out: &mut Vec<u8>, value: i128, base: u32, upper: bool, spec: &PercentSpec) {
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let mut digits = match base {
        8 => format!("{magnitude:o}"),
        16 => {
            if upper {
                format!("{magnitude:X}")
            } else {
                format!("{magnitude:x}")
            }
        }
        _ => format!("{magnitude}"),
    };
    if let Some(prec) = spec.prec
        && digits.len() < prec
    {
        digits = format!("{}{digits}", "0".repeat(prec - digits.len()));
    }
    let sign: &str = if negative {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };
    let prefix: &str = if spec.alt {
        match (base, upper) {
            (8, _) => "0o",
            (16, false) => "0x",
            (16, true) => "0X",
            _ => "",
        }
    } else {
        ""
    };
    let body_len = sign.len() + prefix.len() + digits.len();
    let width = spec.width.unwrap_or(0);
    if spec.zero && !spec.left && spec.prec.is_none() && width > body_len {
        // Zero-fill between sign/prefix and digits.
        out.extend_from_slice(sign.as_bytes());
        out.extend_from_slice(prefix.as_bytes());
        out.extend(std::iter::repeat_n(b'0', width - body_len));
        out.extend_from_slice(digits.as_bytes());
    } else {
        let mut field = Vec::with_capacity(body_len);
        field.extend_from_slice(sign.as_bytes());
        field.extend_from_slice(prefix.as_bytes());
        field.extend_from_slice(digits.as_bytes());
        pad_field(out, &field, spec);
    }
}

/// C-style `%e` exponent normalization: Rust's `{:e}` renders `1.5e2`; C
/// requires `1.500000e+02` (sign + at least two exponent digits).
fn c_style_exponent(rendered: &str, upper: bool) -> String {
    let (mantissa, exp) = match rendered.split_once('e') {
        Some(parts) => parts,
        None => (rendered, "0"),
    };
    let (sign, digits) = match exp.strip_prefix('-') {
        Some(d) => ('-', d),
        None => ('+', exp),
    };
    let e = if upper { 'E' } else { 'e' };
    format!("{mantissa}{e}{sign}{digits:0>2}")
}

/// Render a float per C printf semantics for conv in f/F/e/E/g/G.
fn format_float_percent(value: f64, conv: u8, spec: &PercentSpec) -> String {
    let prec = spec.prec.unwrap_or(6);
    let upper = conv.is_ascii_uppercase();
    if value.is_nan() {
        return if upper { "NAN".into() } else { "nan".into() };
    }
    if value.is_infinite() {
        let body = if upper { "INF" } else { "inf" };
        return if value < 0.0 {
            format!("-{body}")
        } else {
            body.to_string()
        };
    }
    match conv.to_ascii_lowercase() {
        b'f' => format!("{value:.prec$}"),
        b'e' => c_style_exponent(&format!("{value:.prec$e}"), upper),
        _ => {
            // %g: precision P significant digits (0 -> 1); use %e when the
            // decimal exponent X < -4 or X >= P, else %f with P-1-X decimals;
            // strip trailing zeros unless '#'.
            let p = prec.max(1);
            let e_rendered = format!("{value:.*e}", p - 1);
            let exp: i32 = e_rendered
                .split('e')
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let mut body = if exp < -4 || exp >= p as i32 {
                c_style_exponent(&e_rendered, upper)
            } else {
                let decimals = (p as i32 - 1 - exp).max(0) as usize;
                format!("{value:.decimals$}")
            };
            if !spec.alt {
                // Strip trailing zeros (and a bare trailing '.') from the
                // mantissa part only.
                let (mantissa_end, tail) = match body.find(['e', 'E']) {
                    Some(idx) => (idx, body[idx..].to_string()),
                    None => (body.len(), String::new()),
                };
                let mantissa = &body[..mantissa_end];
                if mantissa.contains('.') {
                    let stripped = mantissa.trim_end_matches('0').trim_end_matches('.');
                    body = format!("{stripped}{tail}");
                }
            }
            body
        }
    }
}

/// Resolve the next `%` argument as an integer for `%d/%i/%u/%o/%x/%X/%c`,
/// accepting int (exact) and float (truncated, `%d` only) like CPython's
/// `formatlong` (`PyNumber_Long`). On failure: TypeError + None.
unsafe fn percent_int_arg(arg: *mut PyObject, conv: u8) -> Option<i128> {
    let raise_not_a_number = |arg: *mut PyObject| unsafe {
        let msg = format!(
            "%{} format: a real number is required, not {:.200}",
            conv as char,
            pyobj_type_name(arg)
        );
        set_exc(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            &msg,
        );
    };
    // Native int / bool via the bridge handle (exact, no width truncation).
    let bits = crate::bridge::GLOBAL_BRIDGE.molt_handle_for_pyobj(arg);
    if let Some(bits) = bits {
        let mo = bits.decode();
        if let Some(i) = mo.as_int() {
            return Some(i as i128);
        }
        if mo.is_bool() {
            return Some(mo.as_bool().unwrap_or(false) as i128);
        }
        if (conv == b'd' || conv == b'i' || conv == b'u')
            && let Some(f) = mo.as_float()
            && f.is_finite()
        {
            return Some(f as i128);
        }
        // A definitively-classified native object (bridge resolution
        // succeeded) that is not int/bool/(usable) float can never satisfy
        // PyNumber_Long/__index__ — raise directly rather than falling
        // through to PyLong_AsLongLong below. That converter's non-int path
        // is a silent `-1`-with-no-exception sentinel (numbers.rs, a
        // different lane's file — see the ledger's SILENT_SENTINEL rows for
        // PyLong_AsLongLong); reaching it here previously let e.g.
        // `"%d" % "nope"` silently format as `-1` instead of raising.
        raise_not_a_number(arg);
        return None;
    }
    // Foreign object (bridge resolution failed): go through the checked
    // converter, which dispatches __index__ for a genuine foreign int-like.
    // NOTE: PyLong_AsLongLong's non-int path does not yet guarantee a set
    // exception on every failure (see above) — a foreign object that
    // legitimately converts to -1 must NOT be misreported as an error, so
    // the ambiguous case is resolved by the pending-exception check exactly
    // as CPython callers are required to (`v == -1 && PyErr_Occurred()`).
    unsafe { crate::api::errors::PyErr_Clear() };
    let v = unsafe { crate::api::numbers::PyLong_AsLongLong(arg) };
    if v == -1 && !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        unsafe { crate::api::errors::PyErr_Clear() };
        raise_not_a_number(arg);
        return None;
    }
    Some(v as i128)
}

/// Mapping lookup owns its result even when a later format parse or callback
/// fails. Release on every exit without disturbing that failure.
struct PercentMappedArgument(*mut PyObject);

impl Drop for PercentMappedArgument {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { crate::api::errors::release_preserving_error(&[self.0]) };
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Format(
    format: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    if format.is_null() || args.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(format_bytes) = (unsafe { unicode_bytes(format) }) else {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return ptr::null_mut();
    };
    let format_bytes = format_bytes.to_vec();
    // Writer pattern: one output buffer, one final runtime allocation.
    let mut out = Vec::with_capacity(format_bytes.len());
    let mut cursor = 0usize;
    let mut arg_index: Py_ssize_t = 0;
    let mut used_mapping_keys = false;

    let incomplete = || unsafe {
        set_exc(
            (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
            "incomplete format",
        );
    };

    while cursor < format_bytes.len() {
        let ch = format_bytes[cursor];
        cursor += 1;
        if ch != b'%' {
            out.push(ch);
            continue;
        }
        if cursor >= format_bytes.len() {
            incomplete();
            return ptr::null_mut();
        }
        if format_bytes[cursor] == b'%' {
            out.push(b'%');
            cursor += 1;
            continue;
        }

        // Mapping key: '%(name)s' — args must support item lookup.
        let mut mapped_arg = PercentMappedArgument(ptr::null_mut());
        if format_bytes[cursor] == b'(' {
            used_mapping_keys = true;
            cursor += 1;
            let key_start = cursor;
            let mut depth = 1usize;
            while cursor < format_bytes.len() && depth > 0 {
                match format_bytes[cursor] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    cursor += 1;
                }
            }
            if cursor >= format_bytes.len() {
                incomplete();
                return ptr::null_mut();
            }
            let key_bytes = &format_bytes[key_start..cursor];
            cursor += 1; // consume ')'
            let key_obj = unsafe { unicode_from_python_bytes(key_bytes) };
            if key_obj.is_null() {
                return ptr::null_mut();
            }
            mapped_arg.0 = unsafe { crate::api::object::PyObject_GetItem(args, key_obj) };
            unsafe { crate::api::errors::release_preserving_error(&[key_obj]) };
            if mapped_arg.0.is_null() {
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    unsafe {
                        set_exc(
                            (&raw mut crate::abi_types::PyExc_TypeError)
                                .cast::<crate::abi_types::PyObject>(),
                            "format requires a mapping",
                        )
                    };
                }
                return ptr::null_mut();
            }
        }

        // Flags.
        let mut spec = PercentSpec::default();
        while cursor < format_bytes.len() {
            match format_bytes[cursor] {
                b'#' => spec.alt = true,
                b'0' => spec.zero = true,
                b'-' => spec.left = true,
                b' ' => spec.space = true,
                b'+' => spec.plus = true,
                _ => break,
            }
            cursor += 1;
        }
        // Width ('*' consumes an int argument, like CPython).
        if cursor < format_bytes.len() && format_bytes[cursor] == b'*' {
            cursor += 1;
            let warg = unsafe { unicode_format_arg(args, &mut arg_index) };
            if warg.is_null() {
                unsafe {
                    set_exc(
                        (&raw mut crate::abi_types::PyExc_TypeError)
                            .cast::<crate::abi_types::PyObject>(),
                        "not enough arguments for format string",
                    )
                };
                return ptr::null_mut();
            }
            let Some(w) = (unsafe { percent_int_arg(warg, b'd') }) else {
                return ptr::null_mut();
            };
            if w < 0 {
                spec.left = true;
                spec.width = Some((-w) as usize);
            } else {
                spec.width = Some(w as usize);
            }
        } else {
            let mut width: Option<usize> = None;
            while cursor < format_bytes.len() && format_bytes[cursor].is_ascii_digit() {
                width = Some(width.unwrap_or(0) * 10 + (format_bytes[cursor] - b'0') as usize);
                cursor += 1;
            }
            spec.width = width;
        }
        // Precision.
        if cursor < format_bytes.len() && format_bytes[cursor] == b'.' {
            cursor += 1;
            if cursor < format_bytes.len() && format_bytes[cursor] == b'*' {
                cursor += 1;
                let parg = unsafe { unicode_format_arg(args, &mut arg_index) };
                if parg.is_null() {
                    unsafe {
                        set_exc(
                            (&raw mut crate::abi_types::PyExc_TypeError)
                                .cast::<crate::abi_types::PyObject>(),
                            "not enough arguments for format string",
                        )
                    };
                    return ptr::null_mut();
                }
                let Some(p) = (unsafe { percent_int_arg(parg, b'd') }) else {
                    return ptr::null_mut();
                };
                spec.prec = Some(p.max(0) as usize);
            } else {
                let mut prec = 0usize;
                while cursor < format_bytes.len() && format_bytes[cursor].is_ascii_digit() {
                    prec = prec * 10 + (format_bytes[cursor] - b'0') as usize;
                    cursor += 1;
                }
                spec.prec = Some(prec);
            }
        }
        // Length modifiers 'l'/'h'/'z' are legal no-ops in CPython %-format.
        while cursor < format_bytes.len() && matches!(format_bytes[cursor], b'l' | b'h' | b'z') {
            cursor += 1;
        }
        if cursor >= format_bytes.len() {
            incomplete();
            return ptr::null_mut();
        }
        let conversion_offset = cursor;
        let conv = format_bytes[cursor];
        let conversion = PythonStringBytes::from_bytes(&format_bytes[cursor..])
            .expect("admitted Python format string");
        let codepoint = conversion.code_points().next().expect("format conversion");
        cursor += conversion
            .code_point_indices()
            .nth(1)
            .map_or(conversion.as_bytes().len(), |(offset, _)| offset);

        // Resolve the argument (mapping key result or next positional).
        let arg = if !mapped_arg.0.is_null() {
            mapped_arg.0
        } else {
            let a = unsafe { unicode_format_arg(args, &mut arg_index) };
            if a.is_null() {
                unsafe {
                    set_exc(
                        (&raw mut crate::abi_types::PyExc_TypeError)
                            .cast::<crate::abi_types::PyObject>(),
                        "not enough arguments for format string",
                    )
                };
                return ptr::null_mut();
            }
            a
        };
        match conv {
            b's' | b'r' | b'a' | b'S' | b'R' => {
                let repr = matches!(conv, b'r' | b'R' | b'a');
                let Some(mut text) = (unsafe { unicode_format_object_bytes(arg, repr) }) else {
                    return ptr::null_mut();
                };
                if conv == b'a' {
                    let admitted = PythonStringBytes::from_bytes(&text)
                        .expect("callback result was admitted above");
                    text = ascii_escape(admitted);
                }
                // Precision truncates at Python code points, including surrogates.
                if let Some(prec) = spec.prec {
                    let keep = PythonStringBytes::from_bytes(&text)
                        .expect("percent field is admitted Python text")
                        .prefix_bytes(prec)
                        .len();
                    text.truncate(keep);
                }
                pad_field(&mut out, &text, &spec);
            }
            b'd' | b'i' | b'u' => {
                let Some(v) = (unsafe { percent_int_arg(arg, conv) }) else {
                    return ptr::null_mut();
                };
                emit_formatted_int(&mut out, v, 10, false, &spec);
            }
            b'o' | b'x' | b'X' => {
                let Some(v) = (unsafe { percent_int_arg(arg, conv) }) else {
                    return ptr::null_mut();
                };
                let base = if conv == b'o' { 8 } else { 16 };
                emit_formatted_int(&mut out, v, base, conv == b'X', &spec);
            }
            b'c' => {
                // %c: a single-character str, or an int code point.
                let rendered: Option<Vec<u8>> = match unsafe { unicode_bytes(arg) } {
                    Some(bytes) => {
                        let ok = PythonStringBytes::from_bytes(bytes)
                            .is_some_and(|s| s.code_points().count() == 1);
                        ok.then(|| bytes.to_vec())
                    }
                    None => unsafe { percent_int_arg(arg, b'c') }.and_then(|code| {
                        unsafe { crate::api::errors::PyErr_Clear() };
                        let mut bytes = Vec::new();
                        push_codepoint_utf8(&mut bytes, u32::try_from(code).ok()?)?;
                        Some(bytes)
                    }),
                };
                let Some(rendered) = rendered else {
                    unsafe {
                        set_exc(
                            (&raw mut crate::abi_types::PyExc_TypeError)
                                .cast::<crate::abi_types::PyObject>(),
                            "%c requires int or char",
                        )
                    };
                    return ptr::null_mut();
                };
                pad_field(&mut out, &rendered, &spec);
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                unsafe { crate::api::errors::PyErr_Clear() };
                let v = unsafe { crate::api::numbers::PyFloat_AsDouble(arg) };
                if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    return ptr::null_mut();
                }
                let mut rendered = format_float_percent(v, conv, &spec);
                if !rendered.starts_with('-') {
                    if spec.plus {
                        rendered.insert(0, '+');
                    } else if spec.space {
                        rendered.insert(0, ' ');
                    }
                }
                // '0' zero-fill for floats (after any sign).
                let width = spec.width.unwrap_or(0);
                if spec.zero && !spec.left && width > rendered.len() {
                    let (sign, digits) = match rendered.strip_prefix(['-', '+', ' ']) {
                        Some(rest) => (&rendered[..1], rest),
                        None => ("", rendered.as_str()),
                    };
                    out.extend_from_slice(sign.as_bytes());
                    out.extend(std::iter::repeat_n(b'0', width - rendered.len()));
                    out.extend_from_slice(digits.as_bytes());
                } else {
                    pad_field(&mut out, rendered.as_bytes(), &spec);
                }
            }
            _ => {
                let index = PythonStringBytes::from_bytes(&format_bytes[..conversion_offset])
                    .expect("format prefix ends at a codepoint boundary")
                    .code_points()
                    .count();
                let msg = format!(
                    "unsupported format character '{}' (0x{:x}) at index {}",
                    if (31..=126).contains(&codepoint) {
                        char::from_u32(codepoint).expect("ASCII format diagnostic")
                    } else {
                        '?'
                    },
                    codepoint,
                    index,
                );
                unsafe {
                    set_exc(
                        (&raw mut crate::abi_types::PyExc_ValueError)
                            .cast::<crate::abi_types::PyObject>(),
                        &msg,
                    )
                };
                return ptr::null_mut();
            }
        }
    }

    // Surplus positional args (tuple form, no mapping keys): TypeError.
    if !used_mapping_keys
        && unsafe { crate::api::sequences::PyTuple_Check(args) } != 0
        && arg_index < unsafe { crate::api::sequences::PyTuple_Size(args) }
    {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                "not all arguments converted during string formatting",
            )
        };
        return ptr::null_mut();
    }

    unsafe { unicode_from_python_bytes(&out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_GET_LENGTH(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyUnicode_GetLength(op) }
}

// ─── Additional PyBytes functions ────────────────────────────────────────

/// `PyBytes_Concat(pv, w)` — set `*pv = *pv + w`, dropping the old `*pv`
/// (Objects/bytesobject.c). On failure CPython clears `*pv` to NULL. The stub
/// dropped `newpart` (`let _ = newpart;`), so the concatenation silently did
/// nothing while the caller believed `*pv` had grown. This reads both operands'
/// bytes through the runtime `bytes_data` authority, builds the joined bytes, and
/// replaces `*pv`; on OOM it clears `*pv` and sets MemoryError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Concat(bytes: *mut *mut PyObject, newpart: *mut PyObject) {
    if bytes.is_null() || unsafe { *bytes }.is_null() || newpart.is_null() {
        return;
    }
    let old = unsafe { *bytes };

    // Read the left operand's bytes.
    let Some(left_bits) = GLOBAL_BRIDGE.molt_handle_for_pyobj(old) else {
        return;
    };
    let Some(right_bits) = GLOBAL_BRIDGE.molt_handle_for_pyobj(newpart) else {
        return;
    };
    let h = hooks_or_stubs();
    let mut left_len: usize = 0;
    let left_ptr = unsafe { (h.bytes_data)(left_bits.bits(), std::ptr::addr_of_mut!(left_len)) };
    let mut right_len: usize = 0;
    let right_ptr = unsafe { (h.bytes_data)(right_bits.bits(), std::ptr::addr_of_mut!(right_len)) };
    if left_ptr.is_null() || right_ptr.is_null() {
        // Not bytes-like / no runtime: leave *pv untouched rather than corrupting it.
        return;
    }
    let mut combined = Vec::with_capacity(left_len + right_len);
    combined.extend_from_slice(unsafe { std::slice::from_raw_parts(left_ptr, left_len) });
    combined.extend_from_slice(unsafe { std::slice::from_raw_parts(right_ptr, right_len) });

    let new_obj = unsafe {
        PyBytes_FromStringAndSize(combined.as_ptr().cast(), combined.len() as Py_ssize_t)
    };
    if new_obj.is_null() {
        // OOM: CPython clears *pv to NULL; the constructor already set MemoryError.
        unsafe { crate::api::refcount::Py_DECREF(old) };
        unsafe { *bytes = ptr::null_mut() };
        return;
    }
    unsafe { crate::api::refcount::Py_DECREF(old) };
    unsafe { *bytes = new_obj };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_FromStringAndSize(
    s: *const c_char,
    len: Py_ssize_t,
) -> *mut PyObject {
    if len < 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let bits = unsafe { (hooks_or_stubs().alloc_bytearray)(s.cast(), len as usize) };
    if bits == 0 {
        if !crate::api::errors::transfer_runtime_pending_to_current() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
        }
        return ptr::null_mut();
    }
    let result = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    unsafe { crate::api::errors::check_native_result(result, "PyByteArray_FromStringAndSize") }
}

/// The caller supplies a stable stack address: Py_buffer fields may point to
/// other fields of the same descriptor. Never move an acquired descriptor.
unsafe fn bytearray_copy_buffer(op: *mut PyObject, view: *mut crate::abi_types::Py_buffer) -> bool {
    let status =
        unsafe { crate::api::buffer::PyObject_GetBuffer(op, view, crate::abi_types::PyBUF_SIMPLE) };
    if unsafe { crate::api::errors::check_native_status(status, "PyByteArray buffer acquisition") }
        < 0
    {
        if status >= 0 {
            crate::api::errors::with_preserved_error(|| unsafe {
                crate::api::buffer::PyBuffer_Release(view)
            });
        }
        return false;
    }
    if unsafe { (*view).len < 0 || ((*view).len != 0 && (*view).buf.is_null()) } {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        crate::api::errors::with_preserved_error(|| unsafe {
            crate::api::buffer::PyBuffer_Release(view)
        });
        return false;
    }
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_FromObject(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    if unsafe { crate::api::buffer::PyObject_CheckBuffer(op) } != 0 {
        let mut view: crate::abi_types::Py_buffer = unsafe { std::mem::zeroed() };
        if !unsafe { bytearray_copy_buffer(op, &mut view) } {
            return ptr::null_mut();
        }
        let result = unsafe { PyByteArray_FromStringAndSize(view.buf.cast(), view.len) };
        crate::api::errors::with_preserved_error(|| unsafe {
            crate::api::buffer::PyBuffer_Release(&mut view)
        });
        return result;
    }
    // Non-buffer inputs retain Python bytearray's integer/iterable conversion.
    let result = unsafe {
        crate::api::object::PyObject_CallOneArg(
            (&raw mut crate::abi_types::PyByteArray_Type).cast(),
            op,
        )
    };
    unsafe { crate::api::errors::check_native_result(result, "PyByteArray_FromObject") }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_Concat(
    left: *mut PyObject,
    right: *mut PyObject,
) -> *mut PyObject {
    let mut first: crate::abi_types::Py_buffer = unsafe { std::mem::zeroed() };
    let mut second: crate::abi_types::Py_buffer = unsafe { std::mem::zeroed() };
    if !unsafe { bytearray_copy_buffer(left, &mut first) } {
        return ptr::null_mut();
    }
    if !unsafe { bytearray_copy_buffer(right, &mut second) } {
        crate::api::errors::with_preserved_error(|| unsafe {
            crate::api::buffer::PyBuffer_Release(&mut first)
        });
        return ptr::null_mut();
    }
    let result = if let Some(len) = first.len.checked_add(second.len) {
        let result = unsafe { PyByteArray_FromStringAndSize(ptr::null(), len) };
        if result.is_null() {
            result
        } else {
            let data = unsafe { PyByteArray_AsString(result) };
            if data.is_null() {
                unsafe { crate::api::errors::release_preserving_error(&[result]) };
                ptr::null_mut()
            } else {
                if first.len != 0 {
                    unsafe {
                        ptr::copy_nonoverlapping(
                            first.buf.cast::<c_char>(),
                            data,
                            first.len as usize,
                        )
                    }
                };
                if second.len != 0 {
                    unsafe {
                        ptr::copy_nonoverlapping(
                            second.buf.cast::<c_char>(),
                            data.add(first.len as usize),
                            second.len as usize,
                        )
                    }
                };
                result
            }
        }
    } else {
        unsafe { crate::api::errors::PyErr_NoMemory() }
    };
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::buffer::PyBuffer_Release(&mut second);
        crate::api::buffer::PyBuffer_Release(&mut first);
    });
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_Check(op: *mut PyObject) -> c_int {
    unsafe {
        crate::bridge::is_semantic_instance_of(op, &raw mut crate::abi_types::PyByteArray_Type)
            as c_int
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_CheckExact(op: *mut PyObject) -> c_int {
    unsafe {
        crate::bridge::is_exact_semantic_type(op, &raw mut crate::abi_types::PyByteArray_Type)
            as c_int
    }
}

/// A semantic bytearray predicate never authorizes a C payload dereference.
/// Only a foreign object with an actual bytearray-layout type has this prefix;
/// bridge views always use the runtime backing hooks, even for subclasses.
unsafe fn foreign_bytearray_layout(op: *mut PyObject) -> Option<*mut PyByteArrayObject> {
    if op.is_null() || GLOBAL_BRIDGE.molt_handle_for_pyobj(op).is_some() {
        return None;
    }
    let ty = unsafe { (*op).ob_type };
    if ty.is_null()
        || unsafe { (*ty).tp_basicsize } < std::mem::size_of::<PyByteArrayObject>() as isize
        || (ty != &raw mut crate::abi_types::PyByteArray_Type
            && unsafe {
                crate::api::typeobj::PyType_IsSubtype(
                    ty,
                    &raw mut crate::abi_types::PyByteArray_Type,
                )
            } == 0)
    {
        return None;
    }
    Some(op.cast())
}

unsafe fn bytearray_type_error() {
    unsafe {
        set_exc(
            (&raw mut crate::abi_types::PyExc_TypeError).cast(),
            "bytearray object expected",
        );
    }
}

unsafe fn bytearray_data(op: *mut PyObject) -> Option<(*mut c_char, Py_ssize_t)> {
    if let Some(handle) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) {
        let mut len = 0;
        let data = unsafe { (hooks_or_stubs().bytearray_data)(handle.bits(), &mut len) };
        let status = if data.is_null() { -1 } else { 0 };
        if unsafe { crate::api::errors::check_native_status(status, "bytearray_data") } < 0 {
            return None;
        }
        let Ok(len) = Py_ssize_t::try_from(len) else {
            unsafe {
                set_exc(
                    (&raw mut crate::abi_types::PyExc_OverflowError).cast(),
                    "bytearray is too large",
                )
            };
            return None;
        };
        return Some((data.cast(), len));
    }
    let Some(obj) = (unsafe { foreign_bytearray_layout(op) }) else {
        unsafe { bytearray_type_error() };
        return None;
    };
    let len = unsafe { (*obj).ob_base.ob_size };
    let mut data = unsafe { (*obj).ob_start };
    if len == 0 && data.is_null() && unsafe { (*obj).ob_bytes.is_null() && (*obj).ob_exports == 0 }
    {
        // A freshly allocated foreign subtype may have a zeroed empty prefix.
        // Establish its owned sentinel before the first pointer or export escapes.
        data = unsafe { crate::api::memory::PyMem_Calloc(1, 1) }.cast();
        if data.is_null() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return None;
        }
        unsafe {
            (*obj).ob_bytes = data;
            (*obj).ob_start = data;
            (*obj).ob_alloc = 1;
        }
    }
    if len < 0 || data.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return None;
    }
    Some((data, len))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_AsString(op: *mut PyObject) -> *mut c_char {
    unsafe { bytearray_data(op) }.map_or(ptr::null_mut(), |(data, _)| data)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_Size(op: *mut PyObject) -> Py_ssize_t {
    unsafe { bytearray_data(op) }.map_or(-1, |(_, len)| len)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_Resize(op: *mut PyObject, len: Py_ssize_t) -> c_int {
    if len < 0 {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                "negative count",
            )
        };
        return -1;
    }
    if let Some(handle) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) {
        let status = unsafe { (hooks_or_stubs().bytearray_resize)(handle.bits(), len as usize) };
        return unsafe { crate::api::errors::check_native_status(status, "bytearray_resize") };
    }
    let Some(obj) = (unsafe { foreign_bytearray_layout(op) }) else {
        unsafe { bytearray_type_error() };
        return -1;
    };
    let old_len = unsafe { (*obj).ob_base.ob_size };
    if old_len == len {
        return 0;
    }
    if unsafe { (*obj).ob_exports } != 0 {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_BufferError).cast(),
                "Existing exports of data: object cannot be re-sized",
            )
        };
        return -1;
    }
    if old_len < 0 || (old_len != 0 && unsafe { (*obj).ob_start.is_null() }) {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let Some(alloc) = len.checked_add(1) else {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return -1;
    };
    let data = unsafe { crate::api::memory::PyMem_Calloc(1, alloc as usize) }.cast::<c_char>();
    if data.is_null() {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return -1;
    }
    // Publish only after allocation succeeds; the old buffer stays valid on failure.
    let copied = old_len.min(len) as usize;
    if copied != 0 {
        unsafe { ptr::copy_nonoverlapping((*obj).ob_start, data, copied) };
    }
    unsafe {
        crate::api::memory::PyMem_Free((*obj).ob_bytes.cast());
        (*obj).ob_bytes = data;
        (*obj).ob_start = data;
        (*obj).ob_alloc = alloc;
        (*obj).ob_base.ob_size = len;
    }
    0
}

/// Native exports retain their ob_exports count; managed exports retain the
/// canonical runtime lease. Neither route creates a detached mutable copy.
pub unsafe extern "C" fn molt_bytearray_getbuffer(
    object: *mut PyObject,
    view: *mut crate::abi_types::Py_buffer,
    flags: c_int,
) -> c_int {
    if GLOBAL_BRIDGE.molt_handle_for_pyobj(object).is_some() {
        // Check physical managed bytearray storage before generic buffer dispatch.
        if unsafe { bytearray_data(object) }.is_none() {
            return -1;
        }
        return unsafe { crate::api::buffer::PyObject_GetBuffer(object, view, flags) };
    }
    let Some(bytearray) = (unsafe { foreign_bytearray_layout(object) }) else {
        unsafe { bytearray_type_error() };
        return -1;
    };
    let Some((data, len)) = (unsafe { bytearray_data(object) }) else {
        return -1;
    };
    if unsafe { (*bytearray).ob_exports } == Py_ssize_t::MAX {
        unsafe {
            set_exc(
                (&raw mut crate::abi_types::PyExc_OverflowError).cast(),
                "too many exported buffers",
            )
        };
        return -1;
    }
    let result =
        unsafe { crate::api::buffer::PyBuffer_FillInfo(view, object, data.cast(), len, 0, flags) };
    if result == 0 {
        unsafe { (*bytearray).ob_exports += 1 }
    };
    result
}

pub unsafe extern "C" fn molt_bytearray_releasebuffer(
    object: *mut PyObject,
    view: *mut crate::abi_types::Py_buffer,
) {
    if GLOBAL_BRIDGE.molt_handle_for_pyobj(object).is_some() {
        unsafe { crate::api::buffer::release_managed_export(view) };
        return;
    }
    let Some(bytearray) = (unsafe { foreign_bytearray_layout(object) }) else {
        return;
    };
    assert!(
        unsafe { (*bytearray).ob_exports } > 0,
        "unbalanced bytearray buffer release"
    );
    unsafe { (*bytearray).ob_exports -= 1 };
}

pub unsafe extern "C" fn molt_bytearray_dealloc(op: *mut PyObject) {
    let Some(obj) = (unsafe { foreign_bytearray_layout(op) }) else {
        return;
    };
    crate::api::errors::with_preserved_error(|| unsafe {
        let Some(deallocation) = crate::api::typeobj::NativeDeallocation::storage(op) else {
            return;
        };
        let bytes = std::mem::replace(&mut (*obj).ob_bytes, ptr::null_mut());
        (*obj).ob_start = ptr::null_mut();
        crate::api::memory::PyMem_Free(bytes.cast());
        deallocation.finish();
    });
}

#[cfg(test)]
mod tests {
    use super::{
        _PyUnicode_IsAlpha, _PyUnicode_IsDecimalDigit, _PyUnicode_IsDigit, _PyUnicode_IsLinebreak,
        _PyUnicode_IsLowercase, _PyUnicode_IsNumeric, _PyUnicode_IsPrintable,
        _PyUnicode_IsTitlecase, _PyUnicode_IsUppercase, _PyUnicode_IsWhitespace,
        encoding_name_matches, latin1_encode_utf8_bytes, python_text_to_ucs4,
        unicode_kind_data_to_utf8,
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
    fn unicode_kind_data_preserves_surrogates_and_rejects_out_of_range_codepoints() {
        let surrogate = [0xd800u16];
        assert_eq!(
            unicode_kind_data_to_utf8(2, surrogate.as_ptr().cast(), 1),
            Some(vec![0xed, 0xa0, 0x80])
        );
        let too_large = [0x110000u32];
        assert!(unicode_kind_data_to_utf8(4, too_large.as_ptr().cast(), 1).is_none());
    }

    #[test]
    fn python_text_to_ucs4_preserves_codepoints() {
        assert_eq!(
            python_text_to_ucs4("a\u{3c0}\u{1f642}".as_bytes()).unwrap(),
            [0x61, 0x03c0, 0x1f642]
        );
        assert!(python_text_to_ucs4(b"\xff").is_none());
        assert_eq!(
            python_text_to_ucs4(b"\xed\xa0\x80\xed\xb0\x80\0").unwrap(),
            [0xd800, 0xdc00, 0]
        );
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

#[cfg(test)]
mod ascii_whitespace_table_tests {
    use super::_Py_ascii_whitespace;

    /// The `_Py_ascii_whitespace[128]` table is exactly CPython's: 1 for
    /// 0x09-0x0D, 0x1C-0x1F and 0x20; 0 for every other byte. A drift here
    /// silently changes bytes `.strip()`/`.split()` classification for a C
    /// extension that indexes the table directly.
    #[test]
    fn ascii_whitespace_table_matches_cpython() {
        assert_eq!(_Py_ascii_whitespace.len(), 128);
        for (i, &v) in _Py_ascii_whitespace.iter().enumerate() {
            let expect = matches!(i, 0x09..=0x0d | 0x1c..=0x1f | 0x20);
            assert_eq!(v != 0, expect, "byte {i:#04x} whitespace flag");
        }
    }
}
