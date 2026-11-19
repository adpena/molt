// String percent-format runtime shared by `%` modulo dispatch.
// This owns the parser/conversion authority for legacy `%` formatting
// while ops_arith.rs keeps only arithmetic entrypoint dispatch.

use super::*;
use crate::BuiltinExc;
use crate::object::ops_format::{
    FormatOutput, FormatWriter, format_float_value_with_spec, format_obj_output,
    format_obj_str_output,
};
use crate::object::ops_string::wtf8_step;
use crate::object::tuple_storage::{TupleComparisonItem, TupleStorage, is_tuple};
use molt_cpython_abi::abi_types::PyObject;
use molt_cpython_abi::api::{abstract_number, errors, numbers, object, strings, typeobj};
use molt_cpython_abi::hooks::DecodedHandleResult;

fn percent_native(bits: u64) -> Option<*mut PyObject> {
    let ptr = obj_from_bits(bits).as_ptr()?;
    (unsafe { object_type_id(ptr) } == TYPE_ID_FOREIGN).then(|| {
        std::ptr::with_exposed_provenance_mut(unsafe {
            crate::object::foreign::foreign_ptr_from_obj(ptr)
        })
    })
}

fn percent_native_failed(py: &PyToken<'_>, operation: &str) -> bool {
    if errors::raised_error_pending() {
        crate::cpython_abi_hooks::propagate_native_failure(py, operation);
    }
    exception_pending(py)
}

/// Keep the conversion result through integer conversion, including precision
/// failures, then retire it before width padding. This is _PyUnicode_FormatLong's
/// iobj lifetime, without projecting a
/// managed operand into a process-static C view.
enum PercentIntegerOwner<'guard> {
    Native { value: *mut PyObject },
    Managed { _value: PtrDropGuard<'guard> },
}

impl Drop for PercentIntegerOwner<'_> {
    fn drop(&mut self) {
        if let Self::Native { value } = self {
            crate::object::foreign::retire_native_reference(|| unsafe {
                molt_cpython_abi::api::refcount::Py_XDECREF(*value);
            });
        }
    }
}

struct PercentInteger<'guard> {
    value: BigInt,
    _owner: Option<PercentIntegerOwner<'guard>>,
}

impl<'guard> PercentInteger<'guard> {
    fn value(value: BigInt) -> Self {
        Self {
            value,
            _owner: None,
        }
    }

    fn managed(py: &'guard PyToken<'guard>, bits: u64) -> Option<Self> {
        let owner = obj_from_bits(bits)
            .as_ptr()
            .map(|ptr| PercentIntegerOwner::Managed {
                _value: PtrDropGuard::new(py, ptr),
            });
        if exception_pending(py) {
            return None;
        }
        let value = crate::builtins::numbers::integer_storage_bigint(py, bits).ok()??;
        Some(Self {
            value,
            _owner: owner,
        })
    }
}

fn percent_native_integer<'guard>(
    py: &'guard PyToken<'guard>,
    native: *mut PyObject,
    conv: u8,
) -> Option<PercentInteger<'guard>> {
    let converted = unsafe {
        if numbers::PyLong_Check(native) != 0 {
            object::Py_NewRef(native)
        } else if matches!(conv, b'd' | b'i' | b'u') {
            // PyNumber_Long also parses strings; the Unicode formatter's
            // preceding PyNumber_Check deliberately excludes that protocol.
            if numbers::PyNumber_Check(native) == 0 {
                if !percent_native_failed(py, "percent numeric admission") {
                    errors::PyErr_SetString(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
                        c"a number is required".as_ptr(),
                    );
                }
                std::ptr::null_mut()
            } else {
                abstract_number::PyNumber_Long(native)
            }
        } else {
            abstract_number::number_index(native)
        }
    };
    let owner = PercentIntegerOwner::Native { value: converted };
    if converted.is_null() || percent_native_failed(py, "percent integer conversion") {
        if converted.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(py, "percent integer conversion");
        }
        return None;
    }
    let result = unsafe { numbers::native_numeric_payload(converted) };
    let bits = match result.decode() {
        DecodedHandleResult::Ok(bits) => bits,
        _ => {
            crate::cpython_abi_hooks::propagate_native_failure(py, "percent integer payload");
            return None;
        }
    };
    let _payload = obj_from_bits(bits)
        .as_ptr()
        .map(|__ptr| PtrDropGuard::new(py, __ptr));
    let value = crate::builtins::numbers::index_bigint_integral_bits(bits)?;
    Some(PercentInteger {
        value,
        _owner: Some(owner),
    })
}

fn percent_type_name(py: &PyToken<'_>, obj: MoltObject) -> String {
    percent_native(obj.bits()).map_or_else(
        || type_name(py, obj).into_owned(),
        |native| unsafe { typeobj::object_type_name_with_precision(native, 200) },
    )
}

/// CPython 3.15 rewrote the `%` diagnostics (`Objects/unicode_format.c`):
/// conversion errors name their argument (`FORMAT_ERROR`), argument-count
/// errors give the counts and parse errors give the field's position. The
/// target is read only when a field fails.
fn percent_modern(py: &PyToken<'_>) -> bool {
    crate::object::ops_sys::runtime_target_at_least(py, 3, 15)
}

/// The argument a 3.15 conversion error names: a mapping key (`%R`), the
/// 1-based tuple position, or the one non-tuple argument.
#[derive(Clone, Copy)]
enum PercentArgLabel<'k> {
    Key(&'k [u8]),
    Index(usize),
    Single,
}

impl PercentArgLabel<'_> {
    fn prefix(self, py: &PyToken<'_>) -> Vec<u8> {
        match self {
            Self::Key(key) => {
                let repr = crate::object::ops_format::format_string_repr_bytes(
                    crate::object::ops::unicode_data(py),
                    key,
                );
                [b"format argument ".as_slice(), repr.as_bytes(), b": "].concat()
            }
            Self::Index(index) => format!("format argument {index}: ").into_bytes(),
            Self::Single => b"format argument: ".to_vec(),
        }
    }
}

/// Raise `exc` with `prefix` and `parts` joined; the parts may hold
/// surrogate-carrying type names.
fn percent_raise_bytes<T: crate::builtins::exceptions::ExceptionSentinel>(
    py: &PyToken<'_>,
    exc: BuiltinExc,
    parts: &[&[u8]],
) -> T {
    crate::object::ops_format::FormatError::Bytes(exc, parts.concat()).raise(py)
}

/// `%T`: the type's current fully qualified name, for managed and native
/// objects alike (`format_diagnostic_type_name_bytes`).
fn percent_qualified_type_name(py: &PyToken<'_>, bits: u64) -> Vec<u8> {
    crate::object::ops_format::format_diagnostic_type_name_bytes(py, obj_from_bits(bits))
}

/// The code point index of a byte offset in the format text.
fn percent_codepoint_index(text: &[u8], byte_idx: usize) -> usize {
    let mut cursor = 0;
    let mut index = 0;
    while cursor < byte_idx {
        cursor = wtf8_step(text, cursor, false)
            .expect("Python string codepoint boundary")
            .0;
        index += 1;
    }
    index
}

/// A parse error: the legacy text, or 3.15's text with the field position.
fn percent_parse_error<T: crate::builtins::exceptions::ExceptionSentinel>(
    py: &PyToken<'_>,
    exc: BuiltinExc,
    legacy: &str,
    modern: &str,
    text: &[u8],
    field_start: usize,
) -> T {
    if percent_modern(py) {
        let position = percent_codepoint_index(text, field_start);
        raise_exception(py, exc, &format!("{modern} at position {position}"))
    } else {
        raise_exception(py, exc, legacy)
    }
}

#[derive(Clone, Copy, Default)]
struct PercentFormatFlags {
    left_adjust: bool,
    sign_plus: bool,
    sign_space: bool,
    zero_pad: bool,
    alternate: bool,
}

fn percent_rhs_allows_unused_non_tuple(_py: &PyToken<'_>, rhs: MoltObject) -> Result<bool, ()> {
    let Some(ptr) = rhs.as_ptr() else {
        return Ok(false);
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_STRING
            || is_tuple(rhs.bits())
            || percent_native(rhs.bits())
                .is_some_and(|native| strings::PyUnicode_Check(native) != 0)
        {
            return Ok(false);
        }
    }
    crate::object::ops::value_supports_mp_subscript(_py, rhs.bits())
}

#[derive(Clone, Copy)]
enum PercentField {
    Width,
    Precision,
}

impl PercentField {
    fn limit(self) -> usize {
        match self {
            Self::Width => isize::MAX as usize,
            Self::Precision => i32::MAX as usize,
        }
    }
    fn too_big(self) -> &'static str {
        match self {
            Self::Width => "width too big",
            Self::Precision => "precision too big",
        }
    }
}

fn percent_parse_usize(
    py: &PyToken<'_>,
    bytes: &[u8],
    idx: &mut usize,
    field: PercentField,
    field_start: usize,
) -> Option<usize> {
    let start = *idx;
    let mut out = 0usize;
    while *idx < bytes.len() && bytes[*idx].is_ascii_digit() {
        let digit = usize::from(bytes[*idx] - b'0');
        out = match out
            .checked_mul(10)
            .and_then(|value| value.checked_add(digit))
            .filter(|value| *value <= field.limit())
        {
            Some(value) => value,
            None => {
                return percent_parse_error(
                    py,
                    BuiltinExc::ValueError,
                    field.too_big(),
                    field.too_big(),
                    bytes,
                    field_start,
                );
            }
        };
        *idx += 1;
    }
    (*idx != start).then_some(out)
}

/// 3.15's '*' errors: `* requires int, not T` and `too big for width`.
fn percent_star_type_error(py: &PyToken<'_>, bits: u64, label: PercentArgLabel<'_>) -> Option<i64> {
    if !percent_modern(py) {
        return raise_exception(py, BuiltinExc::TypeError, "* wants int");
    }
    let name = percent_qualified_type_name(py, bits);
    percent_raise_bytes(
        py,
        BuiltinExc::TypeError,
        &[&label.prefix(py), b"* requires int, not ", &name],
    )
}

fn percent_star_overflow<T: crate::builtins::exceptions::ExceptionSentinel>(
    py: &PyToken<'_>,
    field: PercentField,
    label: PercentArgLabel<'_>,
) -> T {
    if percent_modern(py) {
        let what: &[u8] = match field {
            PercentField::Width => b"too big for width",
            PercentField::Precision => b"too big for precision",
        };
        return percent_raise_bytes(py, BuiltinExc::OverflowError, &[&label.prefix(py), what]);
    }
    raise_exception(
        py,
        BuiltinExc::OverflowError,
        match field {
            PercentField::Width => "Python int too large to convert to C ssize_t",
            PercentField::Precision => "Python int too large to convert to C int",
        },
    )
}

/// '*' accepts an actual int or subtype, never an arbitrary __index__ protocol.
fn percent_star_integer(
    py: &PyToken<'_>,
    bits: u64,
    field: PercentField,
    label: PercentArgLabel<'_>,
) -> Option<i64> {
    if let Some(native) = percent_native(bits) {
        if unsafe { numbers::PyLong_Check(native) } == 0 {
            return percent_star_type_error(py, bits, label);
        }
        let value = unsafe {
            match field {
                PercentField::Width => numbers::PyLong_AsSsize_t(native) as i64,
                PercentField::Precision => numbers::_PyLong_AsInt(native) as i64,
            }
        };
        return (!percent_native_failed(py, "percent star field")).then_some(value);
    }
    let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(bits) else {
        return percent_star_type_error(py, bits, label);
    };
    let Some(value) = value.to_i64() else {
        return percent_star_overflow(py, field, label);
    };
    let admitted = match field {
        PercentField::Width => isize::try_from(value).is_ok(),
        PercentField::Precision => i32::try_from(value).is_ok(),
    };
    if !admitted {
        return percent_star_overflow(py, field, label);
    }
    Some(value)
}

fn percent_unsupported_char(
    py: &PyToken<'_>,
    text: &[u8],
    field_start: usize,
    byte_idx: usize,
    codepoint: u32,
) -> Option<Vec<u8>> {
    // Parsing keeps byte offsets for lossless slices; Python diagnostics count
    // codepoints, including independently encoded surrogate codepoints.
    let index = percent_codepoint_index(text, byte_idx);
    if percent_modern(py) {
        let start = percent_codepoint_index(text, field_start);
        let message = if codepoint < 128 && (codepoint as u8).is_ascii_alphabetic() {
            format!(
                "unsupported format %{} at position {start}",
                codepoint as u8 as char
            )
        } else if codepoint == u32::from(b'\'') {
            format!(
                "stray % at position {start} or unexpected format character \"'\" at position {index}"
            )
        } else if (32..127).contains(&codepoint) {
            format!(
                "stray % at position {start} or unexpected format character '{}' at position {index}",
                codepoint as u8 as char
            )
        } else if let Some(ch) = char::from_u32(codepoint)
            .filter(|_| crate::object::ops::unicode_data(py).is_printable(codepoint))
        {
            format!(
                "stray % at position {start} or unexpected format character '{ch}' (U+{codepoint:04X}) at position {index}"
            )
        } else {
            format!(
                "stray % at position {start} or unexpected format character U+{codepoint:04X} at position {index}"
            )
        };
        return raise_exception(py, crate::BuiltinExc::ValueError, &message);
    }
    // unicodeobject.c deliberately prints '?' outside this ASCII range.
    let display = if (31..=126).contains(&codepoint) {
        char::from_u32(codepoint).expect("ASCII format character")
    } else {
        '?'
    };
    let message =
        format!("unsupported format character '{display}' (0x{codepoint:x}) at index {index}");
    raise_exception::<Option<Vec<u8>>>(py, crate::BuiltinExc::ValueError, &message)
}

fn percent_apply_numeric_width(
    py: &PyToken<'_>,
    prefix: &str,
    body: String,
    width: Option<usize>,
    left_adjust: bool,
    zero_pad: bool,
) -> Option<String> {
    let spec = FormatSpec {
        width,
        align: Some(if left_adjust {
            '<'
        } else if zero_pad {
            '='
        } else {
            '>'
        }),
        fill: u32::from(if zero_pad && !left_adjust { '0' } else { ' ' }),
        ..FormatSpec::default()
    };
    match crate::object::ops_format::apply_alignment(prefix, &body, &spec, '>') {
        Ok(bytes) => Some(String::from_utf8(bytes).expect("integer percent presentation is ASCII")),
        Err(error) => error.raise(py),
    }
}

fn percent_integer_precision(
    py: &PyToken<'_>,
    body: String,
    precision: Option<usize>,
) -> Option<String> {
    let spec = FormatSpec {
        width: precision,
        fill: u32::from('0'),
        ..FormatSpec::default()
    };
    match crate::object::ops_format::apply_alignment("", &body, &spec, '>') {
        Ok(bytes) => Some(String::from_utf8(bytes).expect("integer percent presentation is ASCII")),
        Err(error) => error.raise(py),
    }
}

fn percent_integer_precision_admit(py: &PyToken<'_>, precision: Option<usize>) -> Option<()> {
    if precision.is_some_and(|precision| precision > i32::MAX as usize - 3) {
        return raise_exception(py, BuiltinExc::OverflowError, "precision too large");
    }
    Some(())
}

/// 3.15: `format argument ...: %d requires a real number, not T`.
fn percent_raise_modern_type_error<T: crate::builtins::exceptions::ExceptionSentinel>(
    py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
    requirement: &[u8],
    label: PercentArgLabel<'_>,
) -> T {
    let name = percent_qualified_type_name(py, obj.bits());
    percent_raise_bytes(
        py,
        BuiltinExc::TypeError,
        &[
            &label.prefix(py),
            b"%",
            &[conv],
            b" requires ",
            requirement,
            b", not ",
            &name,
        ],
    )
}

fn percent_raise_real_type_error_decimal<T>(
    _py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
    label: PercentArgLabel<'_>,
) -> Option<T> {
    if percent_modern(_py) {
        return percent_raise_modern_type_error(_py, obj, conv, b"a real number", label);
    }
    let conv_ch = conv as char;
    let msg = format!(
        "%{conv_ch} format: a real number is required, not {}",
        percent_type_name(_py, obj)
    );
    raise_exception(_py, BuiltinExc::TypeError, &msg)
}

fn percent_raise_integer_type_error<T>(
    _py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
    label: PercentArgLabel<'_>,
) -> Option<T> {
    if percent_modern(_py) {
        return percent_raise_modern_type_error(_py, obj, conv, b"an integer", label);
    }
    let conv_ch = conv as char;
    let msg = format!(
        "%{conv_ch} format: an integer is required, not {}",
        percent_type_name(_py, obj)
    );
    raise_exception(_py, BuiltinExc::TypeError, &msg)
}

fn percent_raise_char_type_error(
    _py: &PyToken<'_>,
    obj: MoltObject,
    label: PercentArgLabel<'_>,
) -> Option<FormatOutput> {
    if percent_modern(_py) {
        let detail = if let Some(ptr) = obj
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
        {
            let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
            let length = molt_obj_model::text_storage::code_points(bytes).count();
            format!("a string of length {length}").into_bytes()
        } else {
            percent_qualified_type_name(_py, obj.bits())
        };
        return percent_raise_bytes(
            _py,
            BuiltinExc::TypeError,
            &[
                &label.prefix(_py),
                b"%c requires an integer or a unicode character, not ",
                &detail,
            ],
        );
    }
    if crate::object::ops_sys::runtime_target_minor(_py) < 14 {
        return raise_exception(_py, BuiltinExc::TypeError, "%c requires int or char");
    }
    let message = if let Some(ptr) = obj
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
    {
        let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
        let length = molt_obj_model::text_storage::code_points(bytes).count();
        format!("%c requires an int or a unicode character, not a string of length {length}")
    } else {
        let name = crate::object::ops_format::format_diagnostic_type_name_bytes(_py, obj);
        if exception_pending(_py) {
            return None;
        }
        let mut message = Vec::new();
        for bytes in [
            b"%c requires an int or a unicode character, not ".as_slice(),
            name.as_slice(),
        ] {
            if let Err(error) = crate::object::ops_format::append_format_bytes(&mut message, bytes)
            {
                return error.raise(_py);
            }
        }
        return crate::object::ops_format::FormatError::Bytes(
            crate::BuiltinExc::TypeError,
            message,
        )
        .raise(_py);
    };
    raise_exception(_py, BuiltinExc::TypeError, &message)
}

fn percent_char_range_error<T: crate::builtins::exceptions::ExceptionSentinel>(
    py: &PyToken<'_>,
    label: PercentArgLabel<'_>,
) -> T {
    if percent_modern(py) {
        return percent_raise_bytes(
            py,
            BuiltinExc::OverflowError,
            &[&label.prefix(py), b"%c argument not in range(0x110000)"],
        );
    }
    raise_exception(
        py,
        BuiltinExc::OverflowError,
        "%c arg not in range(0x110000)",
    )
}

fn percent_char_from_bigint(
    py: &PyToken<'_>,
    value: BigInt,
    label: PercentArgLabel<'_>,
) -> Option<Vec<u8>> {
    if !(BigInt::zero()..=BigInt::from(0x10ffff)).contains(&value) {
        return percent_char_range_error(py, label);
    }
    match crate::object::ops_format::format_character_codepoint(&value) {
        Ok(bytes) => Some(bytes),
        Err(error) => error.raise(py),
    }
}

fn percent_decimal_float(py: &PyToken<'_>, value: f64) -> Option<BigInt> {
    if value.is_nan() {
        return raise_exception::<Option<BigInt>>(
            py,
            BuiltinExc::ValueError,
            "cannot convert float NaN to integer",
        );
    }
    if value.is_infinite() {
        return raise_exception::<Option<BigInt>>(
            py,
            BuiltinExc::OverflowError,
            "cannot convert float infinity to integer",
        );
    }
    Some(bigint_from_f64_trunc(value))
}

fn percent_decimal_from_obj<'guard>(
    py: &'guard PyToken<'guard>,
    bits: u64,
    conv: u8,
    label: PercentArgLabel<'_>,
) -> Option<PercentInteger<'guard>> {
    if let Some(native) = percent_native(bits) {
        return percent_native_integer(py, native, conv);
    }
    if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(bits) {
        return Some(PercentInteger::value(value));
    }
    let obj = obj_from_bits(bits);
    if builtin_operand(py, obj)
        && let Some(value) = as_float_extended(obj)
    {
        return percent_decimal_float(py, value).map(PercentInteger::value);
    }
    // Mirror PyNumber_Check before PyNumber_Long, using the existing type-level
    // admission authority without binding or executing descriptors.
    let mut numeric = false;
    for name in [
        crate::NameId::DunderIndex,
        crate::NameId::DunderInt,
        crate::NameId::DunderFloat,
    ] {
        numeric = unsafe { crate::builtins::attr::has_special_method(py, bits, name) }.ok()?;
        if numeric {
            break;
        }
    }
    if !numeric {
        return percent_raise_real_type_error_decimal(py, obj, conv, label);
    }
    // Use the same full conversion as PyNumber_Long: an admitted numeric
    // string/bytes subtype may reach int's text parser after absent protocols.
    let result = crate::molt_int_from_obj_attached_body(
        py,
        bits,
        MoltObject::none().bits(),
        MoltObject::from_bool(false).bits(),
    );
    PercentInteger::managed(py, result)
}

fn percent_integer_from_obj<'guard>(
    py: &'guard PyToken<'guard>,
    bits: u64,
    conv: u8,
    label: PercentArgLabel<'_>,
) -> Option<PercentInteger<'guard>> {
    if let Some(native) = percent_native(bits) {
        return percent_native_integer(py, native, conv);
    }
    if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(bits) {
        return Some(PercentInteger::value(value));
    }
    if let Some(result) = crate::builtins::numbers::index_from_number_protocol(py, bits) {
        return PercentInteger::managed(py, result);
    }
    if exception_pending(py) {
        None
    } else {
        percent_raise_integer_type_error(py, obj_from_bits(bits), conv, label)
    }
}

fn percent_char_from_obj(
    py: &PyToken<'_>,
    bits: u64,
    label: PercentArgLabel<'_>,
) -> Option<FormatOutput> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
    {
        let text = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
        if wtf8_step(text, 0, false).is_some_and(|(end, _)| end == text.len()) {
            // %c writes a character, never adopts a str subclass receiver.
            let mut bytes = Vec::new();
            if let Err(error) = crate::object::ops_format::append_format_bytes(&mut bytes, text) {
                return error.raise(py);
            }
            return Some(FormatOutput::Bytes(bytes));
        }
        return percent_raise_char_type_error(py, obj, label);
    }
    if let Some(native) = percent_native(bits) {
        let mut overflow = 0;
        // formatchar retires an __index__ result inside this converter, before
        // constructing its range error. It differs from formatlong's lifetime.
        let value = unsafe { numbers::PyLong_AsLongAndOverflow(native, &mut overflow) };
        if percent_native_failed(py, "percent character conversion") {
            return None;
        }
        if overflow != 0 || !(0..=0x10ffff).contains(&value) {
            return percent_char_range_error(py, label);
        }
        return percent_char_from_bigint(py, BigInt::from(value), label).map(Into::into);
    }
    if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(bits) {
        return percent_char_from_bigint(py, value, label).map(Into::into);
    }
    if let Some(result) = crate::builtins::numbers::index_from_number_protocol(py, bits) {
        let owner = obj_from_bits(result)
            .as_ptr()
            .map(|__ptr| PtrDropGuard::new(py, __ptr));
        let value = crate::builtins::numbers::integer_storage_bigint(py, result)
            .ok()
            .flatten();
        drop(owner);
        return percent_char_from_bigint(py, value?, label).map(Into::into);
    }
    if exception_pending(py) {
        None
    } else {
        percent_raise_char_type_error(py, obj, label)
    }
}

fn percent_numeric_prefix(is_negative: bool, flags: PercentFormatFlags) -> Option<char> {
    if is_negative {
        Some('-')
    } else if flags.sign_plus {
        Some('+')
    } else if flags.sign_space {
        Some(' ')
    } else {
        None
    }
}

fn percent_format_text(
    py: &PyToken<'_>,
    text: FormatOutput,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<FormatOutput> {
    match crate::object::ops_format::format_text_output(
        py,
        text,
        width,
        precision,
        b" ",
        if flags.left_adjust { '<' } else { '>' },
        !flags.sign_plus && !flags.sign_space,
    ) {
        Ok(output) => Some(output),
        Err(error) => error.raise(py),
    }
}

fn percent_format_decimal(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
    label: PercentArgLabel<'_>,
) -> Option<String> {
    let value = percent_decimal_from_obj(_py, value_bits, conv, label).or_else(|| {
        if percent_conversion_type_error(_py) {
            clear_exception(_py);
            percent_raise_real_type_error_decimal(_py, obj_from_bits(value_bits), conv, label)
        } else {
            None
        }
    })?;
    percent_integer_precision_admit(_py, precision)?;
    let negative = value.value.is_negative();
    let mut body =
        match crate::object::ops_format::format_bigint_decimal_magnitude(_py, &value.value) {
            Ok(body) => body,
            // Select the output error before retiring the converted integer owner.
            Err(error) => return error.raise(_py),
        };
    body = percent_integer_precision(_py, body, precision)?;
    drop(value);
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    let zero_pad = flags.zero_pad && !flags.left_adjust;
    percent_apply_numeric_width(
        _py,
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        zero_pad,
    )
}

fn percent_format_radix(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
    label: PercentArgLabel<'_>,
) -> Option<String> {
    let value = percent_integer_from_obj(_py, value_bits, conv, label).or_else(|| {
        if percent_conversion_type_error(_py) {
            clear_exception(_py);
            percent_raise_integer_type_error(_py, obj_from_bits(value_bits), conv, label)
        } else {
            None
        }
    })?;
    percent_integer_precision_admit(_py, precision)?;
    let negative = value.value.is_negative();
    let mut body = match conv {
        b'o' => value.value.abs().to_str_radix(8),
        b'x' | b'X' => value.value.abs().to_str_radix(16),
        _ => unreachable!("radix conversion admitted only o/x/X"),
    };
    if conv == b'X' {
        body = body.to_uppercase();
    }
    body = percent_integer_precision(_py, body, precision)?;
    drop(value);
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    if flags.alternate {
        match conv {
            b'o' => prefix.push_str("0o"),
            b'x' => prefix.push_str("0x"),
            b'X' => prefix.push_str("0X"),
            _ => {}
        }
    }
    percent_apply_numeric_width(
        _py,
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        flags.zero_pad && !flags.left_adjust,
    )
}

fn percent_format_float(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
    label: PercentArgLabel<'_>,
) -> Option<Vec<u8>> {
    let value = if let Some(native) = percent_native(value_bits) {
        // Physical float subtypes bypass overridden __float__ in PyFloat_AsDouble.
        let value = unsafe { numbers::PyFloat_AsDouble(native) };
        if percent_native_failed(_py, "percent float conversion") {
            return percent_float_type_error(_py, value_bits, conv, label);
        }
        value
    } else {
        match crate::builtins::numbers::float_as_double(_py, value_bits) {
            Some(value) => value,
            None => return percent_float_type_error(_py, value_bits, conv, label),
        }
    };
    let sign = if flags.sign_plus {
        Some('+')
    } else if flags.sign_space {
        Some(' ')
    } else {
        None
    };
    let align = if flags.left_adjust {
        Some('<')
    } else if flags.zero_pad {
        Some('=')
    } else {
        None
    };
    let spec = FormatSpec {
        fill: u32::from(if flags.zero_pad && !flags.left_adjust {
            '0'
        } else {
            ' '
        }),
        align,
        zero_flag: flags.zero_pad && !flags.left_adjust,
        sign,
        coerce_negative_zero: false,
        alternate: flags.alternate,
        width,
        grouping: None,
        fractional_grouping: None,
        precision,
        ty: Some(u32::from(conv)),
    };
    match format_float_value_with_spec(value, &spec) {
        Ok(text) => Some(text),
        Err(error) => error.raise(_py),
    }
}

/// 3.15's `formatfloat` replaces PyFloat_AsDouble's TypeError with its own.
fn percent_float_type_error<T>(
    py: &PyToken<'_>,
    value_bits: u64,
    conv: u8,
    label: PercentArgLabel<'_>,
) -> Option<T> {
    if percent_conversion_type_error(py) && percent_modern(py) {
        clear_exception(py);
        return percent_raise_modern_type_error(
            py,
            obj_from_bits(value_bits),
            conv,
            b"a real number",
            label,
        );
    }
    None
}

fn percent_format_ascii(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<FormatOutput> {
    let rendered_bits = molt_ascii_from_obj_attached_body(_py, value_bits);
    percent_format_text(
        _py,
        FormatOutput::OwnedString(rendered_bits),
        width,
        precision,
        flags,
    )
}

fn percent_format_char(
    py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    flags: PercentFormatFlags,
    label: PercentArgLabel<'_>,
) -> Option<FormatOutput> {
    let text = percent_char_from_obj(py, value_bits, label).or_else(|| {
        if percent_conversion_type_error(py) {
            clear_exception(py);
            percent_raise_char_type_error(py, obj_from_bits(value_bits), label)
        } else {
            None
        }
    })?;
    percent_format_text(py, text, width, None, flags)
}

fn percent_conversion_type_error(py: &PyToken<'_>) -> bool {
    let target = crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::TypeError);
    crate::builtins::exceptions::pending_exception_matches_type(py, target)
}

/// `format requires a mapping`; 3.15 adds `, not T`.
fn percent_requires_mapping<T: crate::builtins::exceptions::ExceptionSentinel>(
    py: &PyToken<'_>,
    rhs_bits: u64,
) -> T {
    if percent_modern(py) {
        let name = percent_qualified_type_name(py, rhs_bits);
        return percent_raise_bytes(
            py,
            BuiltinExc::TypeError,
            &[b"format requires a mapping, not ", &name],
        );
    }
    raise_exception(py, BuiltinExc::TypeError, "format requires a mapping")
}

fn percent_lookup_mapping_arg(_py: &PyToken<'_>, rhs_bits: u64, key: &[u8]) -> Option<(u64, bool)> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let Some(rhs_ptr) = rhs_obj.as_ptr() else {
        return percent_requires_mapping(_py, rhs_bits);
    };
    unsafe {
        let rhs_type = object_type_id(rhs_ptr);
        if is_tuple(rhs_bits) {
            return percent_requires_mapping(_py, rhs_bits);
        }
        let key_ptr = alloc_string(_py, key);
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        if rhs_type == TYPE_ID_DICT {
            let found = match dict_get_in_place(_py, rhs_ptr, key_bits) {
                Ok(found) => found,
                Err(()) => {
                    dec_ref_bits(_py, key_bits);
                    return None;
                }
            };
            if let Some(bits) = found {
                dec_ref_bits(_py, key_bits);
                return Some((bits, false));
            }
            raise_key_error_with_key::<()>(_py, key_bits);
            dec_ref_bits(_py, key_bits);
            return None;
        }
        let bits = molt_index_attached_body(_py, rhs_bits, key_bits);
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return None;
        }
        Some((bits, true))
    }
}

/// Mapping fields replace the active argument cursor before option parsing.
/// Keep their returned reference through the next field, including error exits.
enum PercentArgument<'tuple, 'a, 'py> {
    Tuple(TupleComparisonItem<'tuple, 'a, 'py>),
    Value(u64),
}

impl PercentArgument<'_, '_, '_> {
    fn bits(&self) -> u64 {
        match self {
            Self::Tuple(item) => item.bits(),
            Self::Value(bits) => *bits,
        }
    }
}

struct PercentArguments<'a, 'py> {
    source: u64,
    current: u64,
    tuple: Option<TupleStorage<'a, 'py>>,
    tuple_len: usize,
    index: usize,
    consumed: bool,
    mapping: bool,
    /// A field has looked up a mapping key (CPython's `arglen < -1`).
    keyed: bool,
    mapped_owner: Option<PtrDropGuard<'a>>,
}

impl<'a, 'py> PercentArguments<'a, 'py> {
    /// `not enough arguments for format string`; 3.15 adds `(got N)`.
    fn not_enough<T: crate::builtins::exceptions::ExceptionSentinel>(&self, py: &PyToken<'_>) -> T {
        let message = "not enough arguments for format string";
        if percent_modern(py) {
            let got = if self.tuple.is_some() {
                self.tuple_len
            } else {
                1
            };
            return raise_exception(py, BuiltinExc::TypeError, &format!("{message} (got {got})"));
        }
        raise_exception(py, BuiltinExc::TypeError, message)
    }

    /// `allow_one` is false for a '*' field: from 3.15 a non-tuple argument
    /// fills only the conversion (`unicode_format_getnextarg`).
    fn next(&mut self, py: &PyToken<'_>, allow_one: bool) -> Option<PercentArgument<'_, 'a, 'py>> {
        if let Some(tuple) = &self.tuple {
            if self.index >= self.tuple_len {
                return self.not_enough(py);
            }
            let item = tuple.comparison_item(self.index)?;
            self.index += 1;
            return Some(PercentArgument::Tuple(item));
        }
        if self.consumed || (!allow_one && percent_modern(py)) {
            return self.not_enough(py);
        }
        self.consumed = true;
        Some(PercentArgument::Value(self.current))
    }

    /// The argument the next `next` returns, as a 3.15 error names it.
    fn upcoming_label<'k>(&self, key: Option<&'k [u8]>) -> PercentArgLabel<'k> {
        match key {
            Some(key) => PercentArgLabel::Key(key),
            None if self.tuple.is_some() => PercentArgLabel::Index(self.index + 1),
            None => PercentArgLabel::Single,
        }
    }

    fn mapped(&mut self, py: &'a PyToken<'py>, key: &[u8]) -> Option<()> {
        self.mapped_owner = None;
        self.current = self.source;
        let (bits, owned) = percent_lookup_mapping_arg(py, self.source, key)?;
        self.mapped_owner = obj_from_bits(bits).as_ptr().map(|ptr| {
            if !owned {
                inc_ref_bits(py, bits);
            }
            PtrDropGuard::new(py, ptr)
        });
        self.current = bits;
        self.tuple = None;
        self.consumed = false;
        self.keyed = true;
        Some(())
    }
}

fn percent_append(py: &PyToken<'_>, out: &mut FormatWriter<'_, '_>, bytes: &[u8]) -> Option<()> {
    match out.append_bytes(bytes) {
        Ok(()) => Some(()),
        Err(error) => error.raise(py),
    }
}

pub(super) fn string_percent_format_impl(
    _py: &PyToken<'_>,
    text: &[u8],
    receiver_bits: u64,
    rhs_bits: u64,
) -> Option<FormatOutput> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let tuple = TupleStorage::from_bits(_py, rhs_bits);
    let tuple_len = match &tuple {
        Some(tuple) => tuple.len()?,
        None => 0,
    };
    let mapping = percent_rhs_allows_unused_non_tuple(_py, rhs_obj).ok()?;
    let mut arguments = PercentArguments {
        source: rhs_bits,
        current: rhs_bits,
        tuple,
        tuple_len,
        index: 0,
        consumed: false,
        mapping,
        keyed: false,
        mapped_owner: None,
    };
    let bytes = text;
    let mut out = FormatWriter::new(_py);
    let mut literal_start = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            idx += 1;
            continue;
        }
        percent_append(_py, &mut out, &text[literal_start..idx])?;
        let field_start = idx;
        idx += 1;
        if idx >= bytes.len() {
            return percent_parse_error(
                _py,
                BuiltinExc::ValueError,
                "incomplete format",
                "stray %",
                text,
                field_start,
            );
        }
        if bytes[idx] == b'%' {
            percent_append(_py, &mut out, b"%")?;
            idx += 1;
            literal_start = idx;
            continue;
        }
        let mut key: Option<&[u8]> = None;
        if bytes[idx] == b'(' {
            if !arguments.mapping {
                return percent_requires_mapping(_py, rhs_bits);
            }
            let key_start = idx + 1;
            let mut key_end = key_start;
            let mut nesting = 1usize;
            while key_end < bytes.len() {
                match bytes[key_end] {
                    b'(' => nesting += 1,
                    b')' => nesting -= 1,
                    _ => {}
                }
                if nesting == 0 {
                    break;
                }
                key_end += 1;
            }
            if key_end >= bytes.len() {
                return percent_parse_error(
                    _py,
                    BuiltinExc::ValueError,
                    "incomplete format key",
                    "stray % or incomplete format key",
                    text,
                    field_start,
                );
            }
            arguments.mapped(_py, &text[key_start..key_end])?;
            key = Some(&text[key_start..key_end]);
            idx = key_end + 1;
        } else if arguments.keyed && percent_modern(_py) {
            let position = percent_codepoint_index(text, field_start);
            return raise_exception(
                _py,
                BuiltinExc::ValueError,
                &format!("format requires a parenthesised mapping key at position {position}"),
            );
        }
        let mut flags = PercentFormatFlags::default();
        loop {
            if idx >= bytes.len() {
                return percent_parse_error(
                    _py,
                    BuiltinExc::ValueError,
                    "incomplete format",
                    "stray %",
                    text,
                    field_start,
                );
            }
            match bytes[idx] {
                b'-' => flags.left_adjust = true,
                b'+' => flags.sign_plus = true,
                b' ' => flags.sign_space = true,
                b'0' => flags.zero_pad = true,
                b'#' => flags.alternate = true,
                _ => break,
            }
            idx += 1;
        }
        let mut width = if idx < bytes.len() && bytes[idx].is_ascii_digit() {
            percent_parse_usize(_py, bytes, &mut idx, PercentField::Width, field_start)
        } else {
            None
        };
        if exception_pending(_py) {
            return None;
        }
        if width.is_none() && idx < bytes.len() && bytes[idx] == b'*' {
            idx += 1;
            percent_star_admitted(_py, &arguments, text, field_start)?;
            let label = arguments.upcoming_label(None);
            let width_arg = arguments.next(_py, false)?;
            let width_val =
                percent_star_integer(_py, width_arg.bits(), PercentField::Width, label)?;
            if exception_pending(_py) {
                return None;
            }
            if width_val < 0 {
                // CPython's signed field cannot represent -PY_SSIZE_T_MIN.
                // 3.15 rejects it; before, that sentinel leaves the natural
                // width rather than padding.
                if width_val == isize::MIN as i64 && percent_modern(_py) {
                    return percent_star_overflow(_py, PercentField::Width, label);
                }
                flags.left_adjust = true;
                width = width_val
                    .checked_neg()
                    .and_then(|value| isize::try_from(value).ok())
                    .map(|value| value as usize);
            } else {
                let Ok(width_usize) = usize::try_from(width_val) else {
                    return raise_exception::<Option<FormatOutput>>(
                        _py,
                        BuiltinExc::OverflowError,
                        "width too big",
                    );
                };
                width = Some(width_usize);
            }
        }
        let mut precision: Option<usize> = None;
        if idx < bytes.len() && bytes[idx] == b'.' {
            idx += 1;
            if idx < bytes.len() && bytes[idx] == b'*' {
                idx += 1;
                percent_star_admitted(_py, &arguments, text, field_start)?;
                let label = arguments.upcoming_label(None);
                let prec_arg = arguments.next(_py, false)?;
                let prec_val =
                    percent_star_integer(_py, prec_arg.bits(), PercentField::Precision, label)?;
                if exception_pending(_py) {
                    return None;
                }
                if prec_val <= 0 {
                    precision = Some(0);
                } else {
                    let Ok(prec_usize) = usize::try_from(prec_val) else {
                        return raise_exception::<Option<FormatOutput>>(
                            _py,
                            BuiltinExc::OverflowError,
                            "precision too big",
                        );
                    };
                    precision = Some(prec_usize);
                }
            } else {
                precision = Some(
                    percent_parse_usize(_py, bytes, &mut idx, PercentField::Precision, field_start)
                        .unwrap_or(0),
                );
            }
        }
        if exception_pending(_py) {
            return None;
        }
        if idx < bytes.len() && (bytes[idx] == b'h' || bytes[idx] == b'l' || bytes[idx] == b'L') {
            idx += 1;
        }
        if idx >= bytes.len() {
            return percent_parse_error(
                _py,
                BuiltinExc::ValueError,
                "incomplete format",
                "stray %",
                text,
                field_start,
            );
        }
        let conv_idx = idx;
        let conv = bytes[idx];
        let (next_idx, codepoint) =
            wtf8_step(bytes, idx, false).expect("Python format conversion codepoint");
        idx = next_idx;
        let label = arguments.upcoming_label(key);
        let value_arg = arguments.next(_py, true)?;
        let value_bits = value_arg.bits();
        let rendered = match conv {
            b's' => percent_format_text(
                _py,
                format_obj_str_output(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            ),
            b'r' => percent_format_text(
                _py,
                format_obj_output(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            ),
            b'a' => percent_format_ascii(_py, value_bits, width, precision, flags),
            b'c' => percent_format_char(_py, value_bits, width, flags, label),
            b'd' | b'i' | b'u' => {
                percent_format_decimal(_py, value_bits, width, precision, flags, conv, label)
                    .map(Into::into)
            }
            b'o' | b'x' | b'X' => {
                percent_format_radix(_py, value_bits, width, precision, flags, conv, label)
                    .map(Into::into)
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                percent_format_float(_py, value_bits, width, precision, flags, conv, label)
                    .map(Into::into)
            }
            _ => percent_unsupported_char(_py, bytes, field_start, conv_idx, codepoint)
                .map(Into::into),
        };
        if exception_pending(_py) {
            return None;
        }
        let rendered = rendered?;
        if let Err(error) = out.append_output(rendered, idx == bytes.len()) {
            return error.raise(_py);
        }
        literal_start = idx;
    }
    if let Err(error) = out.append_literal(&text[literal_start..], Some(receiver_bits), true) {
        return error.raise(_py);
    }
    let unused = if arguments.tuple.is_some() {
        (arguments.index < arguments.tuple_len).then_some((arguments.index, arguments.tuple_len))
    } else {
        (!arguments.consumed && !arguments.mapping).then_some((0, 1))
    };
    if let Some((required, got)) = unused {
        let message = "not all arguments converted during string formatting";
        if percent_modern(_py) {
            return raise_exception::<Option<FormatOutput>>(
                _py,
                BuiltinExc::TypeError,
                &format!("{message} (required {required}, got {got})"),
            );
        }
        return raise_exception::<Option<FormatOutput>>(_py, BuiltinExc::TypeError, message);
    }
    Some(out.finish())
}

/// 3.15 refuses '*' once a field has used a mapping key.
fn percent_star_admitted(
    py: &PyToken<'_>,
    arguments: &PercentArguments<'_, '_>,
    text: &[u8],
    field_start: usize,
) -> Option<()> {
    if arguments.keyed && percent_modern(py) {
        let position = percent_codepoint_index(text, field_start);
        return raise_exception(
            py,
            BuiltinExc::ValueError,
            &format!("* cannot be used with a parenthesised mapping key at position {position}"),
        );
    }
    Some(())
}
