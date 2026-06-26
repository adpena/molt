use super::*;

pub(super) fn ctypes_attr_present(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<bool, u64> {
    match urllib_request_attr_optional(_py, obj_bits, name)? {
        Some(bits) => {
            dec_ref_bits(_py, bits);
            Ok(true)
        }
        None => Ok(false),
    }
}

pub(super) fn ctypes_is_scalar_ctype(
    _py: &molt_runtime_core::CoreGilToken,
    ctype_bits: u64,
) -> Result<bool, u64> {
    let has_size = ctypes_attr_present(_py, ctype_bits, b"_size")?;
    if !has_size {
        return Ok(false);
    }
    let has_fields = ctypes_attr_present(_py, ctype_bits, b"_fields_")?;
    let has_length = ctypes_attr_present(_py, ctype_bits, b"_length")?;
    Ok(!has_fields && !has_length)
}

pub(super) fn ctypes_attr_i64(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<i64>, u64> {
    let Some(bits) = urllib_request_attr_optional(_py, obj_bits, name)? else {
        return Ok(None);
    };
    let out = to_i64(obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    Ok(out)
}

pub(super) fn ctypes_attr_bool(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<bool>, u64> {
    let Some(bits) = urllib_request_attr_optional(_py, obj_bits, name)? else {
        return Ok(None);
    };
    let out = is_truthy(_py, obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    Ok(Some(out))
}

pub(super) fn ctypes_attr_string(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<String>, u64> {
    let Some(bits) = urllib_request_attr_optional(_py, obj_bits, name)? else {
        return Ok(None);
    };
    let out = string_obj_to_owned(obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    Ok(out)
}

pub(super) fn ctypes_scalar_kind(
    _py: &molt_runtime_core::CoreGilToken,
    ctype_bits: u64,
) -> Result<String, u64> {
    Ok(ctypes_attr_string(_py, ctype_bits, b"_kind")?.unwrap_or_else(|| "int".to_string()))
}

pub(super) fn ctypes_wrap_integer(
    _py: &molt_runtime_core::CoreGilToken,
    value_bits: u64,
    bits: i64,
    signed: bool,
) -> Result<u64, u64> {
    if bits <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "ctypes integer width must be positive",
        ));
    }
    let Some(value) = index_bigint_from_obj(
        _py,
        value_bits,
        "ctypes scalar value must be int-compatible",
    ) else {
        return Err(MoltObject::none().bits());
    };
    let width = bits as usize;
    let modulus = BigInt::one() << width;
    let mut wrapped = ((value % &modulus) + &modulus) % &modulus;
    if signed {
        let sign_bit = BigInt::one() << (width - 1);
        if wrapped >= sign_bit {
            wrapped -= modulus;
        }
    }
    Ok(int_bits_from_bigint(_py, wrapped))
}

pub(super) fn ctypes_coerce_char(
    _py: &molt_runtime_core::CoreGilToken,
    value_bits: u64,
) -> Result<u64, u64> {
    if let Some(num) = to_i64(obj_from_bits(value_bits)) {
        if !(0..=255).contains(&num) {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "one character bytes, bytearray, or an integer in range(256) expected",
            ));
        }
        let ptr = alloc_bytes(_py, &[num as u8]);
        if ptr.is_null() {
            return Err(raise_exception::<u64>(
                _py,
                "MemoryError",
                "allocation failed",
            ));
        }
        return Ok(MoltObject::from_ptr(ptr).bits());
    }
    let Some(slice) = bytes_like_slice(value_bits) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "one character bytes, bytearray, or an integer in range(256) expected",
        ));
    };
    if slice.len() != 1 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "one character bytes, bytearray, or an integer in range(256) expected",
        ));
    }
    let out = alloc_bytes(_py, slice);
    if out.is_null() {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "allocation failed",
        ));
    }
    Ok(MoltObject::from_ptr(out).bits())
}

pub(super) fn ctypes_coerce_float(
    _py: &molt_runtime_core::CoreGilToken,
    value_bits: u64,
    width: i64,
) -> Result<u64, u64> {
    let float_bits = molt_float_from_obj(value_bits);
    if exception_pending(_py) {
        if obj_from_bits(float_bits).as_ptr().is_some() {
            dec_ref_bits(_py, float_bits);
        }
        return Err(float_bits);
    }
    let Some(value) = to_f64(obj_from_bits(float_bits)) else {
        if obj_from_bits(float_bits).as_ptr().is_some() {
            dec_ref_bits(_py, float_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "ctypes scalar value must be float-compatible",
        ));
    };
    if obj_from_bits(float_bits).as_ptr().is_some() {
        dec_ref_bits(_py, float_bits);
    }
    let out = if width == 32 {
        (value as f32) as f64
    } else {
        value
    };
    Ok(MoltObject::from_float(out).bits())
}

pub(super) fn ctypes_coerce_scalar_value(
    _py: &molt_runtime_core::CoreGilToken,
    ctype_bits: u64,
    value_bits: u64,
) -> Result<u64, u64> {
    let kind = ctypes_scalar_kind(_py, ctype_bits)?;
    match kind.as_str() {
        "bool" => Ok(MoltObject::from_bool(is_truthy(_py, obj_from_bits(value_bits))).bits()),
        "char" => ctypes_coerce_char(_py, value_bits),
        "float" => {
            let bits = ctypes_attr_i64(_py, ctype_bits, b"_bits")?.unwrap_or(64);
            ctypes_coerce_float(_py, value_bits, bits)
        }
        "void_p" => {
            if obj_from_bits(value_bits).is_none() {
                Ok(MoltObject::none().bits())
            } else {
                ctypes_wrap_integer(_py, value_bits, 64, false)
            }
        }
        _ => {
            let bits = ctypes_attr_i64(_py, ctype_bits, b"_bits")?.unwrap_or(64);
            let signed = ctypes_attr_bool(_py, ctype_bits, b"_signed")?.unwrap_or(true);
            ctypes_wrap_integer(_py, value_bits, bits, signed)
        }
    }
}

pub(super) fn ctypes_sizeof_bits(
    _py: &molt_runtime_core::CoreGilToken,
    obj_or_type_bits: u64,
) -> Result<u64, u64> {
    let Some(size_bits) = urllib_request_attr_optional(_py, obj_or_type_bits, b"_size")? else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "unsupported type for ctypes.sizeof",
        ));
    };
    let out = match to_i64(obj_from_bits(size_bits)) {
        // Full-range boxing — a ctypes type's _size can in principle exceed the
        // inline window (e.g. sizeof(c_char * (2**50))); `from_int` would
        // silently truncate it mod 2**47.
        Some(value) => int_bits_from_bigint(_py, BigInt::from(value)),
        None => {
            raise_exception::<u64>(_py, "TypeError", "ctypes size value must be int-compatible")
        }
    };
    dec_ref_bits(_py, size_bits);
    Ok(out)
}

pub(super) fn urllib_attr_truthy(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<bool, u64> {
    match urllib_request_attr_optional(_py, obj_bits, name)? {
        Some(bits) => {
            let out = is_truthy(_py, obj_from_bits(bits));
            dec_ref_bits(_py, bits);
            Ok(out)
        }
        None => Ok(false),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ctypes_require_ffi() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !crate::bridge::has_capability(_py, "ffi.unsafe") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "capability 'ffi.unsafe' required",
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ctypes_coerce_value(ctype_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !crate::bridge::has_capability(_py, "ffi.unsafe") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "capability 'ffi.unsafe' required",
            );
        }

        let is_scalar = match ctypes_is_scalar_ctype(_py, ctype_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !is_scalar {
            return value_bits;
        }

        ctypes_coerce_scalar_value(_py, ctype_bits, value_bits).unwrap_or_else(|bits| bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ctypes_default_value(ctype_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !crate::bridge::has_capability(_py, "ffi.unsafe") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "capability 'ffi.unsafe' required",
            );
        }

        let is_scalar = match ctypes_is_scalar_ctype(_py, ctype_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if is_scalar {
            let kind = match ctypes_scalar_kind(_py, ctype_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            return match kind.as_str() {
                "bool" => MoltObject::from_bool(false).bits(),
                "char" => {
                    let ptr = alloc_bytes(_py, &[0]);
                    if ptr.is_null() {
                        raise_exception::<u64>(_py, "MemoryError", "allocation failed")
                    } else {
                        MoltObject::from_ptr(ptr).bits()
                    }
                }
                "float" => MoltObject::from_float(0.0).bits(),
                "void_p" => MoltObject::none().bits(),
                _ => MoltObject::from_int(0).bits(),
            };
        }

        let has_fields = match ctypes_attr_present(_py, ctype_bits, b"_fields_") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let has_length = match ctypes_attr_present(_py, ctype_bits, b"_length") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if has_fields || has_length {
            let out_bits = unsafe { call_callable0(_py, ctype_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return out_bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ctypes_sizeof(obj_or_type_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !crate::bridge::has_capability(_py, "ffi.unsafe") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "capability 'ffi.unsafe' required",
            );
        }
        match ctypes_sizeof_bits(_py, obj_or_type_bits) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
