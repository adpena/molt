use crate::*;

// TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): expand struct format coverage (alignment, signed/unsigned widths, strings, pack_into/unpack_from/iter_unpack) to full CPython parity.

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
}

#[derive(Clone, Copy)]
enum StructCode {
    Int32,
    Float64,
    Pad,
}

#[derive(Clone, Copy)]
struct StructOp {
    code: StructCode,
    count: usize,
}

fn parse_format(text: &str) -> Result<(StructFormat, Vec<StructOp>), String> {
    let mut chars = text.chars().peekable();
    let mut endian = StructEndian::Native;
    let mut aligned = true;
    if let Some(&ch) = chars.peek() {
        match ch {
            '@' | '=' => {
                chars.next();
                endian = StructEndian::Native;
                aligned = ch == '@';
            }
            '<' => {
                chars.next();
                endian = StructEndian::Little;
                aligned = false;
            }
            '>' | '!' => {
                chars.next();
                endian = StructEndian::Big;
                aligned = false;
            }
            _ => {}
        }
    }
    let mut ops = Vec::new();
    let mut count: usize = 0;
    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            let digit = (ch as u8 - b'0') as usize;
            count = count
                .checked_mul(10)
                .and_then(|val| val.checked_add(digit))
                .ok_or_else(|| "format count overflow".to_string())?;
            continue;
        }
        let repeat = if count == 0 { 1 } else { count };
        count = 0;
        let code = match ch {
            'i' => StructCode::Int32,
            'd' => StructCode::Float64,
            'x' => StructCode::Pad,
            _ => return Err(format!("unsupported format character '{ch}'")),
        };
        ops.push(StructOp { code, count: repeat });
    }
    if count != 0 {
        return Err("format ends with count".to_string());
    }
    Ok((StructFormat { endian, aligned }, ops))
}

fn align_up(offset: usize, align: usize) -> Option<usize> {
    if align <= 1 {
        return Some(offset);
    }
    let aligned = offset.checked_add(align - 1)? / align * align;
    Some(aligned)
}

fn code_size(code: StructCode) -> usize {
    match code {
        StructCode::Int32 => 4,
        StructCode::Float64 => 8,
        StructCode::Pad => 1,
    }
}

fn code_align(code: StructCode) -> usize {
    match code {
        StructCode::Int32 => 4,
        StructCode::Float64 => 8,
        StructCode::Pad => 1,
    }
}

fn calc_size(format: StructFormat, ops: &[StructOp]) -> Option<usize> {
    let mut offset: usize = 0;
    for op in ops {
        match op.code {
            StructCode::Pad => {
                offset = offset.checked_add(op.count)?;
            }
            _ => {
                let align = if format.aligned {
                    code_align(op.code)
                } else {
                    1
                };
                let size = code_size(op.code);
                for _ in 0..op.count {
                    offset = align_up(offset, align)?;
                    offset = offset.checked_add(size)?;
                }
            }
        }
    }
    Some(offset)
}

fn expected_value_count(ops: &[StructOp]) -> usize {
    let mut total = 0usize;
    for op in ops {
        match op.code {
            StructCode::Pad => {}
            _ => total = total.saturating_add(op.count),
        }
    }
    total
}

fn push_i32_bytes(out: &mut Vec<u8>, endian: StructEndian, val: i32) {
    let bytes = match endian {
        StructEndian::Native => val.to_ne_bytes(),
        StructEndian::Little => val.to_le_bytes(),
        StructEndian::Big => val.to_be_bytes(),
    };
    out.extend_from_slice(&bytes);
}

fn push_f64_bytes(out: &mut Vec<u8>, endian: StructEndian, val: f64) {
    let bytes = match endian {
        StructEndian::Native => val.to_ne_bytes(),
        StructEndian::Little => val.to_le_bytes(),
        StructEndian::Big => val.to_be_bytes(),
    };
    out.extend_from_slice(&bytes);
}

fn read_i32(bytes: &[u8], endian: StructEndian) -> i32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes);
    match endian {
        StructEndian::Native => i32::from_ne_bytes(buf),
        StructEndian::Little => i32::from_le_bytes(buf),
        StructEndian::Big => i32::from_be_bytes(buf),
    }
}

fn read_f64(bytes: &[u8], endian: StructEndian) -> f64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(bytes);
    match endian {
        StructEndian::Native => f64::from_ne_bytes(buf),
        StructEndian::Little => f64::from_le_bytes(buf),
        StructEndian::Big => f64::from_be_bytes(buf),
    }
}

#[no_mangle]
pub extern "C" fn molt_struct_calcsize(format_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let format_obj = obj_from_bits(format_bits);
        let Some(format) = string_obj_to_owned(format_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "struct format must be a string");
        };
        let (parsed, ops) = match parse_format(&format) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", msg.as_str()),
        };
        let Some(size) = calc_size(parsed, &ops) else {
            return raise_exception::<u64>(_py, "OverflowError", "struct format size overflow");
        };
        MoltObject::from_int(size as i64).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_struct_pack(format_bits: u64, values_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let format_obj = obj_from_bits(format_bits);
        let Some(format) = string_obj_to_owned(format_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "struct format must be a string");
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
        let Some(size) = calc_size(parsed, &ops) else {
            return raise_exception::<u64>(_py, "OverflowError", "struct format size overflow");
        };
        let mut out = Vec::with_capacity(size);
        let mut offset: usize = 0;
        let mut idx = 0usize;
        for op in ops {
            match op.code {
                StructCode::Pad => {
                    out.extend(std::iter::repeat(0u8).take(op.count));
                    offset = match offset.checked_add(op.count) {
                        Some(val) => val,
                        None => {
                            return raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "struct format size overflow",
                            )
                        }
                    };
                }
                StructCode::Int32 => {
                    for _ in 0..op.count {
                        let align = if parsed.aligned {
                            code_align(StructCode::Int32)
                        } else {
                            1
                        };
                        let aligned = match align_up(offset, align) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "struct format size overflow",
                                )
                            }
                        };
                        if aligned > offset {
                            out.extend(std::iter::repeat(0u8).take(aligned - offset));
                            offset = aligned;
                        }
                        let obj = obj_from_bits(values[idx]);
                        idx += 1;
                        let Some(val) = to_i64(obj) else {
                            return raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "pack expected an int",
                            );
                        };
                        if val < i32::MIN as i64 || val > i32::MAX as i64 {
                            return raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "int out of range for 'i' format",
                            );
                        }
                        push_i32_bytes(&mut out, parsed.endian, val as i32);
                        offset = match offset.checked_add(code_size(StructCode::Int32)) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "struct format size overflow",
                                )
                            }
                        };
                    }
                }
                StructCode::Float64 => {
                    for _ in 0..op.count {
                        let align = if parsed.aligned {
                            code_align(StructCode::Float64)
                        } else {
                            1
                        };
                        let aligned = match align_up(offset, align) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "struct format size overflow",
                                )
                            }
                        };
                        if aligned > offset {
                            out.extend(std::iter::repeat(0u8).take(aligned - offset));
                            offset = aligned;
                        }
                        let obj = obj_from_bits(values[idx]);
                        idx += 1;
                        let Some(val) = to_f64(obj) else {
                            return raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "pack expected a float",
                            );
                        };
                        push_f64_bytes(&mut out, parsed.endian, val);
                        offset = match offset.checked_add(code_size(StructCode::Float64)) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "struct format size overflow",
                                )
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

#[no_mangle]
pub extern "C" fn molt_struct_unpack(format_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let format_obj = obj_from_bits(format_bits);
        let Some(format) = string_obj_to_owned(format_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "struct format must be a string");
        };
        let (parsed, ops) = match parse_format(&format) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", msg.as_str()),
        };
        let Some(expected_size) = calc_size(parsed, &ops) else {
            return raise_exception::<u64>(_py, "OverflowError", "struct format size overflow");
        };
        let buffer_obj = obj_from_bits(buffer_bits);
        let Some(buffer_ptr) = buffer_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "unpack expects bytes");
        };
        let Some(buf) = (unsafe { bytes_like_slice_raw(buffer_ptr) }) else {
            return raise_exception::<u64>(_py, "TypeError", "unpack expects bytes");
        };
        if buf.len() != expected_size {
            let msg = format!(
                "unpack requires a buffer of {expected_size} bytes (got {})",
                buf.len()
            );
            return raise_exception::<u64>(_py, "ValueError", msg.as_str());
        }
        let expected_values = expected_value_count(&ops);
        let mut out: Vec<u64> = Vec::with_capacity(expected_values);
        let mut offset = 0usize;
        for op in ops {
            match op.code {
                StructCode::Pad => {
                    offset = offset.saturating_add(op.count);
                }
                StructCode::Int32 => {
                    for _ in 0..op.count {
                        let align = if parsed.aligned {
                            code_align(StructCode::Int32)
                        } else {
                            1
                        };
                        let aligned = match align_up(offset, align) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "struct format size overflow",
                                )
                            }
                        };
                        offset = aligned;
                        let end = offset + 4;
                        let val = read_i32(&buf[offset..end], parsed.endian) as i64;
                        out.push(int_bits_from_i64(_py, val));
                        offset = end;
                    }
                }
                StructCode::Float64 => {
                    for _ in 0..op.count {
                        let align = if parsed.aligned {
                            code_align(StructCode::Float64)
                        } else {
                            1
                        };
                        let aligned = match align_up(offset, align) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "struct format size overflow",
                                )
                            }
                        };
                        offset = aligned;
                        let end = offset + 8;
                        let val = read_f64(&buf[offset..end], parsed.endian);
                        out.push(MoltObject::from_float(val).bits());
                        offset = end;
                    }
                }
            }
        }
        let tuple_ptr = alloc_tuple(_py, &out);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}
