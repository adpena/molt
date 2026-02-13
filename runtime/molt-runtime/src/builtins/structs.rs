use crate::*;
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use std::mem::{align_of, size_of};

#[derive(Clone, Copy)]
enum StructEndian {
    Native,
    Little,
    Big,
}

#[derive(Clone, Copy)]
struct StructFormat {
    endian: StructEndian,
    aligned: bool,
    native_sizes: bool,
}

#[derive(Clone, Copy)]
enum StructKind {
    Pad,
    Int { signed: bool },
    Float,
    Bool,
    Char,
    Bytes,
    Pascal,
}

#[derive(Clone, Copy)]
struct StructOp {
    kind: StructKind,
    count: usize,
    size: usize,
    align: usize,
    code: char,
}

fn parse_format(text: &str) -> Result<(StructFormat, Vec<StructOp>), String> {
    let mut chars = text.chars().peekable();
    let mut format = StructFormat {
        endian: StructEndian::Native,
        aligned: true,
        native_sizes: true,
    };
    if let Some(&ch) = chars.peek() {
        match ch {
            '@' => {
                chars.next();
            }
            '=' => {
                chars.next();
                format = StructFormat {
                    endian: StructEndian::Native,
                    aligned: false,
                    native_sizes: false,
                };
            }
            '<' => {
                chars.next();
                format = StructFormat {
                    endian: StructEndian::Little,
                    aligned: false,
                    native_sizes: false,
                };
            }
            '>' | '!' => {
                chars.next();
                format = StructFormat {
                    endian: StructEndian::Big,
                    aligned: false,
                    native_sizes: false,
                };
            }
            _ => {}
        }
    }
    let mut ops = Vec::new();
    let mut count: usize = 0;
    let mut count_set = false;
    for ch in chars {
        if ch.is_ascii_digit() {
            let digit = (ch as u8 - b'0') as usize;
            count = count
                .checked_mul(10)
                .and_then(|val| val.checked_add(digit))
                .ok_or_else(|| "total struct size too long".to_string())?;
            count_set = true;
            continue;
        }
        let repeat = if count_set { count } else { 1 };
        count = 0;
        count_set = false;
        let op = match ch {
            'x' => StructOp {
                kind: StructKind::Pad,
                count: repeat,
                size: 1,
                align: 1,
                code: ch,
            },
            'c' => StructOp {
                kind: StructKind::Char,
                count: repeat,
                size: 1,
                align: 1,
                code: ch,
            },
            '?' => StructOp {
                kind: StructKind::Bool,
                count: repeat,
                size: 1,
                align: 1,
                code: ch,
            },
            's' | 'p' => {
                let len = repeat;
                StructOp {
                    kind: if ch == 's' {
                        StructKind::Bytes
                    } else {
                        StructKind::Pascal
                    },
                    count: 1,
                    size: len,
                    align: 1,
                    code: ch,
                }
            }
            'b' => int_op(ch, repeat, format, true, size_of::<i8>(), align_of::<i8>())?,
            'B' => int_op(ch, repeat, format, false, size_of::<u8>(), align_of::<u8>())?,
            'h' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<libc::c_short>(), 2)?,
                align_of::<libc::c_short>(),
            )?,
            'H' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<libc::c_ushort>(), 2)?,
                align_of::<libc::c_ushort>(),
            )?,
            'i' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<libc::c_int>(), 4)?,
                align_of::<libc::c_int>(),
            )?,
            'I' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<libc::c_uint>(), 4)?,
                align_of::<libc::c_uint>(),
            )?,
            'l' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<libc::c_long>(), 4)?,
                align_of::<libc::c_long>(),
            )?,
            'L' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<libc::c_ulong>(), 4)?,
                align_of::<libc::c_ulong>(),
            )?,
            'q' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<libc::c_longlong>(), 8)?,
                align_of::<libc::c_longlong>(),
            )?,
            'Q' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<libc::c_ulonglong>(), 8)?,
                align_of::<libc::c_ulonglong>(),
            )?,
            'n' => native_only_int_op(
                ch,
                repeat,
                format,
                true,
                size_of::<isize>(),
                align_of::<isize>(),
            )?,
            'N' => native_only_int_op(
                ch,
                repeat,
                format,
                false,
                size_of::<usize>(),
                align_of::<usize>(),
            )?,
            'P' => native_only_int_op(
                ch,
                repeat,
                format,
                false,
                size_of::<usize>(),
                align_of::<usize>(),
            )?,
            'f' => float_op(
                ch,
                repeat,
                format,
                native_size(format, size_of::<libc::c_float>(), 4)?,
                align_of::<libc::c_float>(),
            )?,
            'd' => float_op(
                ch,
                repeat,
                format,
                native_size(format, size_of::<libc::c_double>(), 8)?,
                align_of::<libc::c_double>(),
            )?,
            'e' => float_op(ch, repeat, format, 2, align_of::<u16>())?,
            _ => return Err("bad char in struct format".to_string()),
        };
        ops.push(op);
    }
    if count_set {
        return Err("repeat count given without format specifier".to_string());
    }
    Ok((format, ops))
}

fn format_obj_to_owned(_py: &PyToken<'_>, format_obj: MoltObject) -> Result<String, String> {
    if let Some(text) = string_obj_to_owned(format_obj) {
        return Ok(text);
    }
    let Some(ptr) = format_obj.as_ptr() else {
        let type_label = type_name(_py, format_obj);
        return Err(format!(
            "Struct() argument 1 must be a str or bytes object, not {type_label}"
        ));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
        if let Some(bytes) = unsafe { bytes_like_slice_raw(ptr) } {
            let out: String = bytes.iter().map(|b| *b as char).collect();
            return Ok(out);
        }
    }
    let type_label = type_name(_py, format_obj);
    Err(format!(
        "Struct() argument 1 must be a str or bytes object, not {type_label}"
    ))
}

fn native_size(format: StructFormat, native: usize, standard: usize) -> Result<usize, String> {
    if format.native_sizes {
        Ok(native)
    } else {
        Ok(standard)
    }
}

fn int_op(
    code: char,
    repeat: usize,
    format: StructFormat,
    signed: bool,
    size: usize,
    native_align: usize,
) -> Result<StructOp, String> {
    let align = if format.aligned { native_align } else { 1 };
    Ok(StructOp {
        kind: StructKind::Int { signed },
        count: repeat,
        size,
        align,
        code,
    })
}

fn native_only_int_op(
    code: char,
    repeat: usize,
    format: StructFormat,
    signed: bool,
    size: usize,
    native_align: usize,
) -> Result<StructOp, String> {
    if !format.native_sizes {
        return Err("bad char in struct format".to_string());
    }
    int_op(code, repeat, format, signed, size, native_align)
}

fn float_op(
    code: char,
    repeat: usize,
    format: StructFormat,
    size: usize,
    native_align: usize,
) -> Result<StructOp, String> {
    let align = if format.aligned { native_align } else { 1 };
    Ok(StructOp {
        kind: StructKind::Float,
        count: repeat,
        size,
        align,
        code,
    })
}

fn align_up(offset: usize, align: usize) -> Option<usize> {
    if align <= 1 {
        return Some(offset);
    }
    let aligned = offset.checked_add(align - 1)? / align * align;
    Some(aligned)
}

fn calc_size(ops: &[StructOp]) -> Option<usize> {
    let mut offset: usize = 0;
    for op in ops {
        match op.kind {
            StructKind::Pad => {
                offset = offset.checked_add(op.count * op.size)?;
            }
            StructKind::Bytes | StructKind::Pascal => {
                offset = align_up(offset, op.align)?;
                offset = offset.checked_add(op.size)?;
            }
            _ => {
                for _ in 0..op.count {
                    offset = align_up(offset, op.align)?;
                    offset = offset.checked_add(op.size)?;
                }
            }
        }
    }
    Some(offset)
}

fn expected_value_count(ops: &[StructOp]) -> usize {
    let mut total = 0usize;
    for op in ops {
        match op.kind {
            StructKind::Pad => {}
            StructKind::Bytes | StructKind::Pascal => total = total.saturating_add(1),
            _ => total = total.saturating_add(op.count),
        }
    }
    total
}

fn align_output(out: &mut Vec<u8>, offset: &mut usize, align: usize) -> Result<(), String> {
    let aligned =
        align_up(*offset, align).ok_or_else(|| "total struct size too long".to_string())?;
    if aligned > *offset {
        out.extend(std::iter::repeat_n(0u8, aligned - *offset));
        *offset = aligned;
    }
    Ok(())
}

fn push_signed(
    out: &mut Vec<u8>,
    endian: StructEndian,
    size: usize,
    val: i128,
) -> Result<(), String> {
    match size {
        1 => out.push(val as i8 as u8),
        2 => {
            let bytes = match endian {
                StructEndian::Native => (val as i16).to_ne_bytes(),
                StructEndian::Little => (val as i16).to_le_bytes(),
                StructEndian::Big => (val as i16).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        4 => {
            let bytes = match endian {
                StructEndian::Native => (val as i32).to_ne_bytes(),
                StructEndian::Little => (val as i32).to_le_bytes(),
                StructEndian::Big => (val as i32).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        8 => {
            let bytes = match endian {
                StructEndian::Native => (val as i64).to_ne_bytes(),
                StructEndian::Little => (val as i64).to_le_bytes(),
                StructEndian::Big => (val as i64).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        _ => return Err("unsupported integer size".to_string()),
    }
    Ok(())
}

fn push_unsigned(
    out: &mut Vec<u8>,
    endian: StructEndian,
    size: usize,
    val: u128,
) -> Result<(), String> {
    match size {
        1 => out.push(val as u8),
        2 => {
            let bytes = match endian {
                StructEndian::Native => (val as u16).to_ne_bytes(),
                StructEndian::Little => (val as u16).to_le_bytes(),
                StructEndian::Big => (val as u16).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        4 => {
            let bytes = match endian {
                StructEndian::Native => (val as u32).to_ne_bytes(),
                StructEndian::Little => (val as u32).to_le_bytes(),
                StructEndian::Big => (val as u32).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        8 => {
            let bytes = match endian {
                StructEndian::Native => (val as u64).to_ne_bytes(),
                StructEndian::Little => (val as u64).to_le_bytes(),
                StructEndian::Big => (val as u64).to_be_bytes(),
            };
            out.extend_from_slice(&bytes);
        }
        _ => return Err("unsupported integer size".to_string()),
    }
    Ok(())
}

fn read_signed(bytes: &[u8], endian: StructEndian, size: usize) -> Result<i128, String> {
    let val = match size {
        1 => i8::from_ne_bytes([bytes[0]]) as i128,
        2 => {
            let mut buf = [0u8; 2];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..2]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..2]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..2]),
            }
            (match endian {
                StructEndian::Native => i16::from_ne_bytes(buf),
                StructEndian::Little => i16::from_le_bytes(buf),
                StructEndian::Big => i16::from_be_bytes(buf),
            }) as i128
        }
        4 => {
            let mut buf = [0u8; 4];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..4]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..4]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..4]),
            }
            (match endian {
                StructEndian::Native => i32::from_ne_bytes(buf),
                StructEndian::Little => i32::from_le_bytes(buf),
                StructEndian::Big => i32::from_be_bytes(buf),
            }) as i128
        }
        8 => {
            let mut buf = [0u8; 8];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..8]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..8]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..8]),
            }
            (match endian {
                StructEndian::Native => i64::from_ne_bytes(buf),
                StructEndian::Little => i64::from_le_bytes(buf),
                StructEndian::Big => i64::from_be_bytes(buf),
            }) as i128
        }
        _ => return Err("unsupported integer size".to_string()),
    };
    Ok(val)
}

fn read_unsigned(bytes: &[u8], endian: StructEndian, size: usize) -> Result<u128, String> {
    let val = match size {
        1 => u8::from_ne_bytes([bytes[0]]) as u128,
        2 => {
            let mut buf = [0u8; 2];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..2]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..2]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..2]),
            }
            (match endian {
                StructEndian::Native => u16::from_ne_bytes(buf),
                StructEndian::Little => u16::from_le_bytes(buf),
                StructEndian::Big => u16::from_be_bytes(buf),
            }) as u128
        }
        4 => {
            let mut buf = [0u8; 4];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..4]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..4]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..4]),
            }
            (match endian {
                StructEndian::Native => u32::from_ne_bytes(buf),
                StructEndian::Little => u32::from_le_bytes(buf),
                StructEndian::Big => u32::from_be_bytes(buf),
            }) as u128
        }
        8 => {
            let mut buf = [0u8; 8];
            match endian {
                StructEndian::Native => buf.copy_from_slice(&bytes[..8]),
                StructEndian::Little => buf.copy_from_slice(&bytes[..8]),
                StructEndian::Big => buf.copy_from_slice(&bytes[..8]),
            }
            (match endian {
                StructEndian::Native => u64::from_ne_bytes(buf),
                StructEndian::Little => u64::from_le_bytes(buf),
                StructEndian::Big => u64::from_be_bytes(buf),
            }) as u128
        }
        _ => return Err("unsupported integer size".to_string()),
    };
    Ok(val)
}

fn f16_from_f64(val: f64) -> u16 {
    let f = val as f32;
    let bits = f.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7fffff;

    if exp == 0xff {
        if mant == 0 {
            return (sign | 0x7c00) as u16;
        }
        let mut payload = (mant >> 13) as u16;
        if payload == 0 {
            payload = 1;
        }
        return (sign | 0x7c00 | payload as u32) as u16;
    }

    let mut exp16 = exp - 127 + 15;
    if exp16 >= 0x1f {
        return (sign | 0x7c00) as u16;
    }
    if exp16 <= 0 {
        if exp16 < -10 {
            return sign as u16;
        }
        let mant32 = mant | 0x800000;
        let shift = 14 - exp16;
        let mut mant16 = (mant32 >> shift) as u16;
        let round_bit = 1u32 << (shift - 1);
        let remainder = mant32 & (round_bit - 1);
        if (mant32 & round_bit) != 0 && (remainder != 0 || (mant16 & 1) != 0) {
            mant16 = mant16.wrapping_add(1);
        }
        return (sign as u16) | mant16;
    }

    let mut mant16 = (mant >> 13) as u16;
    let round_bit = 1u32 << 12;
    let remainder = mant & (round_bit - 1);
    if (mant & round_bit) != 0 && (remainder != 0 || (mant16 & 1) != 0) {
        mant16 = mant16.wrapping_add(1);
        if (mant16 & 0x400) != 0 {
            mant16 = 0;
            exp16 += 1;
            if exp16 >= 0x1f {
                return (sign | 0x7c00) as u16;
            }
        }
    }
    (sign | ((exp16 as u32) << 10) | mant16 as u32) as u16
}

fn f16_to_f64(bits: u16) -> f64 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = ((bits >> 10) & 0x1f) as i32;
    let mant = (bits & 0x03ff) as u32;
    let f32_bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            let mut mant_norm = mant;
            let mut exp_norm = -14;
            while (mant_norm & 0x400) == 0 {
                mant_norm <<= 1;
                exp_norm -= 1;
            }
            mant_norm &= 0x03ff;
            let exp32 = (exp_norm + 127) as u32;
            sign | (exp32 << 23) | (mant_norm << 13)
        }
    } else if exp == 0x1f {
        sign | 0x7f800000 | (mant << 13)
    } else {
        let exp32 = (exp - 15 + 127) as u32;
        sign | (exp32 << 23) | (mant << 13)
    };
    f32::from_bits(f32_bits) as f64
}

fn obj_to_bigint(obj: MoltObject) -> Result<BigInt, String> {
    if let Some(val) = to_i64(obj) {
        return Ok(BigInt::from(val));
    }
    if let Some(big) = to_bigint(obj) {
        return Ok(big);
    }
    Err("required argument is not an integer".to_string())
}

fn signed_range(bits: usize) -> (BigInt, BigInt) {
    let one = BigInt::from(1u8);
    let max = (&one << (bits - 1)) - 1;
    let min = -(&one << (bits - 1));
    (min, max)
}

fn unsigned_max(bits: usize) -> BigInt {
    let one = BigInt::from(1u8);
    (&one << bits) - 1
}

fn signed_range_message(code: char, bits: usize) -> String {
    let (min, max) = signed_range(bits);
    format!("'{code}' format requires {min} <= number <= {max}")
}

fn unsigned_range_message(code: char, bits: usize) -> String {
    let max = unsigned_max(bits);
    format!("'{code}' format requires 0 <= number <= {max}")
}

unsafe fn memoryview_contiguous_bytes(ptr: *mut u8) -> Option<&'static [u8]> {
    unsafe {
        let (owner_ptr, base_offset, nbytes) = memoryview_contiguous_window(ptr, false)?;
        let base = bytes_like_slice_raw(owner_ptr)?;
        Some(&base[base_offset..base_offset + nbytes])
    }
}

unsafe fn memoryview_contiguous_window(
    mut view_ptr: *mut u8,
    writable: bool,
) -> Option<(*mut u8, usize, usize)> {
    unsafe {
        const MAX_DEPTH: usize = 64;
        if !memoryview_is_c_contiguous_view(view_ptr) {
            return None;
        }
        if writable && memoryview_readonly(view_ptr) {
            return None;
        }
        let nbytes = memoryview_nbytes(view_ptr);
        let mut base_offset = 0usize;
        for _ in 0..MAX_DEPTH {
            if !memoryview_is_c_contiguous_view(view_ptr) {
                return None;
            }
            if writable && memoryview_readonly(view_ptr) {
                return None;
            }
            let rel_offset = memoryview_offset(view_ptr);
            if rel_offset < 0 {
                return None;
            }
            base_offset = base_offset.checked_add(rel_offset as usize)?;
            let owner = obj_from_bits(memoryview_owner_bits(view_ptr));
            let owner_ptr = owner.as_ptr()?;
            let owner_type_id = object_type_id(owner_ptr);
            if owner_type_id == TYPE_ID_MEMORYVIEW {
                view_ptr = owner_ptr;
                continue;
            }
            if writable && owner_type_id != TYPE_ID_BYTEARRAY {
                return None;
            }
            let base = bytes_like_slice_raw(owner_ptr)?;
            let end = base_offset.checked_add(nbytes)?;
            if end > base.len() {
                return None;
            }
            return Some((owner_ptr, base_offset, nbytes));
        }
        None
    }
}

type StructIntrinsicError = (&'static str, String);

fn struct_read_buffer_bytes(
    _py: &PyToken<'_>,
    buffer_obj: MoltObject,
) -> Result<&'static [u8], StructIntrinsicError> {
    let Some(buffer_ptr) = buffer_obj.as_ptr() else {
        let type_label = type_name(_py, buffer_obj);
        return Err((
            "TypeError",
            format!("a bytes-like object is required, not '{type_label}'"),
        ));
    };
    let type_id = unsafe { object_type_id(buffer_ptr) };
    let buf = if type_id == TYPE_ID_MEMORYVIEW {
        if !unsafe { memoryview_is_c_contiguous_view(buffer_ptr) } {
            return Err((
                "BufferError",
                "memoryview: underlying buffer is not C-contiguous".to_string(),
            ));
        }
        unsafe { memoryview_contiguous_bytes(buffer_ptr) }
    } else if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
        unsafe { bytes_like_slice_raw(buffer_ptr) }
    } else {
        None
    };
    let Some(buf) = buf else {
        let type_label = type_name(_py, buffer_obj);
        return Err((
            "TypeError",
            format!("a bytes-like object is required, not '{type_label}'"),
        ));
    };
    Ok(buf)
}

fn struct_index_offset(_py: &PyToken<'_>, offset_bits: u64) -> Option<i64> {
    let offset_obj = obj_from_bits(offset_bits);
    let msg = format!(
        "'{}' object cannot be interpreted as an integer",
        type_name(_py, offset_obj)
    );
    let offset = index_i64_from_obj(_py, offset_bits, msg.as_str());
    if exception_pending(_py) {
        return None;
    }
    Some(offset)
}

fn struct_normalize_offset(raw_offset: i64, buf_len: usize) -> Option<usize> {
    let mut start = i128::from(raw_offset);
    if start < 0 {
        start = start.checked_add(buf_len as i128)?;
    }
    usize::try_from(start).ok()
}

fn struct_unpack_values(
    _py: &PyToken<'_>,
    parsed: StructFormat,
    ops: &[StructOp],
    buf: &[u8],
) -> Result<Vec<u64>, StructIntrinsicError> {
    let expected_values = expected_value_count(ops);
    let mut out: Vec<u64> = Vec::with_capacity(expected_values);
    let mut offset = 0usize;
    for op in ops {
        match op.kind {
            StructKind::Pad => {
                let delta = op
                    .count
                    .checked_mul(op.size)
                    .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                offset = offset
                    .checked_add(delta)
                    .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                if offset > buf.len() {
                    return Err((
                        "ValueError",
                        "unpack requires a buffer of sufficient size".to_string(),
                    ));
                }
            }
            StructKind::Bytes | StructKind::Pascal => {
                offset = align_up(offset, op.align)
                    .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                let end = offset
                    .checked_add(op.size)
                    .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                if end > buf.len() {
                    return Err((
                        "ValueError",
                        "unpack requires a buffer of sufficient size".to_string(),
                    ));
                }
                let slice = &buf[offset..end];
                let bytes = if matches!(op.kind, StructKind::Pascal) {
                    if op.size == 0 {
                        &[][..]
                    } else {
                        let len = std::cmp::min(slice[0] as usize, op.size.saturating_sub(1));
                        &slice[1..1 + len]
                    }
                } else {
                    slice
                };
                let ptr = alloc_bytes(_py, bytes);
                if ptr.is_null() {
                    return Err(("MemoryError", "allocation failed".to_string()));
                }
                out.push(MoltObject::from_ptr(ptr).bits());
                offset = end;
            }
            StructKind::Char => {
                for _ in 0..op.count {
                    offset = align_up(offset, op.align)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    let end = offset
                        .checked_add(1)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    if end > buf.len() {
                        return Err((
                            "ValueError",
                            "unpack requires a buffer of sufficient size".to_string(),
                        ));
                    }
                    let slice = &buf[offset..end];
                    let ptr = alloc_bytes(_py, slice);
                    if ptr.is_null() {
                        return Err(("MemoryError", "allocation failed".to_string()));
                    }
                    out.push(MoltObject::from_ptr(ptr).bits());
                    offset = end;
                }
            }
            StructKind::Bool => {
                for _ in 0..op.count {
                    offset = align_up(offset, op.align)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    let end = offset
                        .checked_add(1)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    if end > buf.len() {
                        return Err((
                            "ValueError",
                            "unpack requires a buffer of sufficient size".to_string(),
                        ));
                    }
                    let slice = &buf[offset..end];
                    out.push(MoltObject::from_bool(slice[0] != 0).bits());
                    offset = end;
                }
            }
            StructKind::Int { signed } => {
                for _ in 0..op.count {
                    offset = align_up(offset, op.align)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    let end = offset
                        .checked_add(op.size)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    if end > buf.len() {
                        return Err((
                            "ValueError",
                            "unpack requires a buffer of sufficient size".to_string(),
                        ));
                    }
                    let slice = &buf[offset..end];
                    let bits = if signed {
                        let val = read_signed(slice, parsed.endian, op.size)
                            .map_err(|msg| ("OverflowError", msg))?;
                        int_bits_from_i128(_py, val)
                    } else {
                        let val = read_unsigned(slice, parsed.endian, op.size)
                            .map_err(|msg| ("OverflowError", msg))?;
                        int_bits_from_i128(_py, val as i128)
                    };
                    out.push(bits);
                    offset = end;
                }
            }
            StructKind::Float => {
                for _ in 0..op.count {
                    offset = align_up(offset, op.align)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    let end = offset
                        .checked_add(op.size)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    if end > buf.len() {
                        return Err((
                            "ValueError",
                            "unpack requires a buffer of sufficient size".to_string(),
                        ));
                    }
                    let slice = &buf[offset..end];
                    let val = match op.size {
                        2 => {
                            let mut buf2 = [0u8; 2];
                            buf2.copy_from_slice(slice);
                            let raw = match parsed.endian {
                                StructEndian::Native => u16::from_ne_bytes(buf2),
                                StructEndian::Little => u16::from_le_bytes(buf2),
                                StructEndian::Big => u16::from_be_bytes(buf2),
                            };
                            f16_to_f64(raw)
                        }
                        4 => {
                            let mut buf4 = [0u8; 4];
                            buf4.copy_from_slice(slice);
                            let raw = match parsed.endian {
                                StructEndian::Native => f32::from_ne_bytes(buf4),
                                StructEndian::Little => f32::from_le_bytes(buf4),
                                StructEndian::Big => f32::from_be_bytes(buf4),
                            };
                            raw as f64
                        }
                        8 => {
                            let mut buf8 = [0u8; 8];
                            buf8.copy_from_slice(slice);
                            match parsed.endian {
                                StructEndian::Native => f64::from_ne_bytes(buf8),
                                StructEndian::Little => f64::from_le_bytes(buf8),
                                StructEndian::Big => f64::from_be_bytes(buf8),
                            }
                        }
                        _ => return Err(("OverflowError", "unsupported float size".to_string())),
                    };
                    out.push(MoltObject::from_float(val).bits());
                    offset = end;
                }
            }
        }
    }
    Ok(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_calcsize(format_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let format_obj = obj_from_bits(format_bits);
        let format = match format_obj_to_owned(_py, format_obj) {
            Ok(format) => format,
            Err(msg) => return raise_exception::<u64>(_py, "TypeError", msg.as_str()),
        };
        let (_parsed, ops) = match parse_format(&format) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", msg.as_str()),
        };
        let Some(size) = calc_size(&ops) else {
            return raise_exception::<u64>(_py, "OverflowError", "total struct size too long");
        };
        MoltObject::from_int(size as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_pack(format_bits: u64, values_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let format_obj = obj_from_bits(format_bits);
        let format = match format_obj_to_owned(_py, format_obj) {
            Ok(format) => format,
            Err(msg) => return raise_exception::<u64>(_py, "TypeError", msg.as_str()),
        };
        let (parsed, ops) = match parse_format(&format) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", msg.as_str()),
        };
        let values_obj = obj_from_bits(values_bits);
        let Some(values_ptr) = values_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "pack expects a tuple");
        };
        unsafe {
            if object_type_id(values_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<u64>(_py, "TypeError", "pack expects a tuple");
            }
        }
        let values = unsafe { seq_vec_ref(values_ptr) };
        let expected = expected_value_count(&ops);
        if values.len() != expected {
            let msg = format!(
                "pack expected {expected} items for packing (got {})",
                values.len()
            );
            return raise_exception::<u64>(_py, "TypeError", msg.as_str());
        }
        let Some(size) = calc_size(&ops) else {
            return raise_exception::<u64>(_py, "OverflowError", "total struct size too long");
        };
        let mut out = Vec::with_capacity(size);
        let mut offset: usize = 0;
        let mut idx = 0usize;
        for op in ops {
            match op.kind {
                StructKind::Pad => {
                    out.extend(std::iter::repeat_n(0u8, op.count * op.size));
                    offset = match offset.checked_add(op.count * op.size) {
                        Some(val) => val,
                        None => {
                            return raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "total struct size too long",
                            );
                        }
                    };
                }
                StructKind::Bytes | StructKind::Pascal => {
                    if align_output(&mut out, &mut offset, op.align).is_err() {
                        return raise_exception::<u64>(
                            _py,
                            "OverflowError",
                            "total struct size too long",
                        );
                    }
                    let obj = obj_from_bits(values[idx]);
                    idx += 1;
                    let Some(ptr) = obj.as_ptr() else {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            &format!("argument for '{}' must be a bytes object", op.code),
                        );
                    };
                    let type_id = unsafe { object_type_id(ptr) };
                    if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            &format!("argument for '{}' must be a bytes object", op.code),
                        );
                    }
                    let slice = unsafe { bytes_like_slice_raw(ptr) }.unwrap_or(&[]);
                    let mut buf = vec![0u8; op.size];
                    if op.size > 0 {
                        match op.kind {
                            StructKind::Bytes => {
                                let copy_len = std::cmp::min(slice.len(), op.size);
                                buf[..copy_len].copy_from_slice(&slice[..copy_len]);
                            }
                            StructKind::Pascal => {
                                let max = op.size.saturating_sub(1);
                                let copy_len = std::cmp::min(slice.len(), max);
                                buf[0] = copy_len as u8;
                                if copy_len > 0 {
                                    buf[1..1 + copy_len].copy_from_slice(&slice[..copy_len]);
                                }
                            }
                            _ => {}
                        }
                    }
                    out.extend_from_slice(&buf);
                    offset = match offset.checked_add(op.size) {
                        Some(val) => val,
                        None => {
                            return raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "total struct size too long",
                            );
                        }
                    };
                }
                StructKind::Char => {
                    for _ in 0..op.count {
                        if align_output(&mut out, &mut offset, op.align).is_err() {
                            return raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "total struct size too long",
                            );
                        }
                        let obj = obj_from_bits(values[idx]);
                        idx += 1;
                        let Some(ptr) = obj.as_ptr() else {
                            return raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "char format requires a bytes object of length 1",
                            );
                        };
                        let type_id = unsafe { object_type_id(ptr) };
                        if type_id != TYPE_ID_BYTES {
                            return raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "char format requires a bytes object of length 1",
                            );
                        }
                        let slice = unsafe { bytes_like_slice_raw(ptr) }.unwrap_or(&[]);
                        if slice.len() != 1 {
                            return raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "char format requires a bytes object of length 1",
                            );
                        }
                        out.push(slice[0]);
                        offset = match offset.checked_add(1) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "total struct size too long",
                                );
                            }
                        };
                    }
                }
                StructKind::Bool => {
                    for _ in 0..op.count {
                        if align_output(&mut out, &mut offset, op.align).is_err() {
                            return raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "total struct size too long",
                            );
                        }
                        let obj = obj_from_bits(values[idx]);
                        idx += 1;
                        let truthy = is_truthy(_py, obj);
                        out.push(if truthy { 1 } else { 0 });
                        offset = match offset.checked_add(1) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "total struct size too long",
                                );
                            }
                        };
                    }
                }
                StructKind::Int { signed } => {
                    for _ in 0..op.count {
                        if align_output(&mut out, &mut offset, op.align).is_err() {
                            return raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "total struct size too long",
                            );
                        }
                        let obj = obj_from_bits(values[idx]);
                        idx += 1;
                        let bits = op.size.saturating_mul(8);
                        let bigint = match obj_to_bigint(obj) {
                            Ok(val) => val,
                            Err(msg) => return raise_exception::<u64>(_py, "TypeError", &msg),
                        };
                        if signed {
                            let (min, max) = signed_range(bits);
                            if bigint < min || bigint > max {
                                let msg = signed_range_message(op.code, bits);
                                return raise_exception::<u64>(_py, "OverflowError", &msg);
                            }
                            let val = match bigint.to_i128() {
                                Some(val) => val,
                                None => {
                                    let msg = signed_range_message(op.code, bits);
                                    return raise_exception::<u64>(_py, "OverflowError", &msg);
                                }
                            };
                            if let Err(err) = push_signed(&mut out, parsed.endian, op.size, val) {
                                return raise_exception::<u64>(_py, "OverflowError", &err);
                            }
                        } else {
                            if bigint.sign() == Sign::Minus {
                                let msg = unsigned_range_message(op.code, bits);
                                return raise_exception::<u64>(_py, "OverflowError", &msg);
                            }
                            let max = unsigned_max(bits);
                            if bigint > max {
                                let msg = unsigned_range_message(op.code, bits);
                                return raise_exception::<u64>(_py, "OverflowError", &msg);
                            }
                            let val = match bigint.to_u128() {
                                Some(val) => val,
                                None => {
                                    let msg = unsigned_range_message(op.code, bits);
                                    return raise_exception::<u64>(_py, "OverflowError", &msg);
                                }
                            };
                            if let Err(err) = push_unsigned(&mut out, parsed.endian, op.size, val) {
                                return raise_exception::<u64>(_py, "OverflowError", &err);
                            }
                        }
                        offset = match offset.checked_add(op.size) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "total struct size too long",
                                );
                            }
                        };
                    }
                }
                StructKind::Float => {
                    for _ in 0..op.count {
                        if align_output(&mut out, &mut offset, op.align).is_err() {
                            return raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "total struct size too long",
                            );
                        }
                        let obj = obj_from_bits(values[idx]);
                        idx += 1;
                        let Some(val) = to_f64(obj) else {
                            return raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "required argument is not a float",
                            );
                        };
                        let bytes = match op.size {
                            2 => {
                                let bits = f16_from_f64(val);
                                match parsed.endian {
                                    StructEndian::Native => bits.to_ne_bytes().to_vec(),
                                    StructEndian::Little => bits.to_le_bytes().to_vec(),
                                    StructEndian::Big => bits.to_be_bytes().to_vec(),
                                }
                            }
                            4 => match parsed.endian {
                                StructEndian::Native => (val as f32).to_ne_bytes().to_vec(),
                                StructEndian::Little => (val as f32).to_le_bytes().to_vec(),
                                StructEndian::Big => (val as f32).to_be_bytes().to_vec(),
                            },
                            8 => match parsed.endian {
                                StructEndian::Native => { val }.to_ne_bytes().to_vec(),
                                StructEndian::Little => { val }.to_le_bytes().to_vec(),
                                StructEndian::Big => { val }.to_be_bytes().to_vec(),
                            },
                            _ => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "unsupported float size",
                                );
                            }
                        };
                        out.extend_from_slice(&bytes);
                        offset = match offset.checked_add(op.size) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "total struct size too long",
                                );
                            }
                        };
                    }
                }
            }
        }
        let ptr = alloc_bytes(_py, &out);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_unpack(format_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let format_obj = obj_from_bits(format_bits);
        let format = match format_obj_to_owned(_py, format_obj) {
            Ok(format) => format,
            Err(msg) => return raise_exception::<u64>(_py, "TypeError", msg.as_str()),
        };
        let (parsed, ops) = match parse_format(&format) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", msg.as_str()),
        };
        let Some(expected_size) = calc_size(&ops) else {
            return raise_exception::<u64>(_py, "OverflowError", "total struct size too long");
        };
        let buffer_obj = obj_from_bits(buffer_bits);
        let buf = match struct_read_buffer_bytes(_py, buffer_obj) {
            Ok(buf) => buf,
            Err((exc, msg)) => return raise_exception::<u64>(_py, exc, msg.as_str()),
        };
        if buf.len() != expected_size {
            let msg = format!("unpack requires a buffer of {expected_size} bytes");
            return raise_exception::<u64>(_py, "ValueError", msg.as_str());
        }
        let out = match struct_unpack_values(_py, parsed, &ops, buf) {
            Ok(values) => values,
            Err((exc, msg)) => return raise_exception::<u64>(_py, exc, msg.as_str()),
        };
        let tuple_ptr = alloc_tuple(_py, &out);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_pack_into(buffer_bits: u64, offset_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let payload_obj = obj_from_bits(data_bits);
        let payload_bytes = match struct_read_buffer_bytes(_py, payload_obj) {
            Ok(buf) => buf,
            Err((exc, msg)) => return raise_exception::<u64>(_py, exc, msg.as_str()),
        };
        let payload = payload_bytes.to_vec();
        let payload_len = payload.len();
        let Some(raw_offset) = struct_index_offset(_py, offset_bits) else {
            return MoltObject::none().bits();
        };
        let buffer_obj = obj_from_bits(buffer_bits);
        let Some(buffer_ptr) = buffer_obj.as_ptr() else {
            let msg = format!(
                "argument must be read-write bytes-like object, not {}",
                type_name(_py, buffer_obj)
            );
            return raise_exception::<u64>(_py, "TypeError", msg.as_str());
        };
        let type_id = unsafe { object_type_id(buffer_ptr) };
        let no_space_pack_message = |offset: i64| -> String {
            format!("no space to pack {payload_len} bytes at offset {offset}")
        };
        let out_of_range_message = |offset: i64, buf_len: usize| -> String {
            format!("offset {offset} out of range for {buf_len}-byte buffer")
        };
        let requires_message = |offset: i64, start: usize, buf_len: usize| -> String {
            format!(
                "pack_into requires a buffer of at least {} bytes for packing {payload_len} bytes at offset {offset} (actual buffer size is {buf_len})",
                start.saturating_add(payload_len)
            )
        };
        if type_id == TYPE_ID_BYTEARRAY {
            let buf = unsafe { bytearray_vec(buffer_ptr) };
            let buf_len = buf.len();
            let Some(start) = struct_normalize_offset(raw_offset, buf_len) else {
                let msg = out_of_range_message(raw_offset, buf_len);
                return raise_exception::<u64>(_py, "ValueError", msg.as_str());
            };
            let Some(end) = start.checked_add(payload_len) else {
                let msg = requires_message(raw_offset, start, buf_len);
                return raise_exception::<u64>(_py, "ValueError", msg.as_str());
            };
            if end > buf_len {
                let msg = if raw_offset < 0 {
                    no_space_pack_message(raw_offset)
                } else {
                    requires_message(raw_offset, start, buf_len)
                };
                return raise_exception::<u64>(_py, "ValueError", msg.as_str());
            }
            if payload_len > 0 {
                buf[start..end].copy_from_slice(&payload);
            }
            return MoltObject::none().bits();
        }
        if type_id == TYPE_ID_MEMORYVIEW {
            if !unsafe { memoryview_is_c_contiguous_view(buffer_ptr) } {
                let msg = format!(
                    "argument must be read-write bytes-like object, not {}",
                    type_name(_py, buffer_obj)
                );
                return raise_exception::<u64>(_py, "TypeError", msg.as_str());
            }
            if unsafe { memoryview_readonly(buffer_ptr) } {
                let msg = format!(
                    "argument must be read-write bytes-like object, not {}",
                    type_name(_py, buffer_obj)
                );
                return raise_exception::<u64>(_py, "TypeError", msg.as_str());
            }
            let buf_len = unsafe { memoryview_nbytes(buffer_ptr) };
            let Some(start) = struct_normalize_offset(raw_offset, buf_len) else {
                let msg = out_of_range_message(raw_offset, buf_len);
                return raise_exception::<u64>(_py, "ValueError", msg.as_str());
            };
            let Some(end) = start.checked_add(payload_len) else {
                let msg = requires_message(raw_offset, start, buf_len);
                return raise_exception::<u64>(_py, "ValueError", msg.as_str());
            };
            if end > buf_len {
                let msg = if raw_offset < 0 {
                    no_space_pack_message(raw_offset)
                } else {
                    requires_message(raw_offset, start, buf_len)
                };
                return raise_exception::<u64>(_py, "ValueError", msg.as_str());
            }
            let Some((owner_ptr, owner_offset, owner_nbytes)) =
                (unsafe { memoryview_contiguous_window(buffer_ptr, true) })
            else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "argument must be read-write bytes-like object, not memoryview",
                );
            };
            if owner_nbytes != buf_len {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "argument must be read-write bytes-like object, not memoryview",
                );
            }
            let Some(start_abs) = owner_offset.checked_add(start) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "argument must be read-write bytes-like object, not memoryview",
                );
            };
            let Some(end_abs) = start_abs.checked_add(payload_len) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "argument must be read-write bytes-like object, not memoryview",
                );
            };
            let owner_buf = unsafe { bytearray_vec(owner_ptr) };
            if end_abs > owner_buf.len() {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "argument must be read-write bytes-like object, not memoryview",
                );
            }
            if payload_len > 0 {
                owner_buf[start_abs..end_abs].copy_from_slice(&payload);
            }
            return MoltObject::none().bits();
        }
        let msg = format!(
            "argument must be read-write bytes-like object, not {}",
            type_name(_py, buffer_obj)
        );
        raise_exception::<u64>(_py, "TypeError", msg.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_unpack_from(
    format_bits: u64,
    buffer_bits: u64,
    offset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let format_obj = obj_from_bits(format_bits);
        let format = match format_obj_to_owned(_py, format_obj) {
            Ok(format) => format,
            Err(msg) => return raise_exception::<u64>(_py, "TypeError", msg.as_str()),
        };
        let (parsed, ops) = match parse_format(&format) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", msg.as_str()),
        };
        let Some(size) = calc_size(&ops) else {
            return raise_exception::<u64>(_py, "OverflowError", "total struct size too long");
        };
        let buffer_obj = obj_from_bits(buffer_bits);
        let buf = match struct_read_buffer_bytes(_py, buffer_obj) {
            Ok(buf) => buf,
            Err((exc, msg)) => return raise_exception::<u64>(_py, exc, msg.as_str()),
        };
        let Some(raw_offset) = struct_index_offset(_py, offset_bits) else {
            return MoltObject::none().bits();
        };
        let buf_len = buf.len();
        let Some(start) = struct_normalize_offset(raw_offset, buf_len) else {
            let msg = format!("offset {raw_offset} out of range for {buf_len}-byte buffer");
            return raise_exception::<u64>(_py, "ValueError", msg.as_str());
        };
        let Some(end) = start.checked_add(size) else {
            let msg = format!(
                "unpack_from requires a buffer of at least {} bytes for unpacking {size} bytes at offset {raw_offset} (actual buffer size is {buf_len})",
                start.saturating_add(size)
            );
            return raise_exception::<u64>(_py, "ValueError", msg.as_str());
        };
        if end > buf_len {
            if raw_offset < 0 {
                let msg = format!("not enough data to unpack {size} bytes at offset {raw_offset}");
                return raise_exception::<u64>(_py, "ValueError", msg.as_str());
            }
            let msg = format!(
                "unpack_from requires a buffer of at least {} bytes for unpacking {size} bytes at offset {raw_offset} (actual buffer size is {buf_len})",
                start.saturating_add(size)
            );
            return raise_exception::<u64>(_py, "ValueError", msg.as_str());
        }
        let out = match struct_unpack_values(_py, parsed, &ops, &buf[start..end]) {
            Ok(values) => values,
            Err((exc, msg)) => return raise_exception::<u64>(_py, exc, msg.as_str()),
        };
        let tuple_ptr = alloc_tuple(_py, &out);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_iter_unpack(format_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let format_obj = obj_from_bits(format_bits);
        let format = match format_obj_to_owned(_py, format_obj) {
            Ok(format) => format,
            Err(msg) => return raise_exception::<u64>(_py, "TypeError", msg.as_str()),
        };
        let (parsed, ops) = match parse_format(&format) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", msg.as_str()),
        };
        let Some(size) = calc_size(&ops) else {
            return raise_exception::<u64>(_py, "OverflowError", "total struct size too long");
        };
        if size == 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "cannot iteratively unpack with a struct of length 0",
            );
        }
        let buffer_obj = obj_from_bits(buffer_bits);
        let buf = match struct_read_buffer_bytes(_py, buffer_obj) {
            Ok(buf) => buf,
            Err((exc, msg)) => return raise_exception::<u64>(_py, exc, msg.as_str()),
        };
        if buf.len() % size != 0 {
            let msg =
                format!("iterative unpacking requires a buffer of a multiple of {size} bytes");
            return raise_exception::<u64>(_py, "ValueError", msg.as_str());
        }
        let tuple_count = buf.len() / size;
        let mut tuples = Vec::with_capacity(tuple_count);
        let mut offset = 0usize;
        while offset < buf.len() {
            let end = offset + size;
            let values = match struct_unpack_values(_py, parsed, &ops, &buf[offset..end]) {
                Ok(values) => values,
                Err((exc, msg)) => return raise_exception::<u64>(_py, exc, msg.as_str()),
            };
            let tuple_ptr = alloc_tuple(_py, &values);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            tuples.push(MoltObject::from_ptr(tuple_ptr).bits());
            offset = end;
        }
        let list_ptr = alloc_list(_py, &tuples);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}
