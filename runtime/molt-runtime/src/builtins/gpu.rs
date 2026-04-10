use crate::{
    MoltObject, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES, TYPE_ID_LIST, TYPE_ID_TUPLE, alloc_bytearray,
    bytes_data, bytes_len, obj_from_bits, object_type_id, raise_exception, seq_vec_ref,
    string_obj_to_owned, to_i64,
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

fn parse_shape(_py: &crate::PyToken<'_>, bits: u64, role: &str) -> Result<Vec<usize>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a tuple or list of ints"),
        ));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a tuple or list of ints"),
        ));
    }
    let mut out = Vec::new();
    for dim_bits in unsafe { seq_vec_ref(ptr) }.iter().copied() {
        let Some(dim) = to_i64(obj_from_bits(dim_bits)) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                &format!("{role} must contain integers"),
            ));
        };
        let dim = usize::try_from(dim).map_err(|_| {
            raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("{role} dimensions must be non-negative"),
            )
        })?;
        out.push(dim);
    }
    Ok(out)
}

fn product(shape: &[usize]) -> usize {
    let mut out = 1usize;
    for dim in shape {
        out *= *dim;
    }
    out
}

fn strides(shape: &[usize]) -> Vec<usize> {
    let mut out = vec![0; shape.len()];
    let mut stride = 1usize;
    for (i, dim) in shape.iter().enumerate().rev() {
        out[i] = stride;
        stride *= *dim;
    }
    out
}

fn validate_permutation(_py: &crate::PyToken<'_>, dims: &[usize], ndim: usize) -> Result<(), u64> {
    if dims.len() != ndim {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "permute dims must match tensor rank",
        ));
    }
    let mut seen = vec![false; ndim];
    for &dim in dims {
        if dim >= ndim || seen[dim] {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "permute dims must be a permutation",
            ));
        }
        seen[dim] = true;
    }
    Ok(())
}

fn apply_binary_op(_py: &crate::PyToken<'_>, op_code: i64, a: f64, b: f64) -> Result<f64, u64> {
    match op_code {
        0 => Ok(a + b),
        1 => Ok(a - b),
        2 => Ok(a * b),
        3 => {
            if b == 0.0 {
                if a > 0.0 {
                    Ok(f64::INFINITY)
                } else if a < 0.0 {
                    Ok(f64::NEG_INFINITY)
                } else {
                    Ok(f64::NAN)
                }
            } else {
                Ok(a / b)
            }
        }
        _ => Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            &format!("unsupported broadcast op code {}", op_code),
        )),
    }
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

unsafe fn rope_apply_f32(
    x_ptr: *const u8,
    cos_ptr: *const u8,
    sin_ptr: *const u8,
    out_ptr: *mut u8,
    batch: usize,
    seq: usize,
    heads: usize,
    dim: usize,
    freq_dim: usize,
    seq_len: usize,
) {
    let half = dim / 2;
    let max_seq = seq.min(seq_len);
    unsafe {
        for b in 0..batch {
            for s in 0..max_seq {
                let freq_base = s * freq_dim;
                for h in 0..heads {
                    let base = ((b * seq + s) * heads + h) * dim;
                    for i in 0..half {
                        let (cos_v, sin_v) = if i < freq_dim {
                            (
                                (cos_ptr.add((freq_base + i) * 4) as *const f32)
                                    .read_unaligned(),
                                (sin_ptr.add((freq_base + i) * 4) as *const f32)
                                    .read_unaligned(),
                            )
                        } else {
                            (1.0f32, 0.0f32)
                        };
                        let x0 = (x_ptr.add((base + i) * 4) as *const f32).read_unaligned();
                        let x1 = if i + half < dim {
                            (x_ptr.add((base + i + half) * 4) as *const f32).read_unaligned()
                        } else {
                            0.0f32
                        };
                        (out_ptr.add((base + i) * 4) as *mut f32)
                            .write_unaligned(x0 * cos_v - x1 * sin_v);
                        if i + half < dim {
                            (out_ptr.add((base + i + half) * 4) as *mut f32)
                                .write_unaligned(x0 * sin_v + x1 * cos_v);
                        }
                    }
                }
            }
        }
        if max_seq < seq {
            let start_elem = batch * max_seq * heads * dim;
            let remaining_elems = batch * (seq - max_seq) * heads * dim;
            let byte_len = remaining_elems * 4;
            std::ptr::copy_nonoverlapping(
                x_ptr.add(start_elem * 4),
                out_ptr.add(start_elem * 4),
                byte_len,
            );
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

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_broadcast_binary_contiguous(
    a_data_bits: u64,
    a_format_bits: u64,
    a_shape_bits: u64,
    b_data_bits: u64,
    b_format_bits: u64,
    b_shape_bits: u64,
    op_code_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let a_format = match parse_format(_py, a_format_bits, "a_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let b_format = match parse_format(_py, b_format_bits, "b_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let a_shape = match parse_shape(_py, a_shape_bits, "a_shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let b_shape = match parse_shape(_py, b_shape_bits, "b_shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(op_code) = to_i64(obj_from_bits(op_code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "op_code must be an integer");
        };
        let a_view = match bytes_like_view(_py, a_data_bits, "a_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let b_view = match bytes_like_view(_py, b_data_bits, "b_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let a_elems = product(&a_shape);
        let b_elems = product(&b_shape);
        let Some(a_required) = a_elems.checked_mul(a_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "a_data shape overflow");
        };
        let Some(b_required) = b_elems.checked_mul(b_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "b_data shape overflow");
        };
        if a_view.len < a_required {
            return raise_exception::<_>(_py, "ValueError", "a_data buffer is too small");
        }
        if b_view.len < b_required {
            return raise_exception::<_>(_py, "ValueError", "b_data buffer is too small");
        }

        let out_ndim = a_shape.len().max(b_shape.len());
        let mut a_padded = vec![1usize; out_ndim - a_shape.len()];
        a_padded.extend_from_slice(&a_shape);
        let mut b_padded = vec![1usize; out_ndim - b_shape.len()];
        b_padded.extend_from_slice(&b_shape);
        let mut out_shape = Vec::with_capacity(out_ndim);
        for (&a_dim, &b_dim) in a_padded.iter().zip(b_padded.iter()) {
            if a_dim == b_dim {
                out_shape.push(a_dim);
            } else if a_dim == 1 {
                out_shape.push(b_dim);
            } else if b_dim == 1 {
                out_shape.push(a_dim);
            } else {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "Cannot broadcast input shapes",
                );
            }
        }
        let out_elems = product(&out_shape);
        let out_strides = strides(&out_shape);
        let a_strides = strides(&a_padded);
        let b_strides = strides(&b_padded);
        let Some(out_len) = out_elems.checked_mul(out_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
        };
        let mut out = vec![0u8; out_len];

        for out_index in 0..out_elems {
            let mut rem = out_index;
            let mut a_index = 0usize;
            let mut b_index = 0usize;
            for axis in 0..out_ndim {
                let stride = out_strides[axis];
                let coord = if stride == 0 { 0 } else { rem / stride };
                rem %= stride.max(1);
                if a_padded[axis] != 1 {
                    a_index += coord * a_strides[axis];
                }
                if b_padded[axis] != 1 {
                    b_index += coord * b_strides[axis];
                }
            }
            let a = unsafe { read_scalar(a_view.ptr, a_index, a_format) };
            let b = unsafe { read_scalar(b_view.ptr, b_index, b_format) };
            let value = match apply_binary_op(_py, op_code, a, b) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            unsafe { write_scalar(out.as_mut_ptr(), out_index, out_format, value) };
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_rope_apply_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    cos_data_bits: u64,
    sin_data_bits: u64,
    freq_dim_bits: u64,
    batch_bits: u64,
    seq_bits: u64,
    heads_bits: u64,
    dim_bits: u64,
    seq_len_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let freq_dim = match parse_usize_arg(_py, freq_dim_bits, "freq_dim") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let batch = match parse_usize_arg(_py, batch_bits, "batch") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let seq = match parse_usize_arg(_py, seq_bits, "seq") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let heads = match parse_usize_arg(_py, heads_bits, "heads") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let dim = match parse_usize_arg(_py, dim_bits, "dim") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let seq_len = match parse_usize_arg(_py, seq_len_bits, "seq_len") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if dim % 2 != 0 {
            return raise_exception::<_>(_py, "ValueError", "dim must be even");
        }

        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let cos_view = match bytes_like_view(_py, cos_data_bits, "cos_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let sin_view = match bytes_like_view(_py, sin_data_bits, "sin_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let Some(total_elems) = batch
            .checked_mul(seq)
            .and_then(|n| n.checked_mul(heads))
            .and_then(|n| n.checked_mul(dim))
        else {
            return raise_exception::<_>(_py, "OverflowError", "rope tensor shape overflow");
        };
        let Some(x_required) = total_elems.checked_mul(x_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "x_data shape overflow");
        };
        let Some(freq_required) = seq_len
            .checked_mul(freq_dim)
            .and_then(|n| n.checked_mul(4))
        else {
            return raise_exception::<_>(_py, "OverflowError", "freq buffer shape overflow");
        };
        let Some(out_len) = total_elems.checked_mul(out_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "rope output shape overflow");
        };

        if x_view.len < x_required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        if cos_view.len < freq_required {
            return raise_exception::<_>(_py, "ValueError", "cos_data buffer is too small");
        }
        if sin_view.len < freq_required {
            return raise_exception::<_>(_py, "ValueError", "sin_data buffer is too small");
        }

        let mut out = vec![0u8; out_len];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe {
                rope_apply_f32(
                    x_view.ptr,
                    cos_view.ptr,
                    sin_view.ptr,
                    out.as_mut_ptr(),
                    batch,
                    seq,
                    heads,
                    dim,
                    freq_dim,
                    seq_len,
                );
            }
        } else {
            let half = dim / 2;
            let max_seq = seq.min(seq_len);
            for b in 0..batch {
                for s in 0..seq {
                    let freq_base = s * freq_dim;
                    for h in 0..heads {
                        let base = ((b * seq + s) * heads + h) * dim;
                        if s >= max_seq {
                            for i in 0..dim {
                                let x = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                                unsafe { write_scalar(out.as_mut_ptr(), base + i, out_format, x) };
                            }
                            continue;
                        }
                        for i in 0..half {
                            let (cos_v, sin_v) = if i < freq_dim {
                                (
                                    unsafe { read_scalar(cos_view.ptr, freq_base + i, ScalarFormat::F32) },
                                    unsafe { read_scalar(sin_view.ptr, freq_base + i, ScalarFormat::F32) },
                                )
                            } else {
                                (1.0f64, 0.0f64)
                            };
                            let x0 = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                            let x1 = if i + half < dim {
                                unsafe { read_scalar(x_view.ptr, base + i + half, x_format) }
                            } else {
                                0.0
                            };
                            unsafe {
                                write_scalar(
                                    out.as_mut_ptr(),
                                    base + i,
                                    out_format,
                                    x0 * cos_v - x1 * sin_v,
                                );
                            }
                            if i + half < dim {
                                unsafe {
                                    write_scalar(
                                        out.as_mut_ptr(),
                                        base + i + half,
                                        out_format,
                                        x0 * sin_v + x1 * cos_v,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_permute_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
    dims_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if x_format != out_format {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "permute requires matching input/output formats",
            );
        }
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let dims = match parse_shape(_py, dims_bits, "dims") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(bits) = validate_permutation(_py, &dims, shape.len()) {
            return bits;
        }
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let total_elems = product(&shape);
        let itemsize = x_format.itemsize();
        let Some(required) = total_elems.checked_mul(itemsize) else {
            return raise_exception::<_>(_py, "OverflowError", "permute shape overflow");
        };
        if x_view.len < required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        let out_shape: Vec<usize> = dims.iter().map(|&dim| shape[dim]).collect();
        let old_strides = strides(&shape);
        let new_strides = strides(&out_shape);
        let mut out = vec![0u8; required];
        let src = unsafe { std::slice::from_raw_parts(x_view.ptr, required) };
        for old_index in 0..total_elems {
            let mut rem = old_index;
            let mut coords = vec![0usize; shape.len()];
            for axis in 0..shape.len() {
                let stride = old_strides[axis];
                coords[axis] = if stride == 0 { 0 } else { rem / stride };
                rem %= stride.max(1);
            }
            let mut new_index = 0usize;
            for (new_axis, &old_axis) in dims.iter().enumerate() {
                new_index += coords[old_axis] * new_strides[new_axis];
            }
            let src_base = old_index * itemsize;
            let dst_base = new_index * itemsize;
            out[dst_base..dst_base + itemsize]
                .copy_from_slice(&src[src_base..src_base + itemsize]);
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        molt_gpu_broadcast_binary_contiguous, molt_gpu_linear_contiguous,
        molt_gpu_rope_apply_contiguous,
    };
    use crate::{
        MoltObject, alloc_bytes, alloc_string, alloc_tuple, bytes_data, bytes_len, obj_from_bits,
    };

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

    #[test]
    fn gpu_broadcast_binary_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let a_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let b_ptr = alloc_bytes(_py, &f32_bytes(&[10.0, 20.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            let a_shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(2).bits(), MoltObject::from_int(2).bits()],
            );
            let b_shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(1).bits(), MoltObject::from_int(2).bits()],
            );

            let out_bits = molt_gpu_broadcast_binary_contiguous(
                MoltObject::from_ptr(a_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(a_shape_ptr).bits(),
                MoltObject::from_ptr(b_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(b_shape_ptr).bits(),
                MoltObject::from_int(0).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("broadcast intrinsic should return bytes-like");
            let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert_eq!(values, vec![11.0, 22.0, 13.0, 24.0]);
        });
    }

    #[test]
    fn gpu_rope_apply_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let cos_ptr = alloc_bytes(_py, &f32_bytes(&[0.0, 1.0]));
            let sin_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 0.0]));
            let fmt_ptr = alloc_string(_py, b"f");

            let out_bits = molt_gpu_rope_apply_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(cos_ptr).bits(),
                MoltObject::from_ptr(sin_ptr).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(4).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("rope intrinsic should return bytes-like");
            let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert_eq!(values, vec![-3.0, 2.0, 1.0, 4.0]);
        });
    }

    #[test]
    fn gpu_rope_apply_contiguous_rejects_odd_dim() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0]));
            let cos_ptr = alloc_bytes(_py, &f32_bytes(&[1.0]));
            let sin_ptr = alloc_bytes(_py, &f32_bytes(&[0.0]));
            let fmt_ptr = alloc_string(_py, b"f");

            let out_bits = molt_gpu_rope_apply_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(cos_ptr).bits(),
                MoltObject::from_ptr(sin_ptr).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(3).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );

            assert!(crate::exception_pending(_py));
            let _ = out_bits;
        });
    }
}
