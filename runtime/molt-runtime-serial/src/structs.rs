use crate::bridge::*;
use abi::abi_types::{PyObject, PyTypeObject};
use abi::api::memory::BufferAcquireError;
use molt_cpython_abi as abi;
use molt_cpython_abi::native_custody::{
    NativeObjectOwner, PyBUF_SIMPLE, PyBUF_WRITABLE, RuntimeBufferLease, acquire_buffer,
    retire_native_reference,
};
use molt_runtime_core::prelude::*;
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use std::ffi::c_void;
use std::ffi::{
    c_double, c_float, c_int, c_long, c_longlong, c_short, c_uint, c_ulong, c_ulonglong, c_ushort,
};
use std::mem::{align_of, size_of};
use std::rc::Rc;

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
                native_size(format, size_of::<c_short>(), 2)?,
                align_of::<c_short>(),
            )?,
            'H' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<c_ushort>(), 2)?,
                align_of::<c_ushort>(),
            )?,
            'i' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<c_int>(), 4)?,
                align_of::<c_int>(),
            )?,
            'I' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<c_uint>(), 4)?,
                align_of::<c_uint>(),
            )?,
            'l' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<c_long>(), 4)?,
                align_of::<c_long>(),
            )?,
            'L' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<c_ulong>(), 4)?,
                align_of::<c_ulong>(),
            )?,
            'q' => int_op(
                ch,
                repeat,
                format,
                true,
                native_size(format, size_of::<c_longlong>(), 8)?,
                align_of::<c_longlong>(),
            )?,
            'Q' => int_op(
                ch,
                repeat,
                format,
                false,
                native_size(format, size_of::<c_ulonglong>(), 8)?,
                align_of::<c_ulonglong>(),
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
                native_size(format, size_of::<c_float>(), 4)?,
                align_of::<c_float>(),
            )?,
            'd' => float_op(
                ch,
                repeat,
                format,
                native_size(format, size_of::<c_double>(), 8)?,
                align_of::<c_double>(),
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

fn format_obj_to_owned(_py: &PyToken, format_obj: MoltObject) -> Result<String, String> {
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
    if (type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY)
        && let Some(bytes) = unsafe { bytes_like_slice_raw(ptr) }
    {
        let out: String = bytes.iter().map(|b| *b as char).collect();
        return Ok(out);
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

// A counted export or the pack output owns this span. No Rust slice survives
// a Python conversion: writes happen only after each callback has returned.
struct StructWriter {
    data: *mut u8,
    len: usize,
    position: usize,
}
impl StructWriter {
    fn push(&mut self, byte: u8) {
        assert!(self.position < self.len);
        unsafe { self.data.add(self.position).write(byte) };
        self.position += 1;
    }
    fn extend_from_slice(&mut self, bytes: &[u8]) {
        assert!(bytes.len() <= self.len - self.position);
        // Source can alias a pack_into destination ('s'/'p' bytearray values).
        unsafe { std::ptr::copy(bytes.as_ptr(), self.data.add(self.position), bytes.len()) };
        self.position += bytes.len();
    }
    fn skip(&mut self, count: usize) {
        assert!(count <= self.len - self.position);
        self.position += count;
    }
}

fn push_signed(
    out: &mut StructWriter,
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
    out: &mut StructWriter,
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

fn f16_from_f64(val: f64) -> Result<u16, molt_obj_model::float_bits::FloatNarrowError> {
    molt_obj_model::float_bits::f64_to_f16_bits(val)
}

fn f16_to_f64(bits: u16) -> f64 {
    molt_obj_model::float_bits::f16_bits_to_f64(bits)
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

type StructIntrinsicError = (&'static str, String);

fn struct_unpack_values<'py>(
    _py: &'py PyToken,
    parsed: StructFormat,
    ops: &[StructOp],
    data: *const u8,
    size: usize,
) -> Result<Vec<OwnedRuntimeValue<'py>>, StructIntrinsicError> {
    let expected_values = expected_value_count(ops);
    let mut out = Vec::with_capacity(expected_values);
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
                if offset > size {
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
                if end > size {
                    return Err((
                        "ValueError",
                        "unpack requires a buffer of sufficient size".to_string(),
                    ));
                }
                let slice = unsafe { std::slice::from_raw_parts(data.add(offset), end - offset) };
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
                let bytes = bytes.to_vec();
                let ptr = alloc_bytes(_py, &bytes);
                if ptr.is_null() {
                    return Err(("MemoryError", "allocation failed".to_string()));
                }
                out.push(unsafe {
                    OwnedRuntimeValue::from_owned_bits(_py, MoltObject::from_ptr(ptr).bits())
                });
                offset = end;
            }
            StructKind::Char => {
                for _ in 0..op.count {
                    offset = align_up(offset, op.align)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    let end = offset
                        .checked_add(1)
                        .ok_or(("OverflowError", "total struct size too long".to_string()))?;
                    if end > size {
                        return Err((
                            "ValueError",
                            "unpack requires a buffer of sufficient size".to_string(),
                        ));
                    }
                    let slice =
                        unsafe { std::slice::from_raw_parts(data.add(offset), end - offset) };
                    let byte = [slice[0]];
                    let ptr = alloc_bytes(_py, &byte);
                    if ptr.is_null() {
                        return Err(("MemoryError", "allocation failed".to_string()));
                    }
                    out.push(unsafe {
                        OwnedRuntimeValue::from_owned_bits(_py, MoltObject::from_ptr(ptr).bits())
                    });
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
                    if end > size {
                        return Err((
                            "ValueError",
                            "unpack requires a buffer of sufficient size".to_string(),
                        ));
                    }
                    let slice =
                        unsafe { std::slice::from_raw_parts(data.add(offset), end - offset) };
                    out.push(unsafe {
                        OwnedRuntimeValue::from_owned_bits(
                            _py,
                            MoltObject::from_bool(slice[0] != 0).bits(),
                        )
                    });
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
                    if end > size {
                        return Err((
                            "ValueError",
                            "unpack requires a buffer of sufficient size".to_string(),
                        ));
                    }
                    let slice =
                        unsafe { std::slice::from_raw_parts(data.add(offset), end - offset) };
                    let bits = if signed {
                        let val = read_signed(slice, parsed.endian, op.size)
                            .map_err(|msg| ("OverflowError", msg))?;
                        int_bits_from_i128(_py, val)
                    } else {
                        let val = read_unsigned(slice, parsed.endian, op.size)
                            .map_err(|msg| ("OverflowError", msg))?;
                        int_bits_from_i128(_py, val as i128)
                    };
                    if exception_pending(_py) {
                        return Err(("", String::new()));
                    }
                    out.push(unsafe { OwnedRuntimeValue::from_owned_bits(_py, bits) });
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
                    if end > size {
                        return Err((
                            "ValueError",
                            "unpack requires a buffer of sufficient size".to_string(),
                        ));
                    }
                    let slice =
                        unsafe { std::slice::from_raw_parts(data.add(offset), end - offset) };
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
                    out.push(unsafe {
                        OwnedRuntimeValue::from_owned_bits(_py, MoltObject::from_float(val).bits())
                    });
                    offset = end;
                }
            }
        }
    }
    Ok(out)
}

struct StructPlan {
    format: StructFormat,
    ops: Vec<StructOp>,
    size: usize,
    values: usize,
}

fn struct_error(py: &PyToken, class: u64, message: &str) -> u64 {
    molt_runtime_core::rt_raise_class(py, class, message)
}

fn plan(py: &PyToken, error: u64, format: u64) -> Result<StructPlan, u64> {
    let text = format_obj_to_owned(py, obj_from_bits(format))
        .map_err(|message| raise_exception::<u64>(py, "TypeError", &message))?;
    let (format, ops) = parse_format(&text).map_err(|message| struct_error(py, error, &message))?;
    let size = calc_size(&ops)
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or_else(|| struct_error(py, error, "total struct size too long"))?;
    let values = expected_value_count(&ops);
    Ok(StructPlan {
        format,
        ops,
        size,
        values,
    })
}

unsafe fn native_object(bits: u64) -> NativeObjectOwner {
    unsafe {
        NativeObjectOwner::from_owned(abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits))
    }
}

fn index_object(py: &PyToken, error: u64, bits: u64) -> Result<NativeObjectOwner, u64> {
    unsafe {
        let object = native_object(bits);
        if object.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        if abi::api::abstract_number::PyIndex_Check(object.as_ptr()) == 0 {
            return Err(struct_error(
                py,
                error,
                "required argument is not an integer",
            ));
        }
        let integer = NativeObjectOwner::from_owned(abi::api::abstract_number::PyNumber_Index(
            object.as_ptr(),
        ));
        if integer.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        Ok(integer)
    }
}

fn index_value(py: &PyToken, error: u64, bits: u64) -> Result<BigInt, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_int() {
        return Ok(BigInt::from(obj.as_int_unchecked()));
    }
    if obj.is_bool() {
        return Ok(BigInt::from(u8::from(obj.as_bool().unwrap())));
    }
    let integer = index_object(py, error, bits)?;
    unsafe {
        let Some(value) = abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(integer.as_ptr()) else {
            return Err(MoltObject::none().bits());
        };
        let value = OwnedRuntimeValue::from_owned_bits(py, value);
        to_bigint(obj_from_bits(value.bits()))
            .or_else(|| to_i64(obj_from_bits(value.bits())).map(BigInt::from))
            .ok_or_else(|| {
                raise_exception::<u64>(py, "SystemError", "index result has no integer storage")
            })
    }
}

fn pointer_value(py: &PyToken, error: u64, bits: u64) -> Result<usize, u64> {
    let original = unsafe { native_object(bits) };
    if original.as_ptr().is_null() {
        return Err(MoltObject::none().bits());
    }
    let is_original_integer = unsafe { abi::api::numbers::PyLong_Check(original.as_ptr()) != 0 };
    let integer = index_object(py, error, bits)?;
    let value = unsafe {
        retire_native_reference(|| abi::api::numbers::PyLong_AsVoidPtr(integer.as_ptr()))
    };
    if exception_pending(py) {
        if is_original_integer
            && unsafe {
                abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut abi::abi_types::PyExc_OverflowError).cast(),
                ) != 0
            }
        {
            unsafe {
                abi::api::errors::PyErr_Clear();
            }
            return Err(struct_error(py, error, "int too large to convert"));
        }
        return Err(MoltObject::none().bits());
    }
    Ok(value as usize)
}

fn index_offset(py: &PyToken, bits: u64, packing: bool) -> Result<isize, u64> {
    unsafe {
        let object = native_object(bits);
        if object.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        let result = if packing {
            retire_native_reference(|| {
                abi::api::abstract_number::PyNumber_AsSsize_t(
                    object.as_ptr(),
                    (&raw mut abi::abi_types::PyExc_IndexError).cast(),
                )
            })
        } else {
            // Argument Clinic's Py_ssize_t converter preserves PyLong_AsSsize_t's
            // overflow exception and message; pack_into explicitly requests IndexError.
            let integer = NativeObjectOwner::from_owned(abi::api::abstract_number::PyNumber_Index(
                object.as_ptr(),
            ));
            if integer.as_ptr().is_null() {
                return Err(MoltObject::none().bits());
            }
            retire_native_reference(|| abi::api::numbers::PyLong_AsSsize_t(integer.as_ptr()))
        };
        if exception_pending(py) {
            Err(MoltObject::none().bits())
        } else {
            Ok(result)
        }
    }
}

fn float_value(py: &PyToken, error: u64, bits: u64) -> Result<f64, u64> {
    unsafe {
        let object = native_object(bits);
        if object.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        let value =
            retire_native_reference(|| abi::api::numbers::PyFloat_AsDouble(object.as_ptr()));
        if exception_pending(py) {
            // CPython's struct float converters deliberately replace conversion errors.
            abi::api::errors::PyErr_Clear();
            return Err(struct_error(py, error, "required argument is not a float"));
        }
        Ok(value)
    }
}

fn pack_values(
    py: &PyToken,
    error: u64,
    plan: &StructPlan,
    values: &[u64],
    data: *mut u8,
) -> Result<(), u64> {
    if plan.size != 0 {
        unsafe { std::ptr::write_bytes(data, 0, plan.size) };
    }
    let mut out = StructWriter {
        data,
        len: plan.size,
        position: 0,
    };
    let mut index = 0;
    for op in &plan.ops {
        if matches!(op.kind, StructKind::Pad) {
            out.skip(op.count * op.size);
            continue;
        }
        let count = if matches!(op.kind, StructKind::Bytes | StructKind::Pascal) {
            1
        } else {
            op.count
        };
        for _ in 0..count {
            let aligned = align_up(out.position, op.align).expect("validated struct plan");
            out.skip(aligned - out.position);
            let bits = values[index];
            index += 1;
            let obj = obj_from_bits(bits);
            match op.kind {
                StructKind::Bytes | StructKind::Pascal | StructKind::Char => {
                    let Some(ptr) = obj.as_ptr() else {
                        return Err(struct_error(
                            py,
                            error,
                            if matches!(op.kind, StructKind::Char) {
                                "char format requires a bytes object of length 1"
                            } else if op.code == 'p' {
                                "argument for 'p' must be a bytes object"
                            } else {
                                "argument for 's' must be a bytes object"
                            },
                        ));
                    };
                    let kind = unsafe { object_type_id(ptr) };
                    let is_char = matches!(op.kind, StructKind::Char);
                    if kind != TYPE_ID_BYTES && (is_char || kind != TYPE_ID_BYTEARRAY) {
                        return Err(struct_error(
                            py,
                            error,
                            &if is_char {
                                "char format requires a bytes object of length 1".to_string()
                            } else {
                                format!("argument for '{}' must be a bytes object", op.code)
                            },
                        ));
                    }
                    // Physical admission above guarantees these hooks cannot call Python.
                    // Keep only raw coordinates: an 's'/'p' source may be the destination.
                    let mut length = 0;
                    let hooks = abi::hooks::hooks_or_stubs();
                    let source = unsafe {
                        if kind == TYPE_ID_BYTES {
                            (hooks.bytes_data)(bits, &mut length)
                        } else {
                            (hooks.bytearray_data)(bits, &mut length).cast_const()
                        }
                    };
                    if is_char {
                        if length != 1 {
                            return Err(struct_error(
                                py,
                                error,
                                "char format requires a bytes object of length 1",
                            ));
                        }
                        out.push(unsafe { source.read() });
                    } else if matches!(op.kind, StructKind::Pascal) {
                        if op.size != 0 {
                            let n = length.min(op.size - 1);
                            if n != 0 {
                                unsafe {
                                    std::ptr::copy(source, data.add(out.position + 1), n);
                                }
                            }
                            unsafe {
                                data.add(out.position).write(n.min(255) as u8);
                            }
                        }
                        out.skip(op.size);
                    } else {
                        let n = length.min(op.size);
                        if n != 0 {
                            unsafe {
                                std::ptr::copy(source, data.add(out.position), n);
                            }
                        }
                        out.skip(op.size);
                    }
                }
                StructKind::Bool => {
                    let value = is_truthy(py, obj);
                    if exception_pending(py) {
                        return Err(MoltObject::none().bits());
                    }
                    out.push(u8::from(value));
                }
                StructKind::Int { signed } => {
                    if op.code == 'P' {
                        let raw = pointer_value(py, error, bits)?;
                        push_unsigned(&mut out, plan.format.endian, op.size, raw as u128)
                            .expect("validated pointer width");
                        continue;
                    }
                    let width = op.size * 8;
                    let value = match index_value(py, error, bits) {
                        Ok(value) => value,
                        Err(bits) => {
                            // Narrow/native converters replace even callback OverflowError;
                            // standard q/Q and native P return get_pylong failures unchanged.
                            let replaces_overflow = op.code != 'P'
                                && (plan.format.native_sizes || !matches!(op.code, 'q' | 'Q'));
                            if replaces_overflow
                                && unsafe {
                                    abi::api::errors::PyErr_ExceptionMatches(
                                        (&raw mut abi::abi_types::PyExc_OverflowError).cast(),
                                    ) != 0
                                }
                            {
                                unsafe {
                                    abi::api::errors::PyErr_Clear();
                                }
                                let message = if signed {
                                    signed_range_message(op.code, width)
                                } else {
                                    unsigned_range_message(op.code, width)
                                };
                                return Err(struct_error(py, error, &message));
                            }
                            return Err(bits);
                        }
                    };
                    if signed {
                        let (min, max) = signed_range(width);
                        if value < min || value > max {
                            return Err(struct_error(
                                py,
                                error,
                                &signed_range_message(op.code, width),
                            ));
                        }
                        push_signed(
                            &mut out,
                            plan.format.endian,
                            op.size,
                            value.to_i128().unwrap(),
                        )
                        .expect("validated integer width");
                    } else {
                        if value.sign() == Sign::Minus || value > unsigned_max(width) {
                            return Err(struct_error(
                                py,
                                error,
                                &unsigned_range_message(op.code, width),
                            ));
                        }
                        push_unsigned(
                            &mut out,
                            plan.format.endian,
                            op.size,
                            value.to_u128().unwrap(),
                        )
                        .expect("validated integer width");
                    }
                }
                StructKind::Float => {
                    let value = float_value(py, error, bits)?;
                    let (raw, size) = match op.size {
                        2 => (
                            u64::from(f16_from_f64(value).map_err(|_| {
                                raise_exception::<u64>(
                                    py,
                                    "OverflowError",
                                    "float too large to pack with e format",
                                )
                            })?),
                            2,
                        ),
                        4 => {
                            let bits = if plan.format.native_sizes {
                                (value as f32).to_bits()
                            } else {
                                molt_obj_model::float_bits::f64_to_f32_bits(value).map_err(
                                    |_| {
                                        raise_exception::<u64>(
                                            py,
                                            "OverflowError",
                                            "float too large to pack with f format",
                                        )
                                    },
                                )?
                            };
                            (u64::from(bits), 4)
                        }
                        8 => (value.to_bits(), 8),
                        _ => unreachable!("validated float width"),
                    };
                    push_unsigned(&mut out, plan.format.endian, size, u128::from(raw))
                        .expect("validated float width");
                }
                StructKind::Pad => unreachable!(),
            }
        }
    }
    Ok(())
}

fn packing_arguments(
    py: &PyToken,
    error: u64,
    plan: &StructPlan,
    values: u64,
    operation: &str,
) -> Result<OwnedBridgeHandleSnapshot, u64> {
    let Some(ptr) = obj_from_bits(values).as_ptr() else {
        return Err(struct_error(py, error, "pack expects a tuple"));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return Err(struct_error(py, error, "pack expects a tuple"));
    }
    let values = unsafe { seq_snapshot(ptr) };
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    if values.len() != plan.values {
        return Err(struct_error(
            py,
            error,
            &format!(
                "{operation} expected {} items for packing (got {})",
                plan.values,
                values.len()
            ),
        ));
    }
    Ok(values)
}

fn unpack_record(py: &PyToken, error: Option<u64>, plan: &StructPlan, data: *const u8) -> u64 {
    let values = match struct_unpack_values(py, plan.format, &plan.ops, data, plan.size) {
        Ok(values) => values,
        Err((kind, message)) => {
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            return match error {
                Some(error) => struct_error(py, error, &message),
                None => raise_exception::<u64>(py, kind, &message),
            };
        }
    };
    let result = alloc_tuple(py, OwnedRuntimeValue::as_bits_slice(&values));
    if result.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(result).bits()
    }
}

fn checked_offset(
    py: &PyToken,
    error: u64,
    raw: isize,
    len: usize,
    size: usize,
    packing: bool,
) -> Result<usize, u64> {
    let op = if packing { "pack_into" } else { "unpack_from" };
    let action = if packing { "packing" } else { "unpacking" };
    let n = raw as i128;
    if n < 0 && n + size as i128 > 0 {
        let msg = if packing {
            format!("no space to pack {size} bytes at offset {raw}")
        } else {
            format!("not enough data to unpack {size} bytes at offset {raw}")
        };
        return Err(struct_error(py, error, &msg));
    }
    let start = if n < 0 { n + len as i128 } else { n };
    if start < 0 {
        return Err(struct_error(
            py,
            error,
            &format!("offset {raw} out of range for {len}-byte buffer"),
        ));
    }
    let start = start as usize;
    if start > len || size > len - start {
        return Err(struct_error(
            py,
            error,
            &format!(
                "{op} requires a buffer of at least {} bytes for {action} {size} bytes at offset {raw} (actual buffer size is {len})",
                start as u128 + size as u128
            ),
        ));
    }
    Ok(start)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_calcsize(error: u64, format: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(py, {
        match plan(py, error, format) {
            Ok(plan) => int_bits_from_i128(py, plan.size as i128),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_pack(error: u64, format: u64, values: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        let values = match packing_arguments(py, error, &plan, values, "pack") {
            Ok(values) => values,
            Err(bits) => return bits,
        };
        let mut output = Vec::new();
        if output.try_reserve_exact(plan.size).is_err() {
            return raise_exception::<u64>(py, "MemoryError", "struct output allocation failed");
        }
        output.resize(plan.size, 0);
        if let Err(bits) = pack_values(py, error, &plan, &values, output.as_mut_ptr()) {
            return bits;
        }
        let result = alloc_bytes(py, &output);
        if result.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(result).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_pack_into(
    error: u64,
    format: u64,
    buffer: u64,
    offset: u64,
    values: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        let values = match packing_arguments(py, error, &plan, values, "pack_into") {
            Ok(values) => values,
            Err(bits) => return bits,
        };
        let lease = match unsafe { acquire_buffer(buffer, PyBUF_WRITABLE) } {
            Ok(lease) => lease,
            Err(BufferAcquireError::Exporter) => {
                unsafe {
                    abi::api::errors::PyErr_Clear();
                }
                return raise_exception::<u64>(
                    py,
                    "TypeError",
                    &format!(
                        "argument must be read-write bytes-like object, not {}",
                        if obj_from_bits(buffer).is_none() {
                            "None".into()
                        } else {
                            type_name(py, obj_from_bits(buffer))
                        }
                    ),
                );
            }
            Err(BufferAcquireError::Preparation) => return MoltObject::none().bits(),
        };
        let offset = match index_offset(py, offset, true) {
            Ok(offset) => offset,
            Err(bits) => return bits,
        };
        let view = unsafe { &*lease.descriptor() };
        let start = match checked_offset(py, error, offset, view.len as usize, plan.size, true) {
            Ok(start) => start,
            Err(bits) => return bits,
        };
        let data = if view.len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            view.buf.cast::<u8>()
        };
        if let Err(bits) = pack_values(py, error, &plan, &values, unsafe { data.add(start) }) {
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_unpack(error: u64, format: u64, buffer: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        let lease = match unsafe { acquire_buffer(buffer, PyBUF_SIMPLE) } {
            Ok(lease) => lease,
            Err(_) => return MoltObject::none().bits(),
        };
        let view = unsafe { &*lease.descriptor() };
        if view.len as usize != plan.size {
            return struct_error(
                py,
                error,
                &format!("unpack requires a buffer of {} bytes", plan.size),
            );
        }
        let data = if view.len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            view.buf.cast::<u8>()
        };
        unpack_record(py, Some(error), &plan, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_unpack_from(
    error: u64,
    format: u64,
    buffer: u64,
    offset: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        let lease = match unsafe { acquire_buffer(buffer, PyBUF_SIMPLE) } {
            Ok(lease) => lease,
            Err(_) => return MoltObject::none().bits(),
        };
        let offset = match index_offset(py, offset, false) {
            Ok(offset) => offset,
            Err(bits) => return bits,
        };
        let view = unsafe { &*lease.descriptor() };
        let start = match checked_offset(py, error, offset, view.len as usize, plan.size, false) {
            Ok(start) => start,
            Err(bits) => return bits,
        };
        let data = if view.len == 0 {
            std::ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            view.buf.cast::<u8>()
        };
        unpack_record(py, Some(error), &plan, unsafe { data.add(start) })
    })
}

// A normal native GC object, not an opaque runtime handle. Its only Python
// edge is the acquired export; the parsed plan contains no Python references.
#[repr(C)]
struct UnpackIterator {
    base: PyObject,
    state: *mut UnpackState,
}
struct UnpackState {
    plan: Rc<StructPlan>,
    lease: RuntimeBufferLease,
    index: usize,
}

unsafe extern "C" fn unpack_iter_clear(object: *mut PyObject) -> c_int {
    // Detach before release callbacks can reenter this iterator.
    let state = unsafe {
        std::ptr::replace(
            &mut (*object.cast::<UnpackIterator>()).state,
            std::ptr::null_mut(),
        )
    };
    if !state.is_null() {
        unsafe {
            drop(Box::from_raw(state));
        }
    }
    0
}

unsafe extern "C" fn unpack_iter_dealloc(object: *mut PyObject) {
    unsafe {
        let class = (*object).ob_type;
        abi::api::memory::PyObject_GC_UnTrack(object.cast());
        unpack_iter_clear(object);
        abi::api::memory::PyObject_GC_Del(object.cast());
        abi::api::refcount::Py_DECREF(class.cast());
    }
}

unsafe extern "C" fn unpack_iter_traverse(
    object: *mut PyObject,
    visit: *mut c_void,
    argument: *mut c_void,
) -> c_int {
    let visit: unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int =
        unsafe { std::mem::transmute(visit) };
    let result = unsafe { visit((*object).ob_type.cast(), argument) };
    if result != 0 {
        return result;
    }
    let state = unsafe { (*object.cast::<UnpackIterator>()).state };
    if state.is_null() {
        return 0;
    }
    let owner = unsafe { (*state).lease.owner() };
    if owner.is_null() {
        0
    } else {
        unsafe { visit(owner, argument) }
    }
}

unsafe extern "C" fn unpack_iter_hint(
    object: *mut PyObject,
    _unused: *mut PyObject,
) -> *mut PyObject {
    let state = unsafe { (*object.cast::<UnpackIterator>()).state };
    let remaining = if state.is_null() {
        0
    } else {
        unsafe {
            let state = &*state;
            ((*state.lease.descriptor()).len - state.index as isize) / state.plan.size as isize
        }
    };
    unsafe { abi::api::numbers::PyLong_FromSsize_t(remaining) }
}

unsafe extern "C" fn unpack_iter_next(object: *mut PyObject) -> *mut PyObject {
    molt_runtime_core::with_gil_entry!(py, {
        let state = unsafe { (*object.cast::<UnpackIterator>()).state };
        if state.is_null() {
            return std::ptr::null_mut();
        }
        let (plan, lease, index) = unsafe {
            let state = &*state;
            if state.index >= (*state.lease.descriptor()).len as usize {
                unpack_iter_clear(object);
                return std::ptr::null_mut();
            }
            // Allocation/GC can clear the published state. Local owners preserve
            // the same parsed plan and export without retaining a Rust borrow.
            (Rc::clone(&state.plan), state.lease.clone(), state.index)
        };
        let data = unsafe { (*lease.descriptor()).buf.cast::<u8>().add(index) };
        let result =
            unsafe { OwnedRuntimeValue::from_owned_bits(py, unpack_record(py, None, &plan, data)) };
        let current = unsafe { (*object.cast::<UnpackIterator>()).state };
        if !current.is_null() {
            unsafe {
                (*current).index += plan.size;
            }
        }
        drop(lease);
        drop(plan);
        if exception_pending(py) {
            drop(result);
            return unsafe {
                abi::bridge::GLOBAL_BRIDGE
                    .owned_result_to_pyobj(abi::hooks::OwnedHandleResult::error())
            };
        }
        unsafe { abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(result.into_bits()) }
    })
}

static mut UNPACK_ITER_METHODS: [abi::abi_types::PyMethodDef; 2] = [
    abi::abi_types::PyMethodDef {
        ml_name: c"__length_hint__".as_ptr(),
        ml_meth: Some(unpack_iter_hint),
        ml_flags: abi::abi_types::METH_NOARGS,
        ml_doc: std::ptr::null(),
    },
    abi::abi_types::PyMethodDef {
        ml_name: std::ptr::null(),
        ml_meth: None,
        ml_flags: 0,
        ml_doc: std::ptr::null(),
    },
];

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_unpack_iterator_type() -> u64 {
    molt_runtime_core::with_gil_entry!(py, {
        use abi::type_slots::*;
        let mut slots = [
            abi::abi_types::PyType_Slot {
                slot: Py_tp_dealloc,
                pfunc: unpack_iter_dealloc as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_clear,
                pfunc: unpack_iter_clear as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_traverse,
                pfunc: unpack_iter_traverse as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_iter,
                pfunc: abi::api::object::PyObject_SelfIter as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_iternext,
                pfunc: unpack_iter_next as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_getattro,
                pfunc: abi::api::object::PyObject_GenericGetAttr as *mut c_void,
            },
            abi::abi_types::PyType_Slot {
                slot: Py_tp_methods,
                pfunc: (&raw mut UNPACK_ITER_METHODS).cast(),
            },
            abi::abi_types::PyType_Slot {
                slot: 0,
                pfunc: std::ptr::null_mut(),
            },
        ];
        // Py_TPFLAGS_* are C `unsigned long` and PyType_Spec.flags is C
        // `unsigned int`; the cast narrows on LP64 and is the identity on
        // LLP64 and wasm32.
        #[allow(
            clippy::unnecessary_cast,
            reason = "c_ulong is u64 on LP64 but u32 on LLP64 and wasm32"
        )]
        let flags = (abi::abi_types::Py_TPFLAGS_HAVE_GC
            | abi::abi_types::Py_TPFLAGS_IMMUTABLETYPE
            | abi::abi_types::Py_TPFLAGS_DISALLOW_INSTANTIATION) as u32;
        let mut spec = abi::abi_types::PyType_Spec {
            name: c"_struct.unpack_iterator".as_ptr(),
            basicsize: size_of::<UnpackIterator>() as c_int,
            itemsize: 0,
            flags,
            slots: slots.as_mut_ptr(),
        };
        unsafe {
            let class =
                NativeObjectOwner::from_owned(abi::api::typeobj::PyType_FromSpec(&mut spec));
            if class.as_ptr().is_null() {
                return MoltObject::none().bits();
            }
            match abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(class.as_ptr()) {
                Some(bits) => bits,
                None => {
                    let _ = py;
                    MoltObject::none().bits()
                }
            }
        }
    })
}

unsafe fn admitted_iterator_class(object: *mut PyObject) -> bool {
    unsafe {
        if abi::bridge::molt_capi_pyobj_is_bridge_managed(object) != 0
            || abi::api::typeobj::PyType_Check(object) == 0
        {
            return false;
        }
        let class = object.cast::<PyTypeObject>();
        let required = abi::abi_types::Py_TPFLAGS_HAVE_GC
            | abi::abi_types::Py_TPFLAGS_IMMUTABLETYPE
            | abi::abi_types::Py_TPFLAGS_DISALLOW_INSTANTIATION;
        (*class).tp_flags & required == required
            && (*class).tp_flags & abi::abi_types::Py_TPFLAGS_BASETYPE == 0
            && (*class).tp_basicsize == size_of::<UnpackIterator>() as isize
            && (*class).tp_itemsize == 0
            && (*class).tp_dictoffset == 0
            && (*class).tp_weaklistoffset == 0
            && (*class).tp_flags
                & (abi::abi_types::Py_TPFLAGS_MANAGED_DICT
                    | abi::abi_types::Py_TPFLAGS_MANAGED_WEAKREF)
                == 0
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_dealloc)
                == unpack_iter_dealloc as *mut c_void
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_clear)
                == unpack_iter_clear as *mut c_void
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_traverse)
                == unpack_iter_traverse as *mut c_void
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_iter)
                == abi::api::object::PyObject_SelfIter as *mut c_void
            && abi::api::typeobj::PyType_GetSlot(class, abi::type_slots::Py_tp_iternext)
                == unpack_iter_next as *mut c_void
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_struct_iter_unpack(error: u64, class: u64, format: u64, buffer: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(py, {
        let plan = match plan(py, error, format) {
            Ok(plan) => plan,
            Err(bits) => return bits,
        };
        if plan.size == 0 {
            return struct_error(
                py,
                error,
                "cannot iteratively unpack with a struct of length 0",
            );
        }
        unsafe {
            let class = native_object(class);
            if class.as_ptr().is_null() {
                return MoltObject::none().bits();
            }
            if !admitted_iterator_class(class.as_ptr()) {
                return raise_exception::<u64>(
                    py,
                    "TypeError",
                    "invalid struct iterator declaration",
                );
            }
            let iterator = NativeObjectOwner::from_owned(abi::api::typeobj::PyType_GenericAlloc(
                class.as_ptr().cast(),
                0,
            ));
            if iterator.as_ptr().is_null() {
                return MoltObject::none().bits();
            }
            let lease = match acquire_buffer(buffer, PyBUF_SIMPLE) {
                Ok(lease) => lease,
                Err(_) => return MoltObject::none().bits(),
            };
            if !((*lease.descriptor()).len as usize).is_multiple_of(plan.size) {
                return struct_error(
                    py,
                    error,
                    &format!(
                        "iterative unpacking requires a buffer of a multiple of {} bytes",
                        plan.size
                    ),
                );
            }
            let state = Box::new(UnpackState {
                plan: Rc::new(plan),
                lease,
                index: 0,
            });
            (*iterator.as_ptr().cast::<UnpackIterator>()).state = Box::into_raw(state);
            abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(iterator.as_ptr())
                .unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[cfg(test)]
mod float_pack_tests {
    use super::*;

    #[test]
    fn struct_e_and_f_reject_finite_overflow() {
        assert!(f16_from_f64(65_520.0).is_err());
        assert!(molt_obj_model::float_bits::f64_to_f32_bits(f64::MAX).is_err());
    }

    #[test]
    fn struct_e_uses_direct_f64_ties_to_even_authority() {
        assert_eq!(f16_from_f64(1.0 + 2f64.powi(-11)), Ok(0x3c00));
        assert_eq!(f16_from_f64(1.0 + 3.0 * 2f64.powi(-11)), Ok(0x3c02));
    }
}
