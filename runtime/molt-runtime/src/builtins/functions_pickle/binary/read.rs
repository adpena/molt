// Binary pickle byte-reader, integer/float/string decoders, and attr helpers.

use super::*;

pub(crate) fn pickle_input_to_bytes(_py: &crate::PyToken<'_>, data_bits: u64) -> Result<Vec<u8>, u64> {
    if let Some(ptr) = obj_from_bits(data_bits).as_ptr()
        && let Some(raw) = unsafe { bytes_like_slice(ptr) }
    {
        return Ok(raw.to_vec());
    }
    if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
        return Ok(text.into_bytes());
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "pickle data must be bytes, bytearray, or str",
    ))
}

pub(crate) fn pickle_read_u8(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u8, u64> {
    if *idx >= data.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let byte = data[*idx];
    *idx += 1;
    Ok(byte)
}

pub(crate) fn pickle_read_exact<'a>(
    data: &'a [u8],
    idx: &mut usize,
    n: usize,
    _py: &crate::PyToken<'_>,
) -> Result<&'a [u8], u64> {
    if data.len().saturating_sub(*idx) < n {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let end = start + n;
    *idx = end;
    Ok(&data[start..end])
}

pub(crate) fn pickle_read_line_bytes<'a>(
    data: &'a [u8],
    idx: &mut usize,
    _py: &crate::PyToken<'_>,
) -> Result<&'a [u8], u64> {
    if *idx > data.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let Some(rel_end) = data[start..].iter().position(|b| *b == b'\n') else {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    };
    let end = start + rel_end;
    *idx = end + 1;
    Ok(&data[start..end])
}

pub(crate) fn pickle_read_u16_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u16, u64> {
    let raw = pickle_read_exact(data, idx, 2, _py)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

pub(crate) fn pickle_read_u32_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u32, u64> {
    let raw = pickle_read_exact(data, idx, 4, _py)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

pub(crate) fn pickle_read_u64_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u64, u64> {
    let raw = pickle_read_exact(data, idx, 8, _py)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

pub(crate) fn pickle_parse_long_bytes_bits(_py: &crate::PyToken<'_>, raw: &[u8]) -> Result<u64, u64> {
    if raw.is_empty() {
        return Ok(MoltObject::from_int(0).bits());
    }
    if raw.len() > 8 {
        return Err(pickle_raise(
            _py,
            "pickle.loads: LONG payload exceeds Molt int range",
        ));
    }
    let negative = (raw[raw.len() - 1] & 0x80) != 0;
    let mut bytes = if negative { [0xff; 8] } else { [0u8; 8] };
    bytes[..raw.len()].copy_from_slice(raw);
    Ok(MoltObject::from_int(i64::from_le_bytes(bytes)).bits())
}

pub(crate) fn pickle_read_f64_be(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<f64, u64> {
    let raw = pickle_read_exact(data, idx, 8, _py)?;
    Ok(f64::from_bits(u64::from_be_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ])))
}

pub(crate) fn pickle_decode_utf8(_py: &crate::PyToken<'_>, raw: &[u8], ctx: &str) -> Result<String, u64> {
    String::from_utf8(raw.to_vec()).map_err(|_| {
        let msg = format!("pickle.loads: invalid UTF-8 while decoding {ctx}");
        pickle_raise(_py, &msg)
    })
}

pub(crate) fn pickle_attr_optional(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        return Ok(None);
    }
    if value_bits == missing {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

pub(crate) fn pickle_attr_required(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<u64, u64> {
    match pickle_attr_optional(_py, obj_bits, name)? {
        Some(bits) => Ok(bits),
        None => {
            let name_text = std::str::from_utf8(name).unwrap_or("attribute");
            let msg = format!("pickle: missing required attribute {name_text}");
            Err(pickle_raise(_py, &msg))
        }
    }
}

pub(crate) fn pickle_decode_8bit_string(
    _py: &crate::PyToken<'_>,
    raw: &[u8],
    encoding: &str,
    _errors: &str,
) -> Result<u64, u64> {
    if encoding.eq_ignore_ascii_case("bytes") {
        let ptr = crate::alloc_bytes(_py, raw);
        if ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(MoltObject::from_ptr(ptr).bits());
    }
    let decoded = if encoding.eq_ignore_ascii_case("latin1")
        || encoding.eq_ignore_ascii_case("latin-1")
    {
        raw.iter().map(|&b| char::from(b)).collect::<String>()
    } else {
        String::from_utf8(raw.to_vec())
            .map_err(|_| pickle_raise(_py, "pickle.loads: unable to decode 8-bit string payload"))?
    };
    let ptr = alloc_string(_py, decoded.as_bytes());
    if ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}
