use crate::{
    MoltObject, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES, TYPE_ID_LIST, TYPE_ID_TUPLE, alloc_bytearray,
    alloc_tuple, bytes_data, bytes_len, obj_from_bits, object_type_id, raise_exception,
    seq_vec_ref, string_obj_to_owned, to_f64, to_i64,
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

unsafe fn linear_split_last_dim_f32(
    x_ptr: *const u8,
    weight_ptr: *const u8,
    out_ptrs: &[*mut u8],
    outer: usize,
    in_features: usize,
    split_sizes: &[usize],
) {
    let mut prefix = 0usize;
    for (part_idx, &part_size) in split_sizes.iter().enumerate() {
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * part_size;
            for out_idx in 0..part_size {
                let w_off = (prefix + out_idx) * in_features;
                let mut acc = 0.0f32;
                for k in 0..in_features {
                    let x = unsafe { (x_ptr.add((x_off + k) * 4) as *const f32).read_unaligned() };
                    let w =
                        unsafe { (weight_ptr.add((w_off + k) * 4) as *const f32).read_unaligned() };
                    acc += x * w;
                }
                unsafe {
                    (out_ptrs[part_idx].add((out_off + out_idx) * 4) as *mut f32)
                        .write_unaligned(acc);
                }
            }
        }
        prefix += part_size;
    }
}

unsafe fn matmul_f32(
    a_ptr: *const u8,
    b_ptr: *const u8,
    out_ptr: *mut u8,
    a_shape: &[usize],
    b_shape: &[usize],
) -> Result<(), ()> {
    if a_shape.len() < 2 || b_shape.len() < 2 {
        return Err(());
    }
    let a_rows = a_shape[a_shape.len() - 2];
    let a_cols = a_shape[a_shape.len() - 1];
    let b_rows = b_shape[b_shape.len() - 2];
    let b_cols = b_shape[b_shape.len() - 1];
    if a_cols != b_rows {
        return Err(());
    }

    let a_batch_shape = &a_shape[..a_shape.len() - 2];
    let b_batch_shape = &b_shape[..b_shape.len() - 2];
    let out_batch_ndim = a_batch_shape.len().max(b_batch_shape.len());
    let mut padded_a_batch_shape = vec![1usize; out_batch_ndim - a_batch_shape.len()];
    padded_a_batch_shape.extend_from_slice(a_batch_shape);
    let mut padded_b_batch_shape = vec![1usize; out_batch_ndim - b_batch_shape.len()];
    padded_b_batch_shape.extend_from_slice(b_batch_shape);

    let mut out_batch_shape = Vec::with_capacity(out_batch_ndim);
    for (&a_dim, &b_dim) in padded_a_batch_shape.iter().zip(padded_b_batch_shape.iter()) {
        if a_dim == b_dim {
            out_batch_shape.push(a_dim);
        } else if a_dim == 1 {
            out_batch_shape.push(b_dim);
        } else if b_dim == 1 {
            out_batch_shape.push(a_dim);
        } else {
            return Err(());
        }
    }

    let batch_count = if out_batch_shape.is_empty() {
        1
    } else {
        product(&out_batch_shape)
    };
    let a_batch_strides = if padded_a_batch_shape.is_empty() {
        vec![]
    } else {
        strides(&padded_a_batch_shape)
    };
    let b_batch_strides = if padded_b_batch_shape.is_empty() {
        vec![]
    } else {
        strides(&padded_b_batch_shape)
    };
    let out_batch_strides = if out_batch_shape.is_empty() {
        vec![]
    } else {
        strides(&out_batch_shape)
    };

    let a_stride = a_rows * a_cols;
    let b_stride = b_rows * b_cols;

    for batch in 0..batch_count {
        let mut rem = batch;
        let mut a_batch_index = 0usize;
        let mut b_batch_index = 0usize;
        for axis in 0..out_batch_strides.len() {
            let stride = out_batch_strides[axis];
            let coord = if stride == 0 { 0 } else { rem / stride };
            rem %= stride.max(1);
            if padded_a_batch_shape[axis] != 1 {
                a_batch_index += coord * a_batch_strides[axis];
            }
            if padded_b_batch_shape[axis] != 1 {
                b_batch_index += coord * b_batch_strides[axis];
            }
        }
                let a_off = a_batch_index * a_stride;
                let b_off = b_batch_index * b_stride;
                let out_off = batch * a_rows * b_cols;
                for i in 0..a_rows {
                    for j in 0..b_cols {
                        let mut acc = 0.0f32;
                        for k in 0..a_cols {
                            let a = unsafe {
                                (a_ptr.add((a_off + i * a_cols + k) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            let b = unsafe {
                                (b_ptr.add((b_off + k * b_cols + j) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            acc += a * b;
                        }
                        unsafe {
                            (out_ptr.add((out_off + i * b_cols + j) * 4) as *mut f32)
                                .write_unaligned(acc);
                        }
                    }
                }
            }
    Ok(())
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

unsafe fn softmax_last_axis_f32(
    x_ptr: *const u8,
    out_ptr: *mut u8,
    outer: usize,
    axis_len: usize,
) {
    for row in 0..outer {
        let base = row * axis_len;
        let mut max_val = f32::NEG_INFINITY;
        for i in 0..axis_len {
            let value = unsafe { (x_ptr.add((base + i) * 4) as *const f32).read_unaligned() };
            if value > max_val {
                max_val = value;
            }
        }
        let mut sum = 0.0f32;
        for i in 0..axis_len {
            let value = unsafe { (x_ptr.add((base + i) * 4) as *const f32).read_unaligned() };
            let exp_v = (value - max_val).exp();
            unsafe { (out_ptr.add((base + i) * 4) as *mut f32).write_unaligned(exp_v) };
            sum += exp_v;
        }
        for i in 0..axis_len {
            let exp_v = unsafe { (out_ptr.add((base + i) * 4) as *const f32).read_unaligned() };
            unsafe { (out_ptr.add((base + i) * 4) as *mut f32).write_unaligned(exp_v / sum) };
        }
    }
}

unsafe fn rms_norm_last_axis_f32(
    x_ptr: *const u8,
    out_ptr: *mut u8,
    outer: usize,
    axis_len: usize,
    eps: f32,
) {
    let axis_len_f32 = axis_len as f32;
    for row in 0..outer {
        let base = row * axis_len;
        let mut sumsq = 0.0f32;
        for i in 0..axis_len {
            let value = unsafe { (x_ptr.add((base + i) * 4) as *const f32).read_unaligned() };
            sumsq += value * value;
        }
        let scale = 1.0f32 / ((sumsq / axis_len_f32) + eps).sqrt();
        for i in 0..axis_len {
            let value = unsafe { (x_ptr.add((base + i) * 4) as *const f32).read_unaligned() };
            unsafe { (out_ptr.add((base + i) * 4) as *mut f32).write_unaligned(value * scale) };
        }
    }
}

unsafe fn squared_relu_gate_interleaved_f32(x_ptr: *const u8, out_ptr: *mut u8, outer: usize, axis_len: usize) {
    let hidden = axis_len / 2;
    for row in 0..outer {
        let in_base = row * axis_len;
        let out_base = row * hidden;
        for i in 0..hidden {
            let gate = unsafe { (x_ptr.add((in_base + 2 * i) * 4) as *const f32).read_unaligned() };
            let up = unsafe {
                (x_ptr.add((in_base + 2 * i + 1) * 4) as *const f32).read_unaligned()
            };
            let relu = gate.max(0.0);
            unsafe {
                (out_ptr.add((out_base + i) * 4) as *mut f32).write_unaligned(relu * relu * up);
            }
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
pub extern "C" fn molt_gpu_linear_split_last_dim_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    weight_data_bits: u64,
    weight_format_bits: u64,
    outer_bits: u64,
    in_features_bits: u64,
    split_sizes_bits: u64,
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
        let split_sizes = match parse_shape(_py, split_sizes_bits, "split_sizes") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(out_features) = split_sizes.iter().try_fold(0usize, |acc, size| acc.checked_add(*size))
        else {
            return raise_exception::<_>(_py, "OverflowError", "split_sizes overflow");
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
        if x_view.len < x_required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        if weight_view.len < weight_required {
            return raise_exception::<_>(_py, "ValueError", "weight_data buffer is too small");
        }

        let mut outputs: Vec<Vec<u8>> = Vec::with_capacity(split_sizes.len());
        for &size in &split_sizes {
            let Some(out_len) = outer
                .checked_mul(size)
                .and_then(|n| n.checked_mul(out_format.itemsize()))
            else {
                return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
            };
            outputs.push(vec![0u8; out_len]);
        }

        if x_format == ScalarFormat::F32
            && weight_format == ScalarFormat::F32
            && out_format == ScalarFormat::F32
        {
            let out_ptrs: Vec<*mut u8> = outputs.iter_mut().map(|out| out.as_mut_ptr()).collect();
            unsafe {
                linear_split_last_dim_f32(
                    x_view.ptr,
                    weight_view.ptr,
                    out_ptrs.as_slice(),
                    outer,
                    in_features,
                    split_sizes.as_slice(),
                );
            }
        } else {
            let mut prefix = 0usize;
            for (part_idx, &part_size) in split_sizes.iter().enumerate() {
                for batch in 0..outer {
                    let x_off = batch * in_features;
                    let out_off = batch * part_size;
                    for out_idx in 0..part_size {
                        let w_off = (prefix + out_idx) * in_features;
                        let mut acc = 0.0f64;
                        for k in 0..in_features {
                            let x = unsafe { read_scalar(x_view.ptr, x_off + k, x_format) };
                            let w =
                                unsafe { read_scalar(weight_view.ptr, w_off + k, weight_format) };
                            acc += x * w;
                        }
                        unsafe {
                            write_scalar(
                                outputs[part_idx].as_mut_ptr(),
                                out_off + out_idx,
                                out_format,
                                acc,
                            )
                        };
                    }
                }
                prefix += part_size;
            }
        }

        let mut out_bits = Vec::with_capacity(outputs.len());
        for out in outputs {
            let out_ptr = alloc_bytearray(_py, &out);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(out_ptr).bits());
        }
        let tuple_ptr = alloc_tuple(_py, out_bits.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
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
pub extern "C" fn molt_gpu_matmul_contiguous(
    a_data_bits: u64,
    a_format_bits: u64,
    a_shape_bits: u64,
    b_data_bits: u64,
    b_format_bits: u64,
    b_shape_bits: u64,
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
        if a_shape.len() < 2 || b_shape.len() < 2 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "matmul requires tensors with at least 2 dimensions",
            );
        }

        let a_rows = a_shape[a_shape.len() - 2];
        let a_cols = a_shape[a_shape.len() - 1];
        let b_rows = b_shape[b_shape.len() - 2];
        let b_cols = b_shape[b_shape.len() - 1];
        if a_cols != b_rows {
            return raise_exception::<_>(_py, "ValueError", "matmul dimension mismatch");
        }

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

        let a_batch_shape = &a_shape[..a_shape.len() - 2];
        let b_batch_shape = &b_shape[..b_shape.len() - 2];
        let out_batch_ndim = a_batch_shape.len().max(b_batch_shape.len());
        let mut padded_a_batch_shape = vec![1usize; out_batch_ndim - a_batch_shape.len()];
        padded_a_batch_shape.extend_from_slice(a_batch_shape);
        let mut padded_b_batch_shape = vec![1usize; out_batch_ndim - b_batch_shape.len()];
        padded_b_batch_shape.extend_from_slice(b_batch_shape);
        let mut out_batch_shape = Vec::with_capacity(out_batch_ndim);
        for (&a_dim, &b_dim) in padded_a_batch_shape.iter().zip(padded_b_batch_shape.iter()) {
            if a_dim == b_dim {
                out_batch_shape.push(a_dim);
            } else if a_dim == 1 {
                out_batch_shape.push(b_dim);
            } else if b_dim == 1 {
                out_batch_shape.push(a_dim);
            } else {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "matmul batch shape mismatch",
                );
            }
        }
        let batch_count = if out_batch_shape.is_empty() {
            1
        } else {
            product(&out_batch_shape)
        };
        let Some(out_elems) = batch_count
            .checked_mul(a_rows)
            .and_then(|n| n.checked_mul(b_cols))
        else {
            return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
        };
        let Some(out_len) = out_elems.checked_mul(out_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "output byte size overflow");
        };

        let mut out = vec![0u8; out_len];
        if a_format == ScalarFormat::F32
            && b_format == ScalarFormat::F32
            && out_format == ScalarFormat::F32
        {
            if unsafe { matmul_f32(a_view.ptr, b_view.ptr, out.as_mut_ptr(), &a_shape, &b_shape) }
                .is_err()
            {
                return raise_exception::<_>(_py, "ValueError", "matmul batch shape mismatch");
            }
        } else {
            let a_stride = a_rows * a_cols;
            let b_stride = b_rows * b_cols;
            let a_batch_strides = if padded_a_batch_shape.is_empty() {
                vec![]
            } else {
                strides(&padded_a_batch_shape)
            };
            let b_batch_strides = if padded_b_batch_shape.is_empty() {
                vec![]
            } else {
                strides(&padded_b_batch_shape)
            };
            let out_batch_strides = if out_batch_shape.is_empty() {
                vec![]
            } else {
                strides(&out_batch_shape)
            };
            for batch in 0..batch_count {
                let mut rem = batch;
                let mut a_batch_index = 0usize;
                let mut b_batch_index = 0usize;
                for axis in 0..out_batch_strides.len() {
                    let stride = out_batch_strides[axis];
                    let coord = if stride == 0 { 0 } else { rem / stride };
                    rem %= stride.max(1);
                    if padded_a_batch_shape[axis] != 1 {
                        a_batch_index += coord * a_batch_strides[axis];
                    }
                    if padded_b_batch_shape[axis] != 1 {
                        b_batch_index += coord * b_batch_strides[axis];
                    }
                }
                let a_off = a_batch_index * a_stride;
                let b_off = b_batch_index * b_stride;
                let out_off = batch * a_rows * b_cols;
                for i in 0..a_rows {
                    for j in 0..b_cols {
                        let mut acc = 0.0f64;
                        for k in 0..a_cols {
                            let a = unsafe {
                                read_scalar(a_view.ptr, a_off + i * a_cols + k, a_format)
                            };
                            let b = unsafe {
                                read_scalar(b_view.ptr, b_off + k * b_cols + j, b_format)
                            };
                            acc += a * b;
                        }
                        unsafe {
                            write_scalar(out.as_mut_ptr(), out_off + i * b_cols + j, out_format, acc)
                        };
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_softmax_last_axis_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
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
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if shape.is_empty() {
            let mut out = vec![0u8; out_format.itemsize()];
            unsafe { write_scalar(out.as_mut_ptr(), 0, out_format, 1.0) };
            let out_ptr = alloc_bytearray(_py, &out);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let total_elems = product(&shape);
        let Some(required) = total_elems.checked_mul(x_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "softmax shape overflow");
        };
        if x_view.len < required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        let axis_len = *shape.last().unwrap_or(&1);
        let outer = if axis_len == 0 { 0 } else { total_elems / axis_len };
        let mut out = vec![0u8; total_elems * out_format.itemsize()];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe { softmax_last_axis_f32(x_view.ptr, out.as_mut_ptr(), outer, axis_len) };
        } else {
            for row in 0..outer {
                let base = row * axis_len;
                let mut max_val = f64::NEG_INFINITY;
                for i in 0..axis_len {
                    let value = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                    if value > max_val {
                        max_val = value;
                    }
                }
                let mut sum = 0.0f64;
                for i in 0..axis_len {
                    let value = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                    let exp_v = (value - max_val).exp();
                    unsafe { write_scalar(out.as_mut_ptr(), base + i, out_format, exp_v) };
                    sum += exp_v;
                }
                for i in 0..axis_len {
                    let exp_v = unsafe { read_scalar(out.as_ptr(), base + i, out_format) };
                    unsafe { write_scalar(out.as_mut_ptr(), base + i, out_format, exp_v / sum) };
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
pub extern "C" fn molt_gpu_rms_norm_last_axis_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
    eps_bits: u64,
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
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(shape) => shape,
            Err(bits) => return bits,
        };
        let Some(eps) = to_f64(obj_from_bits(eps_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "eps must be a float");
        };
        if shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "rms_norm requires a tensor with at least 1 dimension",
            );
        }
        let axis_len = shape[shape.len() - 1];
        if axis_len == 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "rms_norm last axis must be non-empty",
            );
        }
        let total_elems = product(&shape);
        if x_view.len != total_elems * x_format.itemsize() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "x_data byte length does not match shape",
            );
        }
        let outer = total_elems / axis_len;
        let mut out = vec![0u8; total_elems * out_format.itemsize()];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe {
                rms_norm_last_axis_f32(
                    x_view.ptr,
                    out.as_mut_ptr(),
                    outer,
                    axis_len,
                    eps as f32,
                )
            };
        } else {
            let axis_len_f64 = axis_len as f64;
            for row in 0..outer {
                let base = row * axis_len;
                let mut sumsq = 0.0f64;
                for i in 0..axis_len {
                    let value = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                    sumsq += value * value;
                }
                let scale = 1.0f64 / ((sumsq / axis_len_f64) + eps).sqrt();
                for i in 0..axis_len {
                    let value = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                    unsafe { write_scalar(out.as_mut_ptr(), base + i, out_format, value * scale) };
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
pub extern "C" fn molt_gpu_squared_relu_gate_interleaved_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
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
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(shape) => shape,
            Err(bits) => return bits,
        };
        if shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "squared_relu_gate_interleaved requires a tensor with at least 1 dimension",
            );
        }
        let axis_len = shape[shape.len() - 1];
        if axis_len % 2 != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "squared_relu_gate_interleaved last axis must be even",
            );
        }
        let total_elems = product(&shape);
        if x_view.len != total_elems * x_format.itemsize() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "x_data byte length does not match shape",
            );
        }
        let outer = if axis_len == 0 { 0 } else { total_elems / axis_len };
        let out_elems = outer * (axis_len / 2);
        let mut out = vec![0u8; out_elems * out_format.itemsize()];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe { squared_relu_gate_interleaved_f32(x_view.ptr, out.as_mut_ptr(), outer, axis_len) };
        } else {
            let hidden = axis_len / 2;
            for row in 0..outer {
                let in_base = row * axis_len;
                let out_base = row * hidden;
                for i in 0..hidden {
                    let gate = unsafe { read_scalar(x_view.ptr, in_base + 2 * i, x_format) };
                    let up = unsafe { read_scalar(x_view.ptr, in_base + 2 * i + 1, x_format) };
                    let relu = gate.max(0.0);
                    unsafe {
                        write_scalar(
                            out.as_mut_ptr(),
                            out_base + i,
                            out_format,
                            relu * relu * up,
                        )
                    };
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

#[cfg(test)]
mod tests {
    use super::{
        molt_gpu_broadcast_binary_contiguous, molt_gpu_linear_contiguous,
        molt_gpu_linear_split_last_dim_contiguous,
        molt_gpu_matmul_contiguous, molt_gpu_rms_norm_last_axis_contiguous,
        molt_gpu_rope_apply_contiguous, molt_gpu_softmax_last_axis_contiguous,
        molt_gpu_squared_relu_gate_interleaved_contiguous,
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
    fn gpu_linear_split_last_dim_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let w_ptr = alloc_bytes(
                _py,
                &f32_bytes(&[1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, 2.0]),
            );
            let fmt_ptr = alloc_string(_py, b"f");
            let sizes_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(2).bits(), MoltObject::from_int(3).bits()],
            );

            let out_bits = molt_gpu_linear_split_last_dim_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(w_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_ptr(sizes_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("linear split intrinsic should return tuple");
            let parts = unsafe { crate::seq_vec_ref(out_ptr) };
            assert_eq!(parts.len(), 2);

            let left_ptr = obj_from_bits(parts[0]).as_ptr().expect("left bytes");
            let left =
                unsafe { std::slice::from_raw_parts(bytes_data(left_ptr), bytes_len(left_ptr)) };
            let right_ptr = obj_from_bits(parts[1]).as_ptr().expect("right bytes");
            let right =
                unsafe { std::slice::from_raw_parts(bytes_data(right_ptr), bytes_len(right_ptr)) };

            let mut left_values = Vec::new();
            for chunk in left.chunks_exact(4) {
                left_values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            let mut right_values = Vec::new();
            for chunk in right.chunks_exact(4) {
                right_values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }

            assert_eq!(left_values, vec![1.0, 2.0, 3.0, 4.0]);
            assert_eq!(right_values, vec![3.0, 2.0, 4.0, 7.0, 6.0, 8.0]);
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
    fn gpu_matmul_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let a_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let b_ptr = alloc_bytes(_py, &f32_bytes(&[5.0, 6.0, 7.0, 8.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            let a_shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(2).bits(), MoltObject::from_int(2).bits()],
            );
            let b_shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(2).bits(), MoltObject::from_int(2).bits()],
            );

            let out_bits = molt_gpu_matmul_contiguous(
                MoltObject::from_ptr(a_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(a_shape_ptr).bits(),
                MoltObject::from_ptr(b_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(b_shape_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("matmul intrinsic should return bytes");
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert_eq!(values, vec![19.0, 22.0, 43.0, 50.0]);
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

    #[test]
    fn gpu_softmax_last_axis_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            let shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(2).bits(), MoltObject::from_int(2).bits()],
            );

            let out_bits = molt_gpu_softmax_last_axis_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(shape_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("softmax intrinsic should return bytes");
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert!((values[0] + values[1] - 1.0).abs() < 1e-6);
            assert!((values[2] + values[3] - 1.0).abs() < 1e-6);
        });
    }

    #[test]
    fn gpu_rms_norm_last_axis_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[3.0, 4.0, 0.0, 5.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            let shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(2).bits(), MoltObject::from_int(2).bits()],
            );

            let out_bits = molt_gpu_rms_norm_last_axis_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(shape_ptr).bits(),
                MoltObject::from_float(0.0).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("rms_norm intrinsic should return bytes");
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert!((values[0] - 0.84852815).abs() < 1e-6);
            assert!((values[1] - 1.1313709).abs() < 1e-6);
            assert!((values[2] - 0.0).abs() < 1e-6);
            assert!((values[3] - std::f32::consts::SQRT_2).abs() < 1e-6);
        });
    }

    #[test]
    fn gpu_squared_relu_gate_interleaved_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 10.0, -2.0, 20.0, 3.0, 30.0, 4.0, 40.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            let shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(1).bits(), MoltObject::from_int(8).bits()],
            );

            let out_bits = molt_gpu_squared_relu_gate_interleaved_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(shape_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("squared relu gate intrinsic should return bytes");
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert_eq!(values, vec![10.0, 0.0, 270.0, 640.0]);
        });
    }
}
