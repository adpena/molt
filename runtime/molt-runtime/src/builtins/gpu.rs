use crate::{
    MoltObject, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES, alloc_bytes, bytes_data, bytes_len, obj_from_bits,
    object_type_id, raise_exception, string_obj_to_owned, to_i64,
};

#[derive(Copy, Clone, Eq, PartialEq)]
enum ScalarFormat {
    F32,
    F64,
    I64,
}

impl ScalarFormat {
    fn itemsize(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::F64 | Self::I64 => 8,
        }
    }
}

#[derive(Copy, Clone)]
struct ByteView {
    ptr: *const u8,
    len: usize,
}

fn parse_format(_py: &crate::PyToken<'_>, bits: u64, role: &str) -> Result<ScalarFormat, u64> {
    let Some(value) = string_obj_to_owned(obj_from_bits(bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a format string"),
        ));
    };
    match value.as_str() {
        "f" => Ok(ScalarFormat::F32),
        "d" => Ok(ScalarFormat::F64),
        "q" => Ok(ScalarFormat::I64),
        _ => Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            &format!("{role} format {:?} is unsupported", value),
        )),
    }
}

fn parse_usize_arg(_py: &crate::PyToken<'_>, bits: u64, role: &str) -> Result<usize, u64> {
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be an integer"),
        ));
    };
    usize::try_from(value).map_err(|_| {
        raise_exception::<_>(
            _py,
            "ValueError",
            &format!("{role} must be non-negative"),
        )
    })
}

fn bytes_like_view(_py: &crate::PyToken<'_>, bits: u64, role: &str) -> Result<ByteView, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be bytes-like"),
        ));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be bytes or bytearray"),
        ));
    }
    Ok(ByteView {
        ptr: unsafe { bytes_data(ptr) },
        len: unsafe { bytes_len(ptr) },
    })
}

unsafe fn read_scalar(ptr: *const u8, index: usize, fmt: ScalarFormat) -> f64 {
    match fmt {
        ScalarFormat::F32 => unsafe { (ptr.add(index * 4) as *const f32).read_unaligned() as f64 },
        ScalarFormat::F64 => unsafe { (ptr.add(index * 8) as *const f64).read_unaligned() },
        ScalarFormat::I64 => unsafe { (ptr.add(index * 8) as *const i64).read_unaligned() as f64 },
    }
}

unsafe fn write_scalar(ptr: *mut u8, index: usize, fmt: ScalarFormat, value: f64) {
    match fmt {
        ScalarFormat::F32 => unsafe {
            (ptr.add(index * 4) as *mut f32).write_unaligned(value as f32);
        },
        ScalarFormat::F64 => unsafe {
            (ptr.add(index * 8) as *mut f64).write_unaligned(value);
        },
        ScalarFormat::I64 => unsafe {
            (ptr.add(index * 8) as *mut i64).write_unaligned(value as i64);
        },
    }
}

unsafe fn linear_f32(
    x_ptr: *const u8,
    weight_ptr: *const u8,
    out_ptr: *mut u8,
    outer: usize,
    in_features: usize,
    out_features: usize,
) {
    for batch in 0..outer {
        let x_off = batch * in_features;
        let out_off = batch * out_features;
        for out_idx in 0..out_features {
            let w_off = out_idx * in_features;
            let mut acc = 0.0f32;
            for k in 0..in_features {
                let x = unsafe { (x_ptr.add((x_off + k) * 4) as *const f32).read_unaligned() };
                let w =
                    unsafe { (weight_ptr.add((w_off + k) * 4) as *const f32).read_unaligned() };
                acc += x * w;
            }
            unsafe { (out_ptr.add((out_off + out_idx) * 4) as *mut f32).write_unaligned(acc) };
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_linear_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    weight_data_bits: u64,
    weight_format_bits: u64,
    outer_bits: u64,
    in_features_bits: u64,
    out_features_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let weight_format = match parse_format(_py, weight_format_bits, "weight_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let outer = match parse_usize_arg(_py, outer_bits, "outer") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let in_features = match parse_usize_arg(_py, in_features_bits, "in_features") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_features = match parse_usize_arg(_py, out_features_bits, "out_features") {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let weight_view = match bytes_like_view(_py, weight_data_bits, "weight_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let Some(x_required) = outer
            .checked_mul(in_features)
            .and_then(|n| n.checked_mul(x_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "x_data shape overflow");
        };
        let Some(weight_required) = out_features
            .checked_mul(in_features)
            .and_then(|n| n.checked_mul(weight_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "weight_data shape overflow");
        };
        let Some(out_len) = outer
            .checked_mul(out_features)
            .and_then(|n| n.checked_mul(out_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
        };

        if x_view.len < x_required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        if weight_view.len < weight_required {
            return raise_exception::<_>(_py, "ValueError", "weight_data buffer is too small");
        }

        let mut out = vec![0u8; out_len];
        if x_format == ScalarFormat::F32
            && weight_format == ScalarFormat::F32
            && out_format == ScalarFormat::F32
        {
            unsafe {
                linear_f32(
                    x_view.ptr,
                    weight_view.ptr,
                    out.as_mut_ptr(),
                    outer,
                    in_features,
                    out_features,
                );
            }
        } else {
            for batch in 0..outer {
                let x_off = batch * in_features;
                let out_off = batch * out_features;
                for out_idx in 0..out_features {
                    let w_off = out_idx * in_features;
                    let mut acc = 0.0f64;
                    for k in 0..in_features {
                        let x = unsafe { read_scalar(x_view.ptr, x_off + k, x_format) };
                        let w = unsafe { read_scalar(weight_view.ptr, w_off + k, weight_format) };
                        acc += x * w;
                    }
                    unsafe { write_scalar(out.as_mut_ptr(), out_off + out_idx, out_format, acc) };
                }
            }
        }

        let out_ptr = alloc_bytes(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[cfg(test)]
mod tests {
    use super::molt_gpu_linear_contiguous;
    use crate::{MoltObject, alloc_bytes, alloc_string, bytes_data, bytes_len, obj_from_bits};

    fn f32_bytes(values: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(values.len() * 4);
        for value in values {
            out.extend_from_slice(&value.to_ne_bytes());
        }
        out
    }

    #[test]
    fn gpu_linear_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let w_ptr = alloc_bytes(_py, &f32_bytes(&[5.0, 6.0, 7.0, 8.0, 9.0, 10.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            assert!(!x_ptr.is_null());
            assert!(!w_ptr.is_null());
            assert!(!fmt_ptr.is_null());

            let out_bits = molt_gpu_linear_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(w_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("linear intrinsic should return bytes");
            let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert_eq!(values, vec![17.0, 23.0, 29.0, 39.0, 53.0, 67.0]);
        });
    }
}
