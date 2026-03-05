use crate::arena::TempArena;
use crate::*;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Cursor;

// --- JSON ---

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_json_parse_int(ptr: *const u8, len_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            let s = {
                let slice = std::slice::from_raw_parts(ptr, len);
                std::str::from_utf8(slice).unwrap()
            };
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            v.as_i64().unwrap_or(0)
        })
    }
}

fn json_escape_codepoint(code: u32, out: &mut String) {
    if code <= 0xFFFF {
        let _ = write!(out, "\\u{code:04x}");
        return;
    }
    let adjusted = code - 0x10000;
    let high = 0xD800 + ((adjusted >> 10) & 0x3FF);
    let low = 0xDC00 + (adjusted & 0x3FF);
    let _ = write!(out, "\\u{high:04x}\\u{low:04x}");
}

fn json_encode_basestring_impl(value: &str, ensure_ascii: bool) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len().saturating_add(8));
    out.push('"');

    if !ensure_ascii {
        // SIMD fast path: scan for safe ASCII runs and copy them in bulk.
        // A byte is "safe" if it's in [0x20..0x7F] and not '"' (0x22) or '\\' (0x5C).
        let mut i = 0usize;

        #[cfg(target_arch = "aarch64")]
        {
            unsafe {
                use std::arch::aarch64::*;
                let lo_bound = vdupq_n_u8(0x20); // space
                let hi_bound = vdupq_n_u8(0x7E); // '~'
                let quote = vdupq_n_u8(b'"');
                let backslash = vdupq_n_u8(b'\\');
                while i + 16 <= bytes.len() {
                    let chunk = vld1q_u8(bytes.as_ptr().add(i));
                    let ge_lo = vcgeq_u8(chunk, lo_bound);
                    let le_hi = vcleq_u8(chunk, hi_bound);
                    let not_quote = vmvnq_u8(vceqq_u8(chunk, quote));
                    let not_bs = vmvnq_u8(vceqq_u8(chunk, backslash));
                    let safe = vandq_u8(vandq_u8(ge_lo, le_hi), vandq_u8(not_quote, not_bs));
                    if vminvq_u8(safe) == 0xFF {
                        // All 16 bytes are safe — copy in bulk
                        out.push_str(std::str::from_utf8_unchecked(&bytes[i..i + 16]));
                        i += 16;
                        continue;
                    }
                    break;
                }
            }
        }

        #[cfg(target_arch = "x86_64")]
        {
            unsafe {
                use std::arch::x86_64::*;
                let lo_bound = _mm_set1_epi8(0x1F); // below space
                let hi_bound = _mm_set1_epi8(0x7F_u8 as i8); // DEL
                let quote = _mm_set1_epi8(b'"' as i8);
                let backslash = _mm_set1_epi8(b'\\' as i8);
                while i + 16 <= bytes.len() {
                    let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    // Safe: > 0x1F && < 0x7F && != '"' && != '\\'
                    let gt_lo = _mm_cmpgt_epi8(chunk, lo_bound);
                    let lt_hi = _mm_cmpgt_epi8(hi_bound, chunk);
                    let not_quote =
                        _mm_andnot_si128(_mm_cmpeq_epi8(chunk, quote), _mm_set1_epi8(-1));
                    let not_bs =
                        _mm_andnot_si128(_mm_cmpeq_epi8(chunk, backslash), _mm_set1_epi8(-1));
                    let safe = _mm_and_si128(
                        _mm_and_si128(gt_lo, lt_hi),
                        _mm_and_si128(not_quote, not_bs),
                    );
                    if _mm_movemask_epi8(safe) == 0xFFFF {
                        out.push_str(std::str::from_utf8_unchecked(&bytes[i..i + 16]));
                        i += 16;
                        continue;
                    }
                    break;
                }
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            if cfg!(target_feature = "simd128") {
                unsafe {
                    use std::arch::wasm32::*;
                    let lo_bound = u8x16_splat(0x20); // space
                    let hi_bound = u8x16_splat(0x7E); // '~'
                    let quote = u8x16_splat(b'"');
                    let backslash = u8x16_splat(b'\\');
                    while i + 16 <= bytes.len() {
                        let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                        let ge_lo = u8x16_ge(chunk, lo_bound);
                        let le_hi = u8x16_le(chunk, hi_bound);
                        let not_quote = v128_not(u8x16_eq(chunk, quote));
                        let not_bs = v128_not(u8x16_eq(chunk, backslash));
                        let safe = v128_and(v128_and(ge_lo, le_hi), v128_and(not_quote, not_bs));
                        if u8x16_bitmask(safe) == 0xFFFF {
                            out.push_str(std::str::from_utf8_unchecked(&bytes[i..i + 16]));
                            i += 16;
                            continue;
                        }
                        break;
                    }
                }
            }
        }

        // Process remaining characters (including where SIMD found a special char)
        for ch in value[i..].chars() {
            let code = ch as u32;
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{08}' => out.push_str("\\b"),
                '\u{0C}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => {
                    if code < 0x20 {
                        json_escape_codepoint(code, &mut out);
                    } else {
                        out.push(ch);
                    }
                }
            }
        }
    } else {
        // ensure_ascii mode — SIMD scan for safe ASCII runs, escape everything else.
        // Safe: 0x20 <= byte <= 0x7E && byte != '"' && byte != '\\'
        let mut i = 0usize;

        #[cfg(target_arch = "aarch64")]
        {
            unsafe {
                use std::arch::aarch64::*;
                let lo_bound = vdupq_n_u8(0x20);
                let hi_bound = vdupq_n_u8(0x7E);
                let quote = vdupq_n_u8(b'"');
                let backslash = vdupq_n_u8(b'\\');
                while i + 16 <= bytes.len() {
                    let chunk = vld1q_u8(bytes.as_ptr().add(i));
                    let ge_lo = vcgeq_u8(chunk, lo_bound);
                    let le_hi = vcleq_u8(chunk, hi_bound);
                    let not_quote = vmvnq_u8(vceqq_u8(chunk, quote));
                    let not_bs = vmvnq_u8(vceqq_u8(chunk, backslash));
                    let safe = vandq_u8(vandq_u8(ge_lo, le_hi), vandq_u8(not_quote, not_bs));
                    if vminvq_u8(safe) == 0xFF {
                        out.push_str(std::str::from_utf8_unchecked(&bytes[i..i + 16]));
                        i += 16;
                        continue;
                    }
                    break;
                }
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            unsafe {
                use std::arch::x86_64::*;
                let lo_bound = _mm_set1_epi8(0x1F);
                let hi_bound = _mm_set1_epi8(0x7F_u8 as i8);
                let quote = _mm_set1_epi8(b'"' as i8);
                let backslash = _mm_set1_epi8(b'\\' as i8);
                while i + 16 <= bytes.len() {
                    let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    let gt_lo = _mm_cmpgt_epi8(chunk, lo_bound);
                    let lt_hi = _mm_cmpgt_epi8(hi_bound, chunk);
                    let not_quote =
                        _mm_andnot_si128(_mm_cmpeq_epi8(chunk, quote), _mm_set1_epi8(-1));
                    let not_bs =
                        _mm_andnot_si128(_mm_cmpeq_epi8(chunk, backslash), _mm_set1_epi8(-1));
                    let safe = _mm_and_si128(
                        _mm_and_si128(gt_lo, lt_hi),
                        _mm_and_si128(not_quote, not_bs),
                    );
                    if _mm_movemask_epi8(safe) == 0xFFFF {
                        out.push_str(std::str::from_utf8_unchecked(&bytes[i..i + 16]));
                        i += 16;
                        continue;
                    }
                    break;
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            if cfg!(target_feature = "simd128") {
                unsafe {
                    use std::arch::wasm32::*;
                    let lo_bound = u8x16_splat(0x20);
                    let hi_bound = u8x16_splat(0x7E);
                    let quote = u8x16_splat(b'"');
                    let backslash = u8x16_splat(b'\\');
                    while i + 16 <= bytes.len() {
                        let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                        let ge_lo = u8x16_ge(chunk, lo_bound);
                        let le_hi = u8x16_le(chunk, hi_bound);
                        let not_quote = v128_not(u8x16_eq(chunk, quote));
                        let not_bs = v128_not(u8x16_eq(chunk, backslash));
                        let safe = v128_and(v128_and(ge_lo, le_hi), v128_and(not_quote, not_bs));
                        if u8x16_bitmask(safe) == 0xFFFF {
                            out.push_str(std::str::from_utf8_unchecked(&bytes[i..i + 16]));
                            i += 16;
                            continue;
                        }
                        break;
                    }
                }
            }
        }

        for ch in value[i..].chars() {
            let code = ch as u32;
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{08}' => out.push_str("\\b"),
                '\u{0C}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => {
                    if code < 0x20 || code > 0x7E {
                        json_escape_codepoint(code, &mut out);
                    } else {
                        out.push(ch);
                    }
                }
            }
        }
    }
    out.push('"');
    out
}

fn json_string_line_col(text: &str, pos: usize) -> (usize, usize) {
    let mut lineno = 1usize;
    let mut last_newline: Option<usize> = None;
    for (idx, ch) in text.chars().enumerate() {
        if idx >= pos {
            break;
        }
        if ch == '\n' {
            lineno += 1;
            last_newline = Some(idx);
        }
    }
    let colno = match last_newline {
        Some(idx) => pos.saturating_sub(idx),
        None => pos.saturating_add(1),
    };
    (lineno, colno)
}

fn json_scanstring_decode(
    text: &str,
    end: usize,
    strict: bool,
) -> Result<(String, usize), (String, usize)> {
    let bytes = text.as_bytes();
    // Use byte offsets for the SIMD fast path, converting to char offsets for escape handling
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if end > len {
        return Err(("end is out of bounds".to_string(), end));
    }
    let mut idx = end;
    let mut out = String::new();

    // SIMD fast path: scan for safe ASCII bytes (not '"', not '\\', not control chars)
    // and bulk-copy them. This is the common case for JSON string values.
    if idx < len {
        // Convert char offset to byte offset for SIMD scanning
        let byte_start: usize = chars[..idx].iter().map(|c| c.len_utf8()).sum();
        let mut bi = byte_start;

        #[cfg(target_arch = "aarch64")]
        {
            unsafe {
                use std::arch::aarch64::*;
                let lo_bound = vdupq_n_u8(0x20);
                let hi_bound = vdupq_n_u8(0x7E);
                let quote = vdupq_n_u8(b'"');
                let backslash = vdupq_n_u8(b'\\');
                while bi + 16 <= bytes.len() {
                    let chunk = vld1q_u8(bytes.as_ptr().add(bi));
                    let ge_lo = vcgeq_u8(chunk, lo_bound);
                    let le_hi = vcleq_u8(chunk, hi_bound);
                    let not_quote = vmvnq_u8(vceqq_u8(chunk, quote));
                    let not_bs = vmvnq_u8(vceqq_u8(chunk, backslash));
                    let safe = vandq_u8(vandq_u8(ge_lo, le_hi), vandq_u8(not_quote, not_bs));
                    if vminvq_u8(safe) == 0xFF {
                        out.push_str(std::str::from_utf8_unchecked(&bytes[bi..bi + 16]));
                        bi += 16;
                        idx += 16; // All safe ASCII, so 1 byte = 1 char
                        continue;
                    }
                    break;
                }
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            unsafe {
                use std::arch::x86_64::*;
                let lo_bound = _mm_set1_epi8(0x1F);
                let hi_bound = _mm_set1_epi8(0x7F_u8 as i8);
                let quote = _mm_set1_epi8(b'"' as i8);
                let backslash = _mm_set1_epi8(b'\\' as i8);
                while bi + 16 <= bytes.len() {
                    let chunk = _mm_loadu_si128(bytes.as_ptr().add(bi) as *const __m128i);
                    let gt_lo = _mm_cmpgt_epi8(chunk, lo_bound);
                    let lt_hi = _mm_cmpgt_epi8(hi_bound, chunk);
                    let not_quote =
                        _mm_andnot_si128(_mm_cmpeq_epi8(chunk, quote), _mm_set1_epi8(-1));
                    let not_bs =
                        _mm_andnot_si128(_mm_cmpeq_epi8(chunk, backslash), _mm_set1_epi8(-1));
                    let safe = _mm_and_si128(
                        _mm_and_si128(gt_lo, lt_hi),
                        _mm_and_si128(not_quote, not_bs),
                    );
                    if _mm_movemask_epi8(safe) == 0xFFFF {
                        out.push_str(std::str::from_utf8_unchecked(&bytes[bi..bi + 16]));
                        bi += 16;
                        idx += 16;
                        continue;
                    }
                    break;
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            if cfg!(target_feature = "simd128") {
                unsafe {
                    use std::arch::wasm32::*;
                    let lo_bound = u8x16_splat(0x20);
                    let hi_bound = u8x16_splat(0x7E);
                    let quote = u8x16_splat(b'"');
                    let backslash = u8x16_splat(b'\\');
                    while bi + 16 <= bytes.len() {
                        let chunk = v128_load(bytes.as_ptr().add(bi) as *const v128);
                        let ge_lo = u8x16_ge(chunk, lo_bound);
                        let le_hi = u8x16_le(chunk, hi_bound);
                        let not_quote = v128_not(u8x16_eq(chunk, quote));
                        let not_bs = v128_not(u8x16_eq(chunk, backslash));
                        let safe = v128_and(v128_and(ge_lo, le_hi), v128_and(not_quote, not_bs));
                        if u8x16_bitmask(safe) == 0xFFFF {
                            out.push_str(std::str::from_utf8_unchecked(&bytes[bi..bi + 16]));
                            bi += 16;
                            idx += 16;
                            continue;
                        }
                        break;
                    }
                }
            }
        }
    }

    // Continue with scalar char-by-char processing from where SIMD left off
    while idx < len {
        let ch = chars[idx];
        if ch == '"' {
            return Ok((out, idx + 1));
        }
        if ch == '\\' {
            idx += 1;
            if idx >= len {
                let start = end.saturating_sub(1);
                return Err(("Unterminated string starting at".to_string(), start));
            }
            let esc = chars[idx];
            match esc {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{08}'),
                'f' => out.push('\u{0C}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex_start = idx + 1;
                    if hex_start + 4 > len {
                        return Err(("Invalid \\uXXXX escape".to_string(), idx));
                    }
                    let mut code: u32 = 0;
                    for c in &chars[hex_start..hex_start + 4] {
                        let Some(digit) = c.to_digit(16) else {
                            return Err(("Invalid \\uXXXX escape".to_string(), idx));
                        };
                        code = (code << 4) | digit;
                    }
                    idx += 4;
                    if (0xD800..=0xDBFF).contains(&code)
                        && idx + 6 <= len
                        && chars[idx + 1] == '\\'
                        && chars[idx + 2] == 'u'
                    {
                        let mut low: u32 = 0;
                        let mut valid = true;
                        for c in &chars[idx + 3..idx + 7] {
                            if let Some(d) = c.to_digit(16) {
                                low = (low << 4) | d;
                            } else {
                                valid = false;
                                break;
                            }
                        }
                        if valid && (0xDC00..=0xDFFF).contains(&low) {
                            let combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                            if let Some(real) = char::from_u32(combined) {
                                out.push(real);
                                idx += 6;
                                idx += 1;
                                continue;
                            }
                        }
                    }
                    if let Some(real) = char::from_u32(code) {
                        out.push(real);
                    } else {
                        return Err(("Invalid \\uXXXX escape".to_string(), idx));
                    }
                }
                _ => return Err(("Invalid \\escape".to_string(), idx.saturating_sub(1))),
            }
            idx += 1;
            continue;
        }
        if strict && (ch as u32) < 0x20 {
            return Err(("Invalid control character at".to_string(), idx));
        }
        out.push(ch);
        idx += 1;
    }
    let start = end.saturating_sub(1);
    Err(("Unterminated string starting at".to_string(), start))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_encode_basestring_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = string_obj_to_owned(obj_from_bits(obj_bits)) else {
            let type_name = type_name(_py, obj_from_bits(obj_bits));
            let msg = format!("first argument must be a string, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let encoded = json_encode_basestring_impl(value.as_str(), false);
        let ptr = alloc_string(_py, encoded.as_bytes());
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_encode_basestring_ascii_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = string_obj_to_owned(obj_from_bits(obj_bits)) else {
            let type_name = type_name(_py, obj_from_bits(obj_bits));
            let msg = format!("first argument must be a string, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let encoded = json_encode_basestring_impl(value.as_str(), true);
        let ptr = alloc_string(_py, encoded.as_bytes());
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_scanstring_obj(text_bits: u64, end_bits: u64, strict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            let type_name = type_name(_py, obj_from_bits(text_bits));
            let msg = format!("first argument must be a string, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let Some(end_i64) = to_i64(obj_from_bits(end_bits)) else {
            let type_name = type_name(_py, obj_from_bits(end_bits));
            let msg = format!("'{type_name}' object cannot be interpreted as an integer");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if end_i64 < 0 {
            return raise_exception::<_>(_py, "ValueError", "end is out of bounds");
        }
        let strict = is_truthy(_py, obj_from_bits(strict_bits));
        let end = end_i64 as usize;
        match json_scanstring_decode(text.as_str(), end, strict) {
            Ok((decoded, idx)) => {
                let decoded_ptr = alloc_string(_py, decoded.as_bytes());
                if decoded_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
                }
                let tuple_ptr = alloc_tuple(
                    _py,
                    &[
                        MoltObject::from_ptr(decoded_ptr).bits(),
                        MoltObject::from_int(idx as i64).bits(),
                    ],
                );
                if tuple_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate tuple");
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
            Err((msg, pos)) => {
                if msg == "end is out of bounds" {
                    return raise_exception::<_>(_py, "ValueError", msg.as_str());
                }
                let (lineno, colno) = json_string_line_col(text.as_str(), pos);
                let detail = format!("{msg}: line {lineno} column {colno} (char {pos})");
                raise_exception::<_>(_py, "ValueError", detail.as_str())
            }
        }
    })
}

fn value_to_object(
    _py: &PyToken<'_>,
    value: serde_json::Value,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    match value {
        serde_json::Value::Null => Ok(MoltObject::none()),
        serde_json::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(MoltObject::from_int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(MoltObject::from_float(f))
            } else {
                Err(2)
            }
        }
        serde_json::Value::String(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_json::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = value_to_object(_py, item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(_py, elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_json::Value::Object(map) => {
            if map.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if map.is_empty() {
                let ptr = alloc_dict_with_pairs(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = map.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in map.into_iter().enumerate() {
                let key_ptr = alloc_string(_py, key.as_bytes());
                if key_ptr.is_null() {
                    return Err(2);
                }
                let val_obj = value_to_object(_py, value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = MoltObject::from_ptr(key_ptr).bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(_py, pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
    }
}

fn msgpack_value_to_object(
    _py: &PyToken<'_>,
    value: rmpv::Value,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    match value {
        rmpv::Value::Nil => Ok(MoltObject::none()),
        rmpv::Value::Boolean(b) => Ok(MoltObject::from_bool(b)),
        rmpv::Value::Integer(i) => {
            if let Some(v) = i.as_i64() {
                Ok(MoltObject::from_int(v))
            } else if let Some(v) = i.as_u64() {
                if v <= i64::MAX as u64 {
                    Ok(MoltObject::from_int(v as i64))
                } else {
                    Err(2)
                }
            } else {
                Err(2)
            }
        }
        rmpv::Value::F32(f) => Ok(MoltObject::from_float(f as f64)),
        rmpv::Value::F64(f) => Ok(MoltObject::from_float(f)),
        rmpv::Value::String(s) => {
            let s = s.as_str().ok_or(2)?;
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Binary(b) => {
            let ptr = alloc_bytes(_py, &b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = msgpack_value_to_object(_py, item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(_py, elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Map(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_dict_with_pairs(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in items.into_iter().enumerate() {
                let key_obj = msgpack_key_to_object(_py, key)?;
                let val_obj = msgpack_value_to_object(_py, value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = key_obj.bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(_py, pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn msgpack_key_to_object(_py: &PyToken<'_>, value: rmpv::Value) -> Result<MoltObject, i32> {
    match value {
        rmpv::Value::Nil => Ok(MoltObject::none()),
        rmpv::Value::Boolean(b) => Ok(MoltObject::from_bool(b)),
        rmpv::Value::Integer(i) => {
            if let Some(v) = i.as_i64() {
                Ok(MoltObject::from_int(v))
            } else if let Some(v) = i.as_u64() {
                if v <= i64::MAX as u64 {
                    Ok(MoltObject::from_int(v as i64))
                } else {
                    Err(2)
                }
            } else {
                Err(2)
            }
        }
        rmpv::Value::String(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Binary(b) => {
            let ptr = alloc_bytes(_py, &b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn cbor_value_to_object(
    _py: &PyToken<'_>,
    value: ciborium::Value,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    match value {
        ciborium::Value::Null => Ok(MoltObject::none()),
        ciborium::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        ciborium::Value::Integer(i) => {
            let i_val: i128 = i.into();
            if i_val < i64::MIN as i128 || i_val > i64::MAX as i128 {
                return Err(2);
            }
            Ok(MoltObject::from_int(i_val as i64))
        }
        ciborium::Value::Float(f) => Ok(MoltObject::from_float(f)),
        ciborium::Value::Text(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        ciborium::Value::Bytes(b) => {
            let ptr = alloc_bytes(_py, &b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        ciborium::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = cbor_value_to_object(_py, item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(_py, elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        ciborium::Value::Map(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_dict_with_pairs(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in items.into_iter().enumerate() {
                let key_obj = cbor_key_to_object(_py, key)?;
                let val_obj = cbor_value_to_object(_py, value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = key_obj.bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(_py, pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn cbor_key_to_object(_py: &PyToken<'_>, value: ciborium::Value) -> Result<MoltObject, i32> {
    match value {
        ciborium::Value::Null => Ok(MoltObject::none()),
        ciborium::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        ciborium::Value::Integer(i) => {
            let i_val: i128 = i.into();
            if i_val < i64::MIN as i128 || i_val > i64::MAX as i128 {
                Err(2)
            } else {
                Ok(MoltObject::from_int(i_val as i64))
            }
        }
        ciborium::Value::Text(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        ciborium::Value::Bytes(b) => {
            let ptr = alloc_bytes(_py, &b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn parse_cbor_value(slice: &[u8]) -> Result<ciborium::Value, ()> {
    let mut cursor = Cursor::new(slice);
    let value: ciborium::Value = ciborium::from_reader(&mut cursor).map_err(|_| ())?;
    if cursor.position() as usize != slice.len() {
        return Err(());
    }
    Ok(value)
}

unsafe fn parse_json_scalar(
    _py: &PyToken<'_>,
    ptr: *const u8,
    len: usize,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    unsafe {
        let slice = std::slice::from_raw_parts(ptr, len);
        let s = std::str::from_utf8(slice).map_err(|_| 1)?;
        let v: serde_json::Value = serde_json::from_str(s).map_err(|_| 1)?;
        value_to_object(_py, v, arena)
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_json_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            if out.is_null() {
                return 2;
            }
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = parse_json_scalar(_py, ptr, len, &mut arena);
                arena.reset();
                result
            });
            let obj = match obj {
                Ok(val) => val,
                Err(code) => return code,
            };
            *out = obj.bits();
            0
        })
    }
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_msgpack_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            if out.is_null() {
                return 2;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let mut cursor = Cursor::new(slice);
            let v = match rmpv::decode::read_value(&mut cursor) {
                Ok(val) => val,
                Err(_) => return 1,
            };
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = msgpack_value_to_object(_py, v, &mut arena);
                arena.reset();
                result
            });
            let obj = match obj {
                Ok(val) => val,
                Err(code) => return code,
            };
            *out = obj.bits();
            0
        })
    }
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cbor_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            if out.is_null() {
                return 2;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let v: ciborium::Value = match parse_cbor_value(slice) {
                Ok(val) => val,
                Err(_) => return 1,
            };
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = cbor_value_to_object(_py, v, &mut arena);
                arena.reset();
                result
            });
            let obj = match obj {
                Ok(val) => val,
                Err(code) => return code,
            };
            *out = obj.bits();
            0
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_parse_scalar_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "json.parse expects str");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let msg = format!("json.parse expects str, got {}", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let len = string_len(ptr);
            let data = string_bytes(ptr);
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = parse_json_scalar(_py, data, len, &mut arena);
                arena.reset();
                result
            });
            match obj {
                Ok(val) => val.bits(),
                Err(_) => raise_exception::<_>(_py, "ValueError", "invalid JSON payload"),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_msgpack_parse_scalar_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "msgpack.parse expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("msgpack.parse expects bytes, got {}", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            let mut cursor = Cursor::new(slice);
            let v = match rmpv::decode::read_value(&mut cursor) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<u64>(_py, "ValueError", "invalid msgpack payload");
                }
            };
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = msgpack_value_to_object(_py, v, &mut arena);
                arena.reset();
                result
            });
            match obj {
                Ok(val) => val.bits(),
                Err(_) => raise_exception::<u64>(_py, "ValueError", "invalid msgpack payload"),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cbor_parse_scalar_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "cbor.parse expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("cbor.parse expects bytes, got {}", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            let v: ciborium::Value = match parse_cbor_value(slice) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<u64>(_py, "ValueError", "invalid cbor payload");
                }
            };
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = cbor_value_to_object(_py, v, &mut arena);
                arena.reset();
                result
            });
            match obj {
                Ok(val) => val.bits(),
                Err(_) => raise_exception::<u64>(_py, "ValueError", "invalid cbor payload"),
            }
        }
    })
}

// ---------------------------------------------------------------------------
// JSON detect_encoding / loads / dumps
// ---------------------------------------------------------------------------

/// Detect the encoding of a JSON byte string by inspecting the BOM or the
/// first few bytes. Returns a MoltObject string.
#[unsafe(no_mangle)]
pub extern "C" fn molt_json_detect_encoding(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(data_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "detect_encoding expects bytes");
        };
        let (data, len) = unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("detect_encoding expects bytes, got {}", type_name(_py, obj));
                return raise_exception::<u64>(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data_ptr = bytes_data(ptr);
            (std::slice::from_raw_parts(data_ptr, len), len)
        };

        let encoding = detect_json_encoding(data, len);
        let enc_ptr = alloc_string(_py, encoding.as_bytes());
        if enc_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(enc_ptr).bits()
    })
}

fn detect_json_encoding(data: &[u8], len: usize) -> &'static str {
    // Check BOM first
    if len >= 4 {
        match (data[0], data[1], data[2], data[3]) {
            (0x00, 0x00, 0xFE, 0xFF) => return "utf-32",
            (0xFF, 0xFE, 0x00, 0x00) => return "utf-32",
            _ => {}
        }
    }
    if len >= 2 {
        match (data[0], data[1]) {
            (0xFE, 0xFF) => return "utf-16",
            (0xFF, 0xFE) => return "utf-16",
            _ => {}
        }
    }
    if len >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
        return "utf-8-sig";
    }

    // No BOM — use the null-byte pattern of the first 2–4 bytes (RFC 4627 §3)
    if len >= 4 {
        match (data[0], data[1], data[2], data[3]) {
            (0x00, 0x00, 0x00, _) => return "utf-32-be",
            (_, 0x00, 0x00, 0x00) => return "utf-32-le",
            (0x00, _, 0x00, _) => return "utf-16-be",
            (_, 0x00, _, 0x00) => return "utf-16-le",
            _ => {}
        }
    } else if len >= 2 {
        match (data[0], data[1]) {
            (0x00, _) => return "utf-16-be",
            (_, 0x00) => return "utf-16-le",
            _ => {}
        }
    }
    "utf-8"
}

fn decode_json_text(_py: &PyToken<'_>, obj: MoltObject, data: &[u8]) -> Result<String, u64> {
    let encoding = detect_json_encoding(data, data.len());
    let decoded = match crate::object::ops::decode_bytes_text(encoding, "strict", data) {
        Ok((text_bytes, _)) => text_bytes,
        Err(crate::object::ops::DecodeTextError::Failure(failure, codec)) => match failure {
            DecodeFailure::Byte { pos, message, .. } => {
                return Err(raise_unicode_decode_error::<u64>(
                    _py,
                    &codec,
                    obj.bits(),
                    pos,
                    pos + 1,
                    message,
                ));
            }
            DecodeFailure::Range {
                start,
                end,
                message,
            } => {
                return Err(raise_unicode_decode_error::<u64>(
                    _py,
                    &codec,
                    obj.bits(),
                    start,
                    end,
                    message,
                ));
            }
            DecodeFailure::UnknownErrorHandler(handler) => {
                let msg = format!("unknown error handler name '{handler}'");
                return Err(raise_exception::<u64>(_py, "LookupError", &msg));
            }
        },
        Err(crate::object::ops::DecodeTextError::UnknownEncoding(codec)) => {
            let msg = format!("unknown encoding: {codec}");
            return Err(raise_exception::<u64>(_py, "LookupError", &msg));
        }
        Err(crate::object::ops::DecodeTextError::UnknownErrorHandler(handler)) => {
            let msg = format!("unknown error handler name '{handler}'");
            return Err(raise_exception::<u64>(_py, "LookupError", &msg));
        }
    };
    match String::from_utf8(decoded) {
        Ok(text) => Ok(text),
        Err(_) => Err(raise_exception::<u64>(
            _py,
            "UnicodeDecodeError",
            "decoded JSON payload was not valid UTF-8",
        )),
    }
}

/// Full JSON loads: parse a JSON string and return the MoltObject tree.
#[unsafe(no_mangle)]
pub extern "C" fn molt_json_loads(text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(text_bits);

        // Accept str
        if let Some(text) = string_obj_to_owned(obj) {
            return json_loads_str(_py, &text);
        }

        // Accept bytes / bytearray with RFC/CPython-style JSON encoding detection.
        if let Some(ptr) = obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let slice = unsafe {
                    let len = bytes_len(ptr);
                    let data_ptr = bytes_data(ptr);
                    std::slice::from_raw_parts(data_ptr, len)
                };
                let text = match decode_json_text(_py, obj, slice) {
                    Ok(text) => text,
                    Err(bits) => return bits,
                };
                return json_loads_str(_py, &text);
            }
        }

        let tn = type_name(_py, obj);
        let msg = format!("the JSON object must be str, bytes or bytearray, not {tn}");
        raise_exception::<u64>(_py, "TypeError", &msg)
    })
}

fn json_loads_str(_py: &PyToken<'_>, text: &str) -> u64 {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(val) => val,
        Err(e) => {
            let msg = format!("{e}");
            return raise_exception::<u64>(_py, "ValueError", &msg);
        }
    };
    PARSE_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let result = value_to_object(_py, v, &mut arena);
        arena.reset();
        match result {
            Ok(val) => val.bits(),
            Err(_) => raise_exception::<u64>(_py, "ValueError", "failed to convert JSON value"),
        }
    })
}

#[derive(Clone, Copy)]
struct JsonLoadsOptions {
    parse_float: Option<u64>,
    parse_int: Option<u64>,
    parse_constant: Option<u64>,
    object_hook: Option<u64>,
    object_pairs_hook: Option<u64>,
    strict: bool,
}

struct JsonDumpsOptions {
    skipkeys: bool,
    ensure_ascii: bool,
    check_circular: bool,
    allow_nan: bool,
    sort_keys: bool,
    indent_text: Option<String>,
    item_separator: String,
    key_separator: String,
    default_fn: Option<u64>,
}

enum JsonParseError {
    Raised(u64),
    Message { msg: String, pos: usize },
}

enum JsonEncodeError {
    Raised(u64),
    Type(String),
    Value(String),
}

fn bits_to_optional_callable(bits: u64) -> Option<u64> {
    if obj_from_bits(bits).is_none() {
        None
    } else {
        Some(bits)
    }
}

fn release_bits(_py: &PyToken<'_>, bits: &[u64]) {
    for bit in bits {
        dec_ref_bits(_py, *bit);
    }
}

fn parse_json_indent_text(_py: &PyToken<'_>, indent_bits: u64) -> Result<Option<String>, u64> {
    let indent_obj = obj_from_bits(indent_bits);
    if indent_obj.is_none() {
        return Ok(None);
    }
    if let Some(value) = to_i64(indent_obj) {
        let n = if value < 0 { 0 } else { value as usize };
        return Ok(Some(" ".repeat(n)));
    }
    if let Some(text) = string_obj_to_owned(indent_obj) {
        return Ok(Some(text));
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "indent must be None, an integer, or a string",
    ))
}

fn parse_json_separator(_py: &PyToken<'_>, bits: u64, name: &str) -> Result<String, u64> {
    let Some(text) = string_obj_to_owned(obj_from_bits(bits)) else {
        let msg = format!("{name} must be a string");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    };
    Ok(text)
}

fn json_write_indent(out: &mut String, indent_text: Option<&str>, depth: usize) {
    if let Some(indent) = indent_text {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(indent);
        }
    }
}

fn json_float_token(value: f64, allow_nan: bool) -> Result<String, JsonEncodeError> {
    if value.is_finite() {
        let s = format!("{value}");
        if !s.contains('.') && !s.contains('e') && !s.contains('E') {
            return Ok(format!("{s}.0"));
        }
        return Ok(s);
    }
    if !allow_nan {
        return Err(JsonEncodeError::Value(
            "Out of range float values are not JSON compliant".to_string(),
        ));
    }
    if value.is_nan() {
        Ok("NaN".to_string())
    } else if value.is_sign_positive() {
        Ok("Infinity".to_string())
    } else {
        Ok("-Infinity".to_string())
    }
}

fn coerce_dict_key_to_text(
    _py: &PyToken<'_>,
    key: MoltObject,
    allow_nan: bool,
) -> Result<Option<String>, JsonEncodeError> {
    if let Some(s) = string_obj_to_owned(key) {
        return Ok(Some(s));
    }
    if key.is_none() {
        return Ok(Some("null".to_string()));
    }
    if let Some(b) = key.as_bool() {
        return Ok(Some(if b { "true" } else { "false" }.to_string()));
    }
    if let Some(i) = key.as_int() {
        return Ok(Some(format!("{i}")));
    }
    if let Some(f) = key.as_float() {
        return Ok(Some(json_float_token(f, allow_nan)?));
    }
    Ok(None)
}

fn object_to_json_with_options(
    _py: &PyToken<'_>,
    obj: MoltObject,
    out: &mut String,
    options: &JsonDumpsOptions,
    depth: usize,
    stack: &mut Vec<usize>,
    stack_set: &mut HashSet<usize>,
) -> Result<(), JsonEncodeError> {
    if obj.is_none() {
        out.push_str("null");
        return Ok(());
    }
    if let Some(value) = obj.as_bool() {
        out.push_str(if value { "true" } else { "false" });
        return Ok(());
    }
    if obj.is_int()
        && let Some(value) = obj.as_int()
    {
        let _ = std::fmt::Write::write_fmt(out, format_args!("{value}"));
        return Ok(());
    }
    if let Some(value) = obj.as_float() {
        out.push_str(&json_float_token(value, options.allow_nan)?);
        return Ok(());
    }

    let Some(ptr) = obj.as_ptr() else {
        let tn = type_name(_py, obj);
        if let Some(default_fn) = options.default_fn {
            let out_bits = unsafe { call_callable1(_py, default_fn, obj.bits()) };
            if exception_pending(_py) {
                return Err(JsonEncodeError::Raised(out_bits));
            }
            return object_to_json_with_options(
                _py,
                obj_from_bits(out_bits),
                out,
                options,
                depth,
                stack,
                stack_set,
            );
        }
        return Err(JsonEncodeError::Type(format!(
            "Object of type {tn} is not JSON serializable"
        )));
    };

    let type_id = unsafe { object_type_id(ptr) };
    match type_id {
        TYPE_ID_STRING => {
            let s = string_obj_to_owned(obj).unwrap_or_default();
            out.push_str(&json_encode_basestring_impl(&s, options.ensure_ascii));
            Ok(())
        }
        TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => Err(JsonEncodeError::Type(
            "Object of type bytes is not JSON serializable".to_string(),
        )),
        TYPE_ID_LIST | TYPE_ID_TUPLE => {
            let marker = ptr as usize;
            if options.check_circular {
                if !stack_set.insert(marker) {
                    return Err(JsonEncodeError::Value(
                        "Circular reference detected".to_string(),
                    ));
                }
                stack.push(marker);
            }

            let elems = unsafe { seq_vec_ref(ptr) };
            out.push('[');
            for (idx, elem_bits) in elems.iter().enumerate() {
                if idx > 0 {
                    out.push_str(&options.item_separator);
                }
                json_write_indent(out, options.indent_text.as_deref(), depth + 1);
                object_to_json_with_options(
                    _py,
                    obj_from_bits(*elem_bits),
                    out,
                    options,
                    depth + 1,
                    stack,
                    stack_set,
                )?;
            }
            if !elems.is_empty() {
                json_write_indent(out, options.indent_text.as_deref(), depth);
            }
            out.push(']');

            if options.check_circular {
                if let Some(popped) = stack.pop() {
                    stack_set.remove(&popped);
                }
            }
            Ok(())
        }
        TYPE_ID_DICT => {
            let marker = ptr as usize;
            if options.check_circular {
                if !stack_set.insert(marker) {
                    return Err(JsonEncodeError::Value(
                        "Circular reference detected".to_string(),
                    ));
                }
                stack.push(marker);
            }

            let order = unsafe { crate::builtins::containers::dict_order(ptr) };
            let mut entries: Vec<(String, u64)> = Vec::with_capacity(order.len() / 2);
            for idx in 0..(order.len() / 2) {
                let key_bits = order[idx * 2];
                let val_bits = order[idx * 2 + 1];
                let key_obj = obj_from_bits(key_bits);
                let key_text = coerce_dict_key_to_text(_py, key_obj, options.allow_nan)?;
                match key_text {
                    Some(text) => entries.push((text, val_bits)),
                    None => {
                        if options.skipkeys {
                            continue;
                        }
                        let tn = type_name(_py, key_obj);
                        return Err(JsonEncodeError::Type(format!(
                            "keys must be str, int, float, bool or None, not {tn}"
                        )));
                    }
                }
            }
            if options.sort_keys {
                entries.sort_by(|a, b| a.0.cmp(&b.0));
            }

            out.push('{');
            for (idx, (key_text, val_bits)) in entries.iter().enumerate() {
                if idx > 0 {
                    out.push_str(&options.item_separator);
                }
                json_write_indent(out, options.indent_text.as_deref(), depth + 1);
                out.push_str(&json_encode_basestring_impl(key_text, options.ensure_ascii));
                out.push_str(&options.key_separator);
                object_to_json_with_options(
                    _py,
                    obj_from_bits(*val_bits),
                    out,
                    options,
                    depth + 1,
                    stack,
                    stack_set,
                )?;
            }
            if !entries.is_empty() {
                json_write_indent(out, options.indent_text.as_deref(), depth);
            }
            out.push('}');

            if options.check_circular {
                if let Some(popped) = stack.pop() {
                    stack_set.remove(&popped);
                }
            }
            Ok(())
        }
        _ => {
            if let Some(default_fn) = options.default_fn {
                let out_bits = unsafe { call_callable1(_py, default_fn, obj.bits()) };
                if exception_pending(_py) {
                    return Err(JsonEncodeError::Raised(out_bits));
                }
                object_to_json_with_options(
                    _py,
                    obj_from_bits(out_bits),
                    out,
                    options,
                    depth,
                    stack,
                    stack_set,
                )
            } else {
                let tn = type_name(_py, obj);
                Err(JsonEncodeError::Type(format!(
                    "Object of type {tn} is not JSON serializable"
                )))
            }
        }
    }
}

struct JsonParser<'a, 'py> {
    _py: &'py PyToken<'py>,
    text: &'a str,
    chars: Vec<char>,
    length: usize,
    index: usize,
    options: JsonLoadsOptions,
}

impl<'a, 'py> JsonParser<'a, 'py> {
    fn new(_py: &'py PyToken<'py>, text: &'a str, options: JsonLoadsOptions) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let length = chars.len();
        Self {
            _py,
            text,
            chars,
            length,
            index: 0,
            options,
        }
    }

    fn parse(&mut self) -> Result<u64, JsonParseError> {
        self.consume_ws();
        let value_bits = self.parse_value()?;
        self.consume_ws();
        if self.index != self.length {
            dec_ref_bits(self._py, value_bits);
            return Err(JsonParseError::Message {
                msg: "Extra data".to_string(),
                pos: self.index,
            });
        }
        Ok(value_bits)
    }

    fn parse_raw(&mut self, idx: usize) -> Result<(u64, usize), JsonParseError> {
        self.index = idx;
        let value_bits = self.parse_value()?;
        Ok((value_bits, self.index))
    }

    fn consume_ws(&mut self) {
        while self.index < self.length {
            let ch = self.chars[self.index];
            if ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n' {
                self.index += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        if self.index < self.length {
            Some(self.chars[self.index])
        } else {
            None
        }
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek();
        if ch.is_some() {
            self.index += 1;
        }
        ch
    }

    fn starts_with(&self, text: &str) -> bool {
        let mut probe = self.index;
        for ch in text.chars() {
            if probe >= self.length || self.chars[probe] != ch {
                return false;
            }
            probe += 1;
        }
        true
    }

    fn bump_text(&mut self, text: &str) {
        self.index += text.chars().count();
    }

    fn alloc_string_bits(&self, text: &str) -> Result<u64, JsonParseError> {
        let ptr = alloc_string(self._py, text.as_bytes());
        if ptr.is_null() {
            return Err(JsonParseError::Raised(raise_exception::<u64>(
                self._py,
                "MemoryError",
                "failed to allocate string",
            )));
        }
        Ok(MoltObject::from_ptr(ptr).bits())
    }

    fn call_text_callback(&self, callback_bits: u64, text: &str) -> Result<u64, JsonParseError> {
        let arg_bits = self.alloc_string_bits(text)?;
        let out_bits = unsafe { call_callable1(self._py, callback_bits, arg_bits) };
        dec_ref_bits(self._py, arg_bits);
        if exception_pending(self._py) {
            return Err(JsonParseError::Raised(out_bits));
        }
        Ok(out_bits)
    }

    fn parse_value(&mut self) -> Result<u64, JsonParseError> {
        match self.peek() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => self.parse_string(),
            Some('t') => self.parse_literal("true", MoltObject::from_bool(true).bits()),
            Some('f') => self.parse_literal("false", MoltObject::from_bool(false).bits()),
            Some('n') => self.parse_literal("null", MoltObject::none().bits()),
            Some('N') => self.parse_constant("NaN"),
            Some('I') => self.parse_constant("Infinity"),
            Some('-') if self.starts_with("-Infinity") => self.parse_constant("-Infinity"),
            Some('-') => self.parse_number(),
            Some(ch) if ch.is_ascii_digit() => self.parse_number(),
            _ => Err(JsonParseError::Message {
                msg: "Expecting value".to_string(),
                pos: self.index,
            }),
        }
    }

    fn parse_literal(&mut self, text: &str, value_bits: u64) -> Result<u64, JsonParseError> {
        if !self.starts_with(text) {
            return Err(JsonParseError::Message {
                msg: "Expecting value".to_string(),
                pos: self.index,
            });
        }
        self.bump_text(text);
        Ok(value_bits)
    }

    fn parse_constant(&mut self, text: &str) -> Result<u64, JsonParseError> {
        if !self.starts_with(text) {
            return Err(JsonParseError::Message {
                msg: "Expecting value".to_string(),
                pos: self.index,
            });
        }
        self.bump_text(text);
        if let Some(callback_bits) = self.options.parse_constant {
            return self.call_text_callback(callback_bits, text);
        }
        let value = match text {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => f64::NAN,
        };
        Ok(MoltObject::from_float(value).bits())
    }

    fn parse_number(&mut self) -> Result<u64, JsonParseError> {
        let start = self.index;
        if self.peek() == Some('-') {
            self.index += 1;
        }
        if self.index >= self.length {
            return Err(JsonParseError::Message {
                msg: "Expecting value".to_string(),
                pos: start,
            });
        }
        match self.peek() {
            Some('0') => self.index += 1,
            Some(ch) if ch.is_ascii_digit() => {
                while self.index < self.length && self.chars[self.index].is_ascii_digit() {
                    self.index += 1;
                }
            }
            _ => {
                return Err(JsonParseError::Message {
                    msg: "Expecting value".to_string(),
                    pos: start,
                });
            }
        }
        if self.peek() == Some('.') {
            self.index += 1;
            if self.index >= self.length || !self.chars[self.index].is_ascii_digit() {
                return Err(JsonParseError::Message {
                    msg: "Expecting value".to_string(),
                    pos: start,
                });
            }
            while self.index < self.length && self.chars[self.index].is_ascii_digit() {
                self.index += 1;
            }
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.index += 1;
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.index += 1;
            }
            if self.index >= self.length || !self.chars[self.index].is_ascii_digit() {
                return Err(JsonParseError::Message {
                    msg: "Expecting value".to_string(),
                    pos: start,
                });
            }
            while self.index < self.length && self.chars[self.index].is_ascii_digit() {
                self.index += 1;
            }
        }

        let raw: String = self.chars[start..self.index].iter().collect();
        let is_float = raw.contains('.') || raw.contains('e') || raw.contains('E');
        if is_float {
            if let Some(callback_bits) = self.options.parse_float {
                return self.call_text_callback(callback_bits, &raw);
            }
            let arg_bits = self.alloc_string_bits(&raw)?;
            let out_bits =
                unsafe { call_callable1(self._py, builtin_classes(self._py).float, arg_bits) };
            dec_ref_bits(self._py, arg_bits);
            if exception_pending(self._py) {
                return Err(JsonParseError::Raised(out_bits));
            }
            return Ok(out_bits);
        }

        if let Some(callback_bits) = self.options.parse_int {
            return self.call_text_callback(callback_bits, &raw);
        }
        let arg_bits = self.alloc_string_bits(&raw)?;
        let out_bits = unsafe { call_callable1(self._py, builtin_classes(self._py).int, arg_bits) };
        dec_ref_bits(self._py, arg_bits);
        if exception_pending(self._py) {
            return Err(JsonParseError::Raised(out_bits));
        }
        Ok(out_bits)
    }

    fn parse_string(&mut self) -> Result<u64, JsonParseError> {
        if self.advance() != Some('"') {
            return Err(JsonParseError::Message {
                msg: "Expecting value".to_string(),
                pos: self.index,
            });
        }
        match json_scanstring_decode(self.text, self.index, self.options.strict) {
            Ok((decoded, next_idx)) => {
                self.index = next_idx;
                self.alloc_string_bits(&decoded)
            }
            Err((msg, pos)) => Err(JsonParseError::Message { msg, pos }),
        }
    }

    fn parse_array(&mut self) -> Result<u64, JsonParseError> {
        if self.advance() != Some('[') {
            return Err(JsonParseError::Message {
                msg: "Expecting value".to_string(),
                pos: self.index,
            });
        }
        let mut items: Vec<u64> = Vec::new();
        self.consume_ws();
        if self.peek() == Some(']') {
            self.index += 1;
            let ptr = alloc_list(self._py, &[]);
            if ptr.is_null() {
                return Err(JsonParseError::Raised(raise_exception::<u64>(
                    self._py,
                    "MemoryError",
                    "failed to allocate list",
                )));
            }
            return Ok(MoltObject::from_ptr(ptr).bits());
        }

        loop {
            self.consume_ws();
            let item_bits = match self.parse_value() {
                Ok(bits) => bits,
                Err(err) => {
                    release_bits(self._py, &items);
                    return Err(err);
                }
            };
            items.push(item_bits);
            self.consume_ws();
            match self.peek() {
                Some(']') => {
                    self.index += 1;
                    break;
                }
                Some(',') => {
                    let comma_pos = self.index;
                    self.index += 1;
                    self.consume_ws();
                    if self.peek() == Some(']') {
                        release_bits(self._py, &items);
                        return Err(JsonParseError::Message {
                            msg: "Illegal trailing comma before end of array".to_string(),
                            pos: comma_pos,
                        });
                    }
                }
                _ => {
                    release_bits(self._py, &items);
                    return Err(JsonParseError::Message {
                        msg: "Expecting ',' delimiter".to_string(),
                        pos: self.index,
                    });
                }
            }
        }

        let ptr = alloc_list(self._py, &items);
        release_bits(self._py, &items);
        if ptr.is_null() {
            return Err(JsonParseError::Raised(raise_exception::<u64>(
                self._py,
                "MemoryError",
                "failed to allocate list",
            )));
        }
        Ok(MoltObject::from_ptr(ptr).bits())
    }

    fn finish_object(&self, pairs: Vec<(u64, u64)>) -> Result<u64, JsonParseError> {
        if let Some(hook_bits) = self.options.object_pairs_hook {
            let mut tuple_bits: Vec<u64> = Vec::with_capacity(pairs.len());
            for (key_bits, val_bits) in pairs {
                let tuple_ptr = alloc_tuple(self._py, &[key_bits, val_bits]);
                dec_ref_bits(self._py, key_bits);
                dec_ref_bits(self._py, val_bits);
                if tuple_ptr.is_null() {
                    release_bits(self._py, &tuple_bits);
                    return Err(JsonParseError::Raised(raise_exception::<u64>(
                        self._py,
                        "MemoryError",
                        "failed to allocate tuple",
                    )));
                }
                tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
            }
            let list_ptr = alloc_list(self._py, &tuple_bits);
            release_bits(self._py, &tuple_bits);
            if list_ptr.is_null() {
                return Err(JsonParseError::Raised(raise_exception::<u64>(
                    self._py,
                    "MemoryError",
                    "failed to allocate list",
                )));
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let out_bits = unsafe { call_callable1(self._py, hook_bits, list_bits) };
            dec_ref_bits(self._py, list_bits);
            if exception_pending(self._py) {
                return Err(JsonParseError::Raised(out_bits));
            }
            return Ok(out_bits);
        }

        let mut flat: Vec<u64> = Vec::with_capacity(pairs.len() * 2);
        for (key_bits, val_bits) in &pairs {
            flat.push(*key_bits);
            flat.push(*val_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(self._py, &flat);
        for (key_bits, val_bits) in pairs {
            dec_ref_bits(self._py, key_bits);
            dec_ref_bits(self._py, val_bits);
        }
        if dict_ptr.is_null() {
            return Err(JsonParseError::Raised(raise_exception::<u64>(
                self._py,
                "MemoryError",
                "failed to allocate dict",
            )));
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        if let Some(hook_bits) = self.options.object_hook {
            let out_bits = unsafe { call_callable1(self._py, hook_bits, dict_bits) };
            dec_ref_bits(self._py, dict_bits);
            if exception_pending(self._py) {
                return Err(JsonParseError::Raised(out_bits));
            }
            return Ok(out_bits);
        }
        Ok(dict_bits)
    }

    fn parse_object(&mut self) -> Result<u64, JsonParseError> {
        if self.advance() != Some('{') {
            return Err(JsonParseError::Message {
                msg: "Expecting value".to_string(),
                pos: self.index,
            });
        }
        let mut pairs: Vec<(u64, u64)> = Vec::new();
        self.consume_ws();
        if self.peek() == Some('}') {
            self.index += 1;
            return self.finish_object(pairs);
        }

        loop {
            self.consume_ws();
            if self.peek() != Some('"') {
                for (k, v) in pairs {
                    dec_ref_bits(self._py, k);
                    dec_ref_bits(self._py, v);
                }
                return Err(JsonParseError::Message {
                    msg: "Expecting property name enclosed in double quotes".to_string(),
                    pos: self.index,
                });
            }
            let key_bits = match self.parse_string() {
                Ok(bits) => bits,
                Err(err) => {
                    for (k, v) in pairs {
                        dec_ref_bits(self._py, k);
                        dec_ref_bits(self._py, v);
                    }
                    return Err(err);
                }
            };
            self.consume_ws();
            if self.peek() != Some(':') {
                dec_ref_bits(self._py, key_bits);
                for (k, v) in pairs {
                    dec_ref_bits(self._py, k);
                    dec_ref_bits(self._py, v);
                }
                return Err(JsonParseError::Message {
                    msg: "Expecting ':' delimiter".to_string(),
                    pos: self.index,
                });
            }
            self.index += 1;
            self.consume_ws();
            let val_bits = match self.parse_value() {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(self._py, key_bits);
                    for (k, v) in pairs {
                        dec_ref_bits(self._py, k);
                        dec_ref_bits(self._py, v);
                    }
                    return Err(err);
                }
            };
            pairs.push((key_bits, val_bits));
            self.consume_ws();
            match self.peek() {
                Some('}') => {
                    self.index += 1;
                    break;
                }
                Some(',') => {
                    let comma_pos = self.index;
                    self.index += 1;
                    self.consume_ws();
                    if self.peek() == Some('}') {
                        for (k, v) in pairs {
                            dec_ref_bits(self._py, k);
                            dec_ref_bits(self._py, v);
                        }
                        return Err(JsonParseError::Message {
                            msg: "Illegal trailing comma before end of object".to_string(),
                            pos: comma_pos,
                        });
                    }
                }
                _ => {
                    for (k, v) in pairs {
                        dec_ref_bits(self._py, k);
                        dec_ref_bits(self._py, v);
                    }
                    return Err(JsonParseError::Message {
                        msg: "Expecting ',' delimiter".to_string(),
                        pos: self.index,
                    });
                }
            }
        }

        self.finish_object(pairs)
    }
}

fn json_parse_error_message(text: &str, msg: &str, pos: usize) -> String {
    let (lineno, colno) = json_string_line_col(text, pos);
    format!("{msg}: line {lineno} column {colno} (char {pos})")
}

fn run_json_parse(
    _py: &PyToken<'_>,
    text: &str,
    options: JsonLoadsOptions,
) -> Result<u64, JsonParseError> {
    if text.starts_with('\u{FEFF}') {
        return Err(JsonParseError::Message {
            msg: "Unexpected UTF-8 BOM (decode using utf-8-sig)".to_string(),
            pos: 0,
        });
    }
    let mut parser = JsonParser::new(_py, text, options);
    parser.parse()
}

fn run_json_raw_decode(
    _py: &PyToken<'_>,
    text: &str,
    idx: usize,
    options: JsonLoadsOptions,
) -> Result<(u64, usize), JsonParseError> {
    let mut parser = JsonParser::new(_py, text, options);
    parser.parse_raw(idx)
}

fn raise_json_parse_error(_py: &PyToken<'_>, text: &str, err: JsonParseError) -> u64 {
    match err {
        JsonParseError::Raised(bits) => bits,
        JsonParseError::Message { msg, pos } => {
            let detail = json_parse_error_message(text, &msg, pos);
            raise_exception::<u64>(_py, "ValueError", &detail)
        }
    }
}

/// Full JSON dumps: serialize a MoltObject to a JSON string.
///
/// Arguments (all as NaN-boxed u64 bits):
///   obj_bits          — the value to serialize
///   indent_bits       — None for compact, int for number of spaces
///   sort_keys_bits    — boolean
///   ensure_ascii_bits — boolean
#[unsafe(no_mangle)]
pub extern "C" fn molt_json_dumps(
    obj_bits: u64,
    indent_bits: u64,
    sort_keys_bits: u64,
    ensure_ascii_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let indent_text = match parse_json_indent_text(_py, indent_bits) {
            Ok(text) => text,
            Err(bits) => return bits,
        };
        let options = JsonDumpsOptions {
            skipkeys: false,
            ensure_ascii: is_truthy(_py, obj_from_bits(ensure_ascii_bits)),
            check_circular: true,
            allow_nan: false,
            sort_keys: is_truthy(_py, obj_from_bits(sort_keys_bits)),
            indent_text: indent_text.clone(),
            item_separator: if indent_text.is_some() {
                ",".to_string()
            } else {
                ", ".to_string()
            },
            key_separator: if indent_text.is_some() {
                ": ".to_string()
            } else {
                ": ".to_string()
            },
            default_fn: None,
        };
        let mut out = String::with_capacity(128);
        let mut stack = Vec::new();
        let mut stack_set = HashSet::new();
        match object_to_json_with_options(
            _py,
            obj_from_bits(obj_bits),
            &mut out,
            &options,
            0,
            &mut stack,
            &mut stack_set,
        ) {
            Ok(()) => {
                let ptr = alloc_string(_py, out.as_bytes());
                if ptr.is_null() {
                    return raise_exception::<u64>(_py, "MemoryError", "failed to allocate string");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(JsonEncodeError::Raised(bits)) => bits,
            Err(JsonEncodeError::Type(msg)) => raise_exception::<u64>(_py, "TypeError", &msg),
            Err(JsonEncodeError::Value(msg)) => raise_exception::<u64>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_dumps_ex(
    obj_bits: u64,
    skipkeys_bits: u64,
    ensure_ascii_bits: u64,
    check_circular_bits: u64,
    allow_nan_bits: u64,
    sort_keys_bits: u64,
    indent_bits: u64,
    item_separator_bits: u64,
    key_separator_bits: u64,
    default_fn_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let indent_text = match parse_json_indent_text(_py, indent_bits) {
            Ok(text) => text,
            Err(bits) => return bits,
        };
        let item_separator = match parse_json_separator(_py, item_separator_bits, "item_separator")
        {
            Ok(text) => text,
            Err(bits) => return bits,
        };
        let key_separator = match parse_json_separator(_py, key_separator_bits, "key_separator") {
            Ok(text) => text,
            Err(bits) => return bits,
        };
        let options = JsonDumpsOptions {
            skipkeys: is_truthy(_py, obj_from_bits(skipkeys_bits)),
            ensure_ascii: is_truthy(_py, obj_from_bits(ensure_ascii_bits)),
            check_circular: is_truthy(_py, obj_from_bits(check_circular_bits)),
            allow_nan: is_truthy(_py, obj_from_bits(allow_nan_bits)),
            sort_keys: is_truthy(_py, obj_from_bits(sort_keys_bits)),
            indent_text,
            item_separator,
            key_separator,
            default_fn: bits_to_optional_callable(default_fn_bits),
        };

        let mut out = String::with_capacity(128);
        let mut stack = Vec::new();
        let mut stack_set = HashSet::new();
        match object_to_json_with_options(
            _py,
            obj_from_bits(obj_bits),
            &mut out,
            &options,
            0,
            &mut stack,
            &mut stack_set,
        ) {
            Ok(()) => {
                let ptr = alloc_string(_py, out.as_bytes());
                if ptr.is_null() {
                    return raise_exception::<u64>(_py, "MemoryError", "failed to allocate string");
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(JsonEncodeError::Raised(bits)) => bits,
            Err(JsonEncodeError::Type(msg)) => raise_exception::<u64>(_py, "TypeError", &msg),
            Err(JsonEncodeError::Value(msg)) => raise_exception::<u64>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_loads_ex(
    text_bits: u64,
    parse_float_bits: u64,
    parse_int_bits: u64,
    parse_constant_bits: u64,
    object_hook_bits: u64,
    object_pairs_hook_bits: u64,
    strict_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let payload_obj = obj_from_bits(text_bits);
        let text = if let Some(text) = string_obj_to_owned(payload_obj) {
            text
        } else if let Some(ptr) = payload_obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let tn = type_name(_py, payload_obj);
                let msg = format!("the JSON object must be str, bytes or bytearray, not {tn}");
                return raise_exception::<u64>(_py, "TypeError", &msg);
            }
            let bytes = unsafe {
                let len = bytes_len(ptr);
                let data_ptr = bytes_data(ptr);
                std::slice::from_raw_parts(data_ptr, len)
            };
            match decode_json_text(_py, payload_obj, bytes) {
                Ok(text) => text,
                Err(bits) => return bits,
            }
        } else {
            let tn = type_name(_py, payload_obj);
            let msg = format!("the JSON object must be str, bytes or bytearray, not {tn}");
            return raise_exception::<u64>(_py, "TypeError", &msg);
        };

        let options = JsonLoadsOptions {
            parse_float: bits_to_optional_callable(parse_float_bits),
            parse_int: bits_to_optional_callable(parse_int_bits),
            parse_constant: bits_to_optional_callable(parse_constant_bits),
            object_hook: bits_to_optional_callable(object_hook_bits),
            object_pairs_hook: bits_to_optional_callable(object_pairs_hook_bits),
            strict: is_truthy(_py, obj_from_bits(strict_bits)),
        };
        match run_json_parse(_py, &text, options) {
            Ok(bits) => bits,
            Err(err) => raise_json_parse_error(_py, &text, err),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_raw_decode_ex(
    text_bits: u64,
    idx_bits: u64,
    parse_float_bits: u64,
    parse_int_bits: u64,
    parse_constant_bits: u64,
    object_hook_bits: u64,
    object_pairs_hook_bits: u64,
    strict_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            let tn = type_name(_py, obj_from_bits(text_bits));
            let msg = format!("first argument must be a string, not {tn}");
            return raise_exception::<u64>(_py, "TypeError", &msg);
        };
        let Some(idx) = to_i64(obj_from_bits(idx_bits)) else {
            let tn = type_name(_py, obj_from_bits(idx_bits));
            let msg = format!("'{tn}' object cannot be interpreted as an integer");
            return raise_exception::<u64>(_py, "TypeError", &msg);
        };
        if idx < 0 {
            return raise_exception::<u64>(_py, "ValueError", "idx cannot be negative");
        }

        let options = JsonLoadsOptions {
            parse_float: bits_to_optional_callable(parse_float_bits),
            parse_int: bits_to_optional_callable(parse_int_bits),
            parse_constant: bits_to_optional_callable(parse_constant_bits),
            object_hook: bits_to_optional_callable(object_hook_bits),
            object_pairs_hook: bits_to_optional_callable(object_pairs_hook_bits),
            strict: is_truthy(_py, obj_from_bits(strict_bits)),
        };
        match run_json_raw_decode(_py, &text, idx as usize, options) {
            Ok((value_bits, end_idx)) => {
                let tuple_ptr = alloc_tuple(
                    _py,
                    &[value_bits, MoltObject::from_int(end_idx as i64).bits()],
                );
                dec_ref_bits(_py, value_bits);
                if tuple_ptr.is_null() {
                    return raise_exception::<u64>(_py, "MemoryError", "failed to allocate tuple");
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
            Err(err) => raise_json_parse_error(_py, &text, err),
        }
    })
}
